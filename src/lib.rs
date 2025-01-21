use std::{sync::Arc, time::Duration};

pub mod util;
type DynError = Box<dyn std::error::Error + Send + Sync>;
use crate::util::{
    io::{self, create_dual_stack_listener},
    tls::{tls_config, TlsAcceptor},
};
use axum::Router;
use hyper_util::rt::TokioExecutor;
use log::{info, warn};
use tokio::{pin, sync::broadcast, time};
use tokio_rustls::rustls::ServerConfig;
use tower_service::Service;

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct TlsParam {
    pub tls: bool,
    pub cert: String,
    pub key: String,
}

pub async fn axum_serve(
    router: Router,
    port: u16,
    tls_param: Option<TlsParam>,
) -> Result<(), DynError> {
    let use_tls = match tls_param.clone() {
        Some(config) => config.tls,
        None => false,
    };
    log::info!("listening on port {port}, use_tls: {use_tls}",);
    let server: hyper_util::server::conn::auto::Builder<TokioExecutor> =
        hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let graceful: hyper_util::server::graceful::GracefulShutdown =
        hyper_util::server::graceful::GracefulShutdown::new();
    match use_tls {
        #[allow(clippy::expect_used)]
        true => {
            serve_tls(
                &router,
                server,
                graceful,
                port,
                tls_param.expect("should be some"),
            )
            .await?
        }
        false => serve_plantext(&router, server, graceful, port).await?,
    }
    Ok(())
}

macro_rules! handle_connection {
    ($conn:expr, $app:expr, $server:expr, $graceful:expr) => {
        match $conn {
            Ok((conn, client_socket_addr)) => {
                let tower_service = $app.clone();
                let timeout_io = Box::pin(io::TimeoutIO::new(conn, Duration::from_secs(120)));
                use hyper::Request;
                use hyper_util::rt::TokioIo;
                let stream = TokioIo::new(timeout_io);
                let hyper_service =
                    hyper::service::service_fn(move |request: Request<Incoming>| {
                        tower_service.clone().call(request)
                    });

                let conn = $server.serve_connection_with_upgrades(stream, hyper_service);
                let conn = $graceful.watch(conn.into_owned());

                tokio::spawn(async move {
                    if let Err(err) = conn.await {
                        info!("connection error: {}", err);
                    }
                    log::debug!("connection dropped: {}", client_socket_addr);
                });
            }
            Err(err) => {
                warn!("Error accepting connection: {}", err);
            }
        }
    };
}

async fn serve_plantext(
    app: &Router,
    server: hyper_util::server::conn::auto::Builder<TokioExecutor>,
    graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16,
) -> Result<(), DynError> {
    use hyper::body::Incoming;
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
                handle_connection!(conn, app, server, graceful);
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

async fn serve_tls(
    app: &Router,
    server: hyper_util::server::conn::auto::Builder<TokioExecutor>,
    graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16,
    tls_param: TlsParam,
) -> Result<(), DynError> {
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
    use hyper::body::Incoming;
    let mut acceptor: TlsAcceptor = TlsAcceptor::new(
        tls_config(&tls_param.key, &tls_param.cert)?,
        create_dual_stack_listener(port).await?,
    );
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
                handle_connection!(conn, app, server, graceful);
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
