use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

pub mod error;
pub mod init_log;
pub mod util;
type DynError = Box<dyn std::error::Error + Send + Sync>;
use crate::util::{
    io::{self, create_dual_stack_listener},
    tls::{tls_config, TlsAcceptor},
};

use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    Router,
};

use hyper::body::Incoming;
use hyper_util::rt::TokioExecutor;
use log::{info, warn};
use tokio::{
    sync::{broadcast, mpsc},
    time,
};
use tokio_rustls::rustls::ServerConfig;
use tower::{Service, ServiceExt};
use util::format::SocketAddrFormat;

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Server<I: ReqInterceptor = DummyInterceptor> {
    pub port: u16,
    pub tls_param: Option<TlsParam>,
    router: Router,
    pub interceptor: Option<I>,
    pub idle_timeout: Duration,
    shutdown_rx: mpsc::Receiver<()>,
}

#[derive(Debug, Clone)]
pub struct TlsParam {
    pub tls: bool,
    pub cert: String,
    pub key: String,
}

pub enum InterceptResult<T: IntoResponse> {
    Return(Response),
    Drop,
    Continue(Request<Incoming>),
    Error(T),
}

pub trait ReqInterceptor: Send {
    type Error: IntoResponse + Send + Sync + 'static;
    fn intercept(&self, req: Request<Incoming>, ip: SocketAddr) -> impl std::future::Future<Output = InterceptResult<Self::Error>> + Send;
}

#[derive(Clone)]
pub struct DummyInterceptor;

impl ReqInterceptor for DummyInterceptor {
    type Error = error::AppError;

    async fn intercept(&self, req: Request<Incoming>, _ip: SocketAddr) -> InterceptResult<Self::Error> {
        InterceptResult::Continue(req)
    }
}

pub type DefaultServer = Server<DummyInterceptor>;

pub fn new_server(port: u16, router: Router) -> (Server, mpsc::Sender<()>) {
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let server = Server {
        port,
        tls_param: None, // No TLS by default
        router,
        interceptor: None,
        idle_timeout: Duration::from_secs(120),
        shutdown_rx,
    };
    (server, shutdown_tx)
}

impl<I> Server<I>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    pub fn with_interceptor<R>(self: Server<I>, interceptor: R) -> Server<R>
    where
        R: ReqInterceptor + Clone + Send + Sync + 'static,
    {
        Server::<R> {
            port: self.port,
            tls_param: self.tls_param,
            router: self.router,
            interceptor: Some(interceptor),
            idle_timeout: self.idle_timeout, // keep the same idle timeout
            shutdown_rx: self.shutdown_rx,
        }
    }
    pub fn with_tls_param(mut self, tls_param: Option<TlsParam>) -> Self {
        // Enable TLS by setting the tls_param
        self.tls_param = tls_param;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    pub async fn run(mut self) -> Result<(), std::io::Error> {
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
                    &mut self.shutdown_rx,
                )
                .await?
            }
            false => {
                serve_plantext(&self.router, server, graceful, self.port, self.interceptor.clone(), self.idle_timeout, &mut self.shutdown_rx).await?
            }
        }
        Ok(())
    }
}

async fn handle<I>(
    request: Request<Incoming>, client_socket_addr: SocketAddr, app: axum::middleware::AddExtension<Router, axum::extract::ConnectInfo<SocketAddr>>,
    interceptor: Option<I>,
) -> std::result::Result<Response, std::io::Error>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    if let Some(interceptor) = interceptor {
        match interceptor.intercept(request, client_socket_addr).await {
            InterceptResult::Return(res) => Ok(res),
            InterceptResult::Drop => Err(std::io::Error::other("Request dropped by interceptor")),
            InterceptResult::Continue(req) => app
                .oneshot(req)
                .await
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Interrupted, err)),
            InterceptResult::Error(err) => {
                let res = err.into_response();
                Ok(res)
            }
        }
    } else {
        app.oneshot(request)
            .await
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Interrupted, err))
    }
}

async fn handle_connection<C, I>(
    conn: C, client_socket_addr: std::net::SocketAddr, app: Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>,
    interceptor: Option<I>, graceful: &hyper_util::server::graceful::GracefulShutdown, timeout: Duration,
) where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static + Send + Sync,
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let timeout_io = Box::pin(io::TimeoutIO::new(conn, timeout));
    use hyper::Request;
    use hyper_util::rt::TokioIo;
    let stream = TokioIo::new(timeout_io);
    let mut app = app.into_make_service_with_connect_info::<SocketAddr>();
    let app: axum::middleware::AddExtension<Router, axum::extract::ConnectInfo<SocketAddr>> = unwrap_infallible(app.call(client_socket_addr).await);
    // https://github.com/tokio-rs/axum/blob/main/examples/serve-with-hyper/src/main.rs#L81
    let hyper_service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        handle(request, client_socket_addr, app.clone(), interceptor.clone())
    });

    let conn = server.serve_connection_with_upgrades(stream, hyper_service);
    let conn = graceful.watch(conn.into_owned());

    tokio::spawn(async move {
        if let Err(err) = conn.await {
            handle_hyper_error(client_socket_addr, err);
        }
        log::debug!("connection dropped: {client_socket_addr}");
    });
}

fn handle_hyper_error(client_socket_addr: SocketAddr, http_err: DynError) {
    use std::error::Error;
    match http_err.downcast_ref::<hyper::Error>() {
        Some(hyper_err) => {
            let level = if hyper_err.is_user() { log::Level::Warn } else { log::Level::Debug };
            let source = hyper_err.source().unwrap_or(hyper_err);
            log::log!(
                level,
                "[hyper {}]: {:?} from {}",
                if hyper_err.is_user() { "user" } else { "system" },
                source,
                SocketAddrFormat(&client_socket_addr)
            );
        }
        None => match http_err.downcast_ref::<std::io::Error>() {
            Some(io_err) => {
                warn!("[hyper io]: [{}] {} from {}", io_err.kind(), io_err, SocketAddrFormat(&client_socket_addr));
            }
            None => {
                warn!("[hyper]: {} from {}", http_err, SocketAddrFormat(&client_socket_addr));
            }
        },
    }
}

async fn serve_plantext<I>(
    app: &Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>, graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16, interceptor: Option<I>, timeout: Duration, shutdown_rx: &mut mpsc::Receiver<()>,
) -> Result<(), std::io::Error>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let listener = create_dual_stack_listener(port).await?;
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("start graceful shutdown!");
                drop(listener);
                break;
            }
            conn = listener.accept() => {
                match conn {
                    Ok((conn, client_socket_addr)) => {
                        handle_connection(conn,client_socket_addr, app.clone(), server.clone(),interceptor.clone(), &graceful, timeout).await;}
                    Err(e) => {
                        warn!("accept error:{e}");
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

#[allow(clippy::too_many_arguments)]
async fn serve_tls<I>(
    app: &Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>, graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16, tls_param: &TlsParam, interceptor: Option<I>, timeout: Duration, shutdown_rx: &mut mpsc::Receiver<()>,
) -> Result<(), std::io::Error>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let (tx, _rx) = broadcast::channel::<Arc<ServerConfig>>(10);
    let tx_clone = tx.clone();
    let tls_param_clone = tls_param.clone();
    tokio::spawn(async move {
        info!("update tls config every {REFRESH_INTERVAL:?}");
        loop {
            time::sleep(REFRESH_INTERVAL).await;
            if let Ok(new_acceptor) = tls_config(&tls_param_clone.key, &tls_param_clone.cert) {
                info!("update tls config");
                if let Err(e) = tx.send(new_acceptor) {
                    warn!("send tls config error:{e}");
                }
            }
        }
    });
    let mut rx = tx_clone.subscribe();
    let mut acceptor: TlsAcceptor = TlsAcceptor::new(tls_config(&tls_param.key, &tls_param.cert)?, create_dual_stack_listener(port).await?);
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
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
                        handle_connection(conn,client_socket_addr, app.clone(), server.clone(),interceptor.clone(), &graceful, timeout).await;}
                    Err(e) => {
                        warn!("accept error:{e}");
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
pub async fn wait_signal() -> Result<(), DynError> {
    use log::info;
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate_signal = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = terminate_signal.recv() => {
            info!("receive terminate signal");
        },
        _ = tokio::signal::ctrl_c() => {
            info!("receive ctrl_c signal");
        },
    };
    Ok(())
}

#[cfg(windows)]
pub async fn wait_signal() -> Result<(), DynError> {
    let _ = tokio::signal::ctrl_c().await;
    info!("receive ctrl_c signal");
    Ok(())
}

fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
