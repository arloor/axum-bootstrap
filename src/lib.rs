use std::{convert::Infallible, fmt::Display, net::SocketAddr, sync::Arc, time::Duration};

pub mod init_log;
pub mod util;
type DynError = Box<dyn std::error::Error + Send + Sync>;
use crate::util::{
    io::{self, create_dual_stack_listener},
    tls::{tls_config, TlsAcceptor},
};
use anyhow::anyhow;
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    Router,
};

use hyper::{body::Incoming, StatusCode};
use hyper_util::rt::TokioExecutor;
use log::{info, warn};
use tokio::{pin, sync::broadcast, time};
use tokio_rustls::rustls::ServerConfig;
use tower::{Service, ServiceExt};

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Server<I: ReqInterceptor = DummyInterceptor> {
    pub port: u16,
    pub tls_param: Option<TlsParam>,
    router: Router,
    pub interceptor: Option<I>,
    pub idle_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct TlsParam {
    pub tls: bool,
    pub cert: String,
    pub key: String,
}

pub enum InterceptResult {
    Return(Response),
    Continue(Request<Incoming>),
    Error(AppError),
}

pub trait ReqInterceptor {
    fn intercept(&self, req: Request<Incoming>, ip: SocketAddr) -> impl std::future::Future<Output = InterceptResult> + Send;
}

#[derive(Clone)]
pub struct DummyInterceptor;

impl ReqInterceptor for DummyInterceptor {
    async fn intercept(&self, req: Request<Incoming>, _ip: SocketAddr) -> InterceptResult {
        InterceptResult::Continue(req)
    }
}

pub type DefaultServer = Server<DummyInterceptor>;

pub fn new_server(port: u16, tls_param: Option<TlsParam>, router: Router) -> Server {
    Server {
        port,
        tls_param,
        router,
        interceptor: None,
        idle_timeout: Duration::from_secs(120),
    }
}

pub fn new_server_with_interceptor<I>(port: u16, tls_param: Option<TlsParam>, interceptor: I, router: Router) -> Server<I>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    Server {
        port,
        tls_param,
        router,
        interceptor: Some(interceptor),
        idle_timeout: Duration::from_secs(120),
    }
}

impl<I> Server<I>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    pub async fn run(&self) -> Result<(), DynError> {
        let use_tls = match self.tls_param.clone() {
            Some(config) => config.tls,
            None => false,
        };
        log::info!("listening on port {}, use_tls: {}", self.port, use_tls);
        let server: hyper_util::server::conn::auto::Builder<TokioExecutor> = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
        let graceful: hyper_util::server::graceful::GracefulShutdown = hyper_util::server::graceful::GracefulShutdown::new();
        match use_tls {
            #[allow(clippy::expect_used)]
            true => {
                serve_tls(
                    &self.router,
                    server,
                    graceful,
                    self.port,
                    self.tls_param.as_ref().expect("should be some"),
                    self.interceptor.clone(),
                    self.idle_timeout,
                )
                .await?
            }
            false => serve_plantext(&self.router, server, graceful, self.port, self.interceptor.clone(), self.idle_timeout).await?,
        }
        Ok(())
    }
}

async fn handle<I>(
    request: Request<Incoming>, client_socket_addr: SocketAddr, app: axum::middleware::AddExtension<Router, axum::extract::ConnectInfo<SocketAddr>>,
    interceptor: Option<I>,
) -> std::result::Result<Response, std::convert::Infallible>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    if let Some(interceptor) = interceptor {
        match interceptor.intercept(request, client_socket_addr).await {
            InterceptResult::Continue(req) => app.oneshot(req).await,
            InterceptResult::Return(res) => Ok(res),
            InterceptResult::Error(err) => {
                let res = err.into_response();
                Ok(res)
            }
        }
    } else {
        app.oneshot(request).await
    }
}

async fn handle_connection<C, I>(
    conn: C, client_socket_addr: std::net::SocketAddr, mut app: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, SocketAddr>,
    server: hyper_util::server::conn::auto::Builder<TokioExecutor>, interceptor: Option<I>,
    graceful: &hyper_util::server::graceful::GracefulShutdown, timeout: Duration,
) where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static + Send + Sync,
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let timeout_io = Box::pin(io::TimeoutIO::new(conn, timeout));
    use hyper::Request;
    use hyper_util::rt::TokioIo;
    let stream = TokioIo::new(timeout_io);
    let app: axum::middleware::AddExtension<Router, axum::extract::ConnectInfo<SocketAddr>> = unwrap_infallible(app.call(client_socket_addr).await);
    // https://github.com/tokio-rs/axum/blob/main/examples/serve-with-hyper/src/main.rs#L81
    let hyper_service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        handle(request, client_socket_addr, app.clone(), interceptor.clone())
    });

    let conn = server.serve_connection_with_upgrades(stream, hyper_service);
    let conn = graceful.watch(conn.into_owned());

    tokio::spawn(async move {
        if let Err(err) = conn.await {
            info!("connection error: {}", err);
        }
        log::debug!("connection dropped: {}", client_socket_addr);
    });
}

async fn serve_plantext<I>(
    app: &Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>, graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16, interceptor: Option<I>, timeout: Duration,
) -> Result<(), DynError>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let listener = create_dual_stack_listener(port).await?;
    let signal = handle_signal();
    pin!(signal);
    loop {
        tokio::select! {
            _ = signal.as_mut() => {
                info!("start graceful shutdown!");
                drop(listener);
                break;
            }
            conn = listener.accept() => {
                match conn {
                    Ok((conn, client_socket_addr)) => {
                        let app: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, SocketAddr> = app.clone().into_make_service_with_connect_info::<SocketAddr>();
                        handle_connection(conn,client_socket_addr, app, server.clone(),interceptor.clone(), &graceful, timeout).await;}
                    Err(e) => {
                        warn!("accept error:{}", e);
                    }
                }
            }
        }
    }
    tokio::select! {
        _ = graceful.shutdown() => {
            info!("Gracefully shutdown!");
        },
        _ = tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT) => {
            info!("Waited {GRACEFUL_SHUTDOWN_TIMEOUT:?} for graceful shutdown, aborting...");
        }
    }
    Ok(())
}

async fn serve_tls<I>(
    app: &Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>, graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16, tls_param: &TlsParam, interceptor: Option<I>, timeout: Duration,
) -> Result<(), DynError>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let (tx, _rx) = broadcast::channel::<Arc<ServerConfig>>(10);
    let tx_clone = tx.clone();
    let tls_param_clone = tls_param.clone();
    tokio::spawn(async move {
        info!("update tls config every {:?}", REFRESH_INTERVAL);
        loop {
            time::sleep(REFRESH_INTERVAL).await;
            if let Ok(new_acceptor) = tls_config(&tls_param_clone.key, &tls_param_clone.cert) {
                info!("update tls config");
                if let Err(e) = tx.send(new_acceptor) {
                    warn!("send tls config error:{}", e);
                }
            }
        }
    });
    let mut rx = tx_clone.subscribe();
    let mut acceptor: TlsAcceptor = TlsAcceptor::new(tls_config(&tls_param.key, &tls_param.cert)?, create_dual_stack_listener(port).await?);
    let signal = handle_signal();
    pin!(signal);
    loop {
        tokio::select! {
            _ = signal.as_mut() => {
                info!("start graceful shutdown!");
                drop(acceptor);
                break;
            }
            message = rx.recv() => {
                #[allow(clippy::expect_used)]
                let new_config = message.expect("Channel should not be closed");
                // Replace the acceptor with the new one
                acceptor.replace_config(new_config);
                info!("replaced tls config");
            }
            conn = acceptor.accept() => {
                match conn {
                    Ok((conn, client_socket_addr)) => {
                        let app: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, SocketAddr> = app.clone().into_make_service_with_connect_info::<SocketAddr>();
                        handle_connection(conn,client_socket_addr, app, server.clone(),interceptor.clone(), &graceful, timeout).await;}
                    Err(e) => {
                        warn!("accept error:{}", e);
                    }
                }
            }
        }
    }
    tokio::select! {
        _ = graceful.shutdown() => {
            info!("Gracefully shutdown!");
        },
        _ = tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT) => {
            info!("Waited {GRACEFUL_SHUTDOWN_TIMEOUT:?} for graceful shutdown, aborting...");
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn handle_signal() -> Result<(), DynError> {
    use log::info;
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate_signal = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = terminate_signal.recv() => {
            info!("receive terminate signal, shutdowning");
        },
        _ = tokio::signal::ctrl_c() => {
            info!("ctrl_c => shutdowning");
        },
    };
    Ok(())
}

#[cfg(windows)]
async fn handle_signal() -> Result<(), DynError> {
    let _ = tokio::signal::ctrl_c().await;
    info!("ctrl_c => shutdowning");
    Ok(())
}

// Make our own error that wraps `anyhow::Error`.
#[derive(Debug)]
pub struct AppError(anyhow::Error);

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let err = self.0;
        // Because `TraceLayer` wraps each request in a span that contains the request
        // method, uri, etc we don't need to include those details here
        tracing::error!(%err, "error");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Something went wrong: {}", &err)).into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl AppError {
    pub fn new<T: std::error::Error + Send + Sync + 'static>(err: T) -> Self {
        Self(anyhow!(err))
    }
}

fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
