//! # Axum Bootstrap 服务器核心模块
//!
//! 这个模块提供了基于 Axum 和 Hyper 的高性能 HTTP/HTTPS 服务器实现。
//! 主要特性包括:
//! - 支持 HTTP 和 HTTPS (TLS)
//! - 请求拦截机制
//! - 优雅关闭 (Graceful Shutdown)
//! - IPv4/IPv6 双栈支持
//! - 连接超时控制
//! - TLS 证书动态更新
//!
//! # 示例
//!
//! ```no_run
//! use axum::Router;
//! use axum_bootstrap::{new_server, generate_shutdown_receiver};
//!
//! #[tokio::main]
//! async fn main() {
//!     let router = Router::new();
//!     let shutdown_rx = generate_shutdown_receiver();
//!     let server = new_server(8080, router, shutdown_rx);
//!     server.run().await.unwrap();
//! }
//! ```

use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

/// 错误处理模块
pub mod error;
/// 日志初始化模块
pub mod init_log;
/// JWT 认证模块 (需要启用 jwt feature)
#[cfg(feature = "jwt")]
pub mod jwt;
/// 工具函数模块
pub mod util;

/// 动态错误类型别名
type DynError = Box<dyn std::error::Error + Send + Sync>;

use crate::util::{
    io::{self, create_dual_stack_listener},
    tls::{TlsAcceptor, tls_config},
};

use axum::{
    Router,
    extract::Request,
    response::{IntoResponse, Response},
};

use hyper::body::Incoming;
use hyper_util::rt::TokioExecutor;
use log::{info, warn};
use tokio::{
    sync::broadcast::{self, Receiver, Sender, error::RecvError},
    time,
};
use tokio_rustls::rustls::ServerConfig;
use tower::{Service, ServiceExt};
use util::format::SocketAddrFormat;

/// TLS 配置刷新间隔 (24小时)
const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);

/// 优雅关闭等待超时时间 (10秒)
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP/HTTPS 服务器核心结构
///
/// # 泛型参数
/// - `I`: 请求拦截器类型，必须实现 `ReqInterceptor` trait
///
/// # 字段
/// - `port`: 监听端口
/// - `tls_param`: TLS 配置参数 (可选)
/// - `router`: Axum 路由
/// - `interceptor`: 请求拦截器实例 (可选)
/// - `idle_timeout`: 连接空闲超时时间
/// - `shutdown_rx`: 关闭信号接收器
pub struct Server<I: ReqInterceptor = DummyInterceptor> {
    pub port: u16,
    pub tls_param: Option<TlsParam>,
    router: Router,
    pub interceptor: Option<I>,
    pub idle_timeout: Duration,
    shutdown_rx: broadcast::Receiver<()>,
}

/// TLS 配置参数
///
/// # 字段
/// - `tls`: 是否启用 TLS
/// - `cert`: TLS 证书文件路径
/// - `key`: TLS 私钥文件路径
#[derive(Debug, Clone)]
pub struct TlsParam {
    pub tls: bool,
    pub cert: String,
    pub key: String,
}

/// 请求拦截结果
///
/// 用于控制请求的处理流程
///
/// # 变体
/// - `Return(Response)`: 直接返回响应，不继续处理
/// - `Drop`: 丢弃请求，不返回响应
/// - `Continue(Request)`: 继续处理请求
/// - `Error(T)`: 返回错误响应
pub enum InterceptResult<T: IntoResponse> {
    Return(Response),
    Drop,
    Continue(Request<Incoming>),
    Error(T),
}

/// 请求拦截器 trait
///
/// 实现此 trait 可以在请求到达路由处理器之前进行拦截和处理
///
/// # 关联类型
/// - `Error`: 拦截器可能返回的错误类型
///
/// # 方法
/// - `intercept`: 拦截请求的方法
///
/// # 示例
///
/// ```no_run
/// use axum_bootstrap::{ReqInterceptor, InterceptResult};
/// use axum::extract::Request;
/// use hyper::body::Incoming;
/// use std::net::SocketAddr;
///
/// #[derive(Clone)]
/// struct MyInterceptor;
///
/// impl ReqInterceptor for MyInterceptor {
///     type Error = axum_bootstrap::error::AppError;
///
///     async fn intercept(&self, req: Request<Incoming>, ip: SocketAddr) -> InterceptResult<Self::Error> {
///         // 自定义拦截逻辑
///         InterceptResult::Continue(req)
///     }
/// }
/// ```
pub trait ReqInterceptor: Send {
    type Error: IntoResponse + Send + Sync + 'static;
    fn intercept(&self, req: Request<Incoming>, ip: SocketAddr) -> impl std::future::Future<Output = InterceptResult<Self::Error>> + Send;
}

/// 空实现的请求拦截器
///
/// 默认不执行任何拦截操作，直接继续处理请求
#[derive(Clone)]
pub struct DummyInterceptor;

impl ReqInterceptor for DummyInterceptor {
    type Error = error::AppError;

    async fn intercept(&self, req: Request<Incoming>, _ip: SocketAddr) -> InterceptResult<Self::Error> {
        InterceptResult::Continue(req)
    }
}

/// 默认服务器类型 (使用 DummyInterceptor)
pub type DefaultServer = Server<DummyInterceptor>;

/// 创建默认服务器实例
///
/// # 参数
/// - `port`: 监听端口
/// - `router`: Axum 路由
/// - `shutdown_rx`: 关闭信号接收器
///
/// # 返回
/// 返回配置好的服务器实例，默认不启用 TLS，空闲超时为 120 秒
///
/// # 示例
///
/// ```no_run
/// use axum::Router;
/// use axum_bootstrap::{new_server, generate_shutdown_receiver};
///
/// #[tokio::main]
/// async fn main() {
///     let router = Router::new();
///     let shutdown_rx = generate_shutdown_receiver();
///     let server = new_server(8080, router, shutdown_rx);
///     server.run().await.unwrap();
/// }
/// ```
pub fn new_server(port: u16, router: Router, shutdown_rx: broadcast::Receiver<()>) -> Server {
    Server {
        port,
        tls_param: None, // 默认不启用 TLS
        router,
        interceptor: None,
        idle_timeout: Duration::from_secs(120),
        shutdown_rx,
    }
}

impl<I> Server<I>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    /// 设置请求拦截器
    ///
    /// 用于将服务器的拦截器类型更改为新的类型
    ///
    /// # 类型参数
    /// - `R`: 新的拦截器类型
    ///
    /// # 参数
    /// - `interceptor`: 新的拦截器实例
    ///
    /// # 返回
    /// 返回配置了新拦截器的服务器实例
    pub fn with_interceptor<R>(self: Server<I>, interceptor: R) -> Server<R>
    where
        R: ReqInterceptor + Clone + Send + Sync + 'static,
    {
        Server::<R> {
            port: self.port,
            tls_param: self.tls_param,
            router: self.router,
            interceptor: Some(interceptor),
            idle_timeout: self.idle_timeout, // 保持相同的空闲超时
            shutdown_rx: self.shutdown_rx,
        }
    }

    /// 设置 TLS 参数
    ///
    /// # 参数
    /// - `tls_param`: TLS 配置参数，为 None 时禁用 TLS
    ///
    /// # 返回
    /// 返回配置了 TLS 的服务器实例
    pub fn with_tls_param(mut self, tls_param: Option<TlsParam>) -> Self {
        self.tls_param = tls_param;
        self
    }

    /// 设置连接空闲超时时间
    ///
    /// # 参数
    /// - `timeout`: 超时时长
    ///
    /// # 返回
    /// 返回配置了超时的服务器实例
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// 启动服务器
    ///
    /// 根据 TLS 配置启动 HTTP 或 HTTPS 服务器，并监听关闭信号
    ///
    /// # 返回
    /// - `Ok(())`: 服务器成功启动并正常关闭
    /// - `Err(std::io::Error)`: 启动或运行过程中出现 I/O 错误
    ///
    /// # 错误
    /// - 端口绑定失败
    /// - TLS 证书加载失败
    /// - 网络 I/O 错误
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

/// 处理单个 HTTP 请求
///
/// 如果配置了拦截器，会先调用拦截器处理请求，否则直接路由到应用
///
/// # 参数
/// - `request`: HTTP 请求
/// - `client_socket_addr`: 客户端地址
/// - `app`: Axum 应用实例
/// - `interceptor`: 可选的请求拦截器
///
/// # 返回
/// - `Ok(Response)`: 成功生成的 HTTP 响应
/// - `Err(std::io::Error)`: 处理过程中的 I/O 错误
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

/// 处理单个连接
///
/// 为每个连接创建超时包装器和 Hyper 服务，并在新的 tokio 任务中处理
///
/// # 类型参数
/// - `C`: 连接类型，必须实现 AsyncRead + AsyncWrite
/// - `I`: 请求拦截器类型
///
/// # 参数
/// - `conn`: 网络连接
/// - `client_socket_addr`: 客户端地址
/// - `app`: Axum 路由
/// - `server`: Hyper 服务器构建器
/// - `interceptor`: 可选的请求拦截器
/// - `graceful`: 优雅关闭句柄
/// - `timeout`: 连接空闲超时时间
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
        log::debug!("dropped: {client_socket_addr}");
    });
}

/// 处理 Hyper 错误并记录日志
///
/// 根据错误类型输出不同级别的日志
///
/// # 参数
/// - `client_socket_addr`: 客户端地址
/// - `http_err`: HTTP 错误
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

/// 启动纯文本 HTTP 服务器
///
/// 监听指定端口并处理 HTTP 连接，支持优雅关闭
///
/// # 参数
/// - `app`: Axum 路由
/// - `server`: Hyper 服务器构建器
/// - `graceful`: 优雅关闭句柄
/// - `port`: 监听端口
/// - `interceptor`: 可选的请求拦截器
/// - `timeout`: 连接空闲超时时间
/// - `shutdown_rx`: 关闭信号接收器
///
/// # 返回
/// - `Ok(())`: 服务器成功启动并正常关闭
/// - `Err(std::io::Error)`: 启动或运行过程中出现错误
async fn serve_plantext<I>(
    app: &Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>, graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16, interceptor: Option<I>, timeout: Duration, shutdown_rx: &mut broadcast::Receiver<()>,
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
    match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, graceful.shutdown()).await {
        Ok(_) => info!("Gracefully shutdown!"),
        Err(_) => info!("Waited {GRACEFUL_SHUTDOWN_TIMEOUT:?} for graceful shutdown, aborting..."),
    }
    Ok(())
}

/// 启动 TLS HTTPS 服务器
///
/// 监听指定端口并处理 HTTPS 连接，支持 TLS 证书动态更新和优雅关闭
///
/// # 参数
/// - `app`: Axum 路由
/// - `server`: Hyper 服务器构建器
/// - `graceful`: 优雅关闭句柄
/// - `port`: 监听端口
/// - `tls_param`: TLS 配置参数
/// - `interceptor`: 可选的请求拦截器
/// - `timeout`: 连接空闲超时时间
/// - `shutdown_rx`: 关闭信号接收器
///
/// # 返回
/// - `Ok(())`: 服务器成功启动并正常关闭
/// - `Err(std::io::Error)`: 启动或运行过程中出现错误
///
/// # 说明
/// 服务器会在后台启动一个定时任务，每隔 REFRESH_INTERVAL (24小时) 刷新一次 TLS 配置
#[allow(clippy::too_many_arguments)]
async fn serve_tls<I>(
    app: &Router, server: hyper_util::server::conn::auto::Builder<TokioExecutor>, graceful: hyper_util::server::graceful::GracefulShutdown,
    port: u16, tls_param: &TlsParam, interceptor: Option<I>, timeout: Duration, shutdown_rx: &mut broadcast::Receiver<()>,
) -> Result<(), std::io::Error>
where
    I: ReqInterceptor + Clone + Send + Sync + 'static,
{
    let (tx, mut rx) = broadcast::channel::<Arc<ServerConfig>>(1);
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
    let mut acceptor: TlsAcceptor = TlsAcceptor::new(tls_config(&tls_param.key, &tls_param.cert)?, create_dual_stack_listener(port).await?);
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("start graceful shutdown!");
                drop(acceptor);
                break;
            }
            message = rx.recv() => {
                match message {
                    Ok(new_config) => {
                        acceptor.replace_config(new_config);
                        info!("replaced tls config");
                    },
                    Err(e) => {
                        match e {
                            RecvError::Closed => {
                                warn!("this channel should not be closed!");
                                break;
                            },
                            RecvError::Lagged(n) => {
                                warn!("lagged {n} messages, this may cause tls config not updated in time");
                            }
                        }
                    }
                }
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
    match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, graceful.shutdown()).await {
        Ok(_) => info!("Gracefully shutdown!"),
        Err(_) => info!("Waited {GRACEFUL_SHUTDOWN_TIMEOUT:?} for graceful shutdown, aborting..."),
    }
    Ok(())
}

/// 生成关闭信号接收器
///
/// 创建一个广播通道并订阅系统信号，返回接收器用于监听关闭信号
///
/// # 返回
/// 关闭信号接收器，当收到 SIGTERM 或 Ctrl+C 信号时会收到通知
///
/// # 示例
///
/// ```no_run
/// use axum_bootstrap::generate_shutdown_receiver;
///
/// let shutdown_rx = generate_shutdown_receiver();
/// // 传递给服务器
/// ```
pub fn generate_shutdown_receiver() -> Receiver<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    subscribe_shutdown_sender(shutdown_tx);
    shutdown_rx
}

/// 订阅关闭信号发送器
///
/// 在后台任务中监听系统信号，当收到信号时通过发送器通知所有接收器
///
/// # 参数
/// - `shutdown_tx`: 关闭信号发送器
pub fn subscribe_shutdown_sender(shutdown_tx: Sender<()>) {
    tokio::spawn(async move {
        match wait_signal().await {
            Ok(_) => {
                let _ = shutdown_tx.send(());
            }
            Err(e) => {
                log::error!("wait_signal error: {}", e);
                panic!("wait_signal error: {}", e);
            }
        }
    });
}

/// 等待系统关闭信号 (Unix 平台)
///
/// 监听 SIGTERM 和 Ctrl+C 信号
///
/// # 返回
/// - `Ok(())`: 成功接收到信号
/// - `Err(DynError)`: 信号处理出错
#[cfg(unix)]
pub(crate) async fn wait_signal() -> Result<(), DynError> {
    use log::info;
    use tokio::signal::unix::{SignalKind, signal};
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

/// 等待系统关闭信号 (Windows 平台)
///
/// 监听 Ctrl+C 信号
///
/// # 返回
/// - `Ok(())`: 成功接收到信号
/// - `Err(DynError)`: 信号处理出错
#[cfg(windows)]
pub(crate) async fn wait_signal() -> Result<(), DynError> {
    let _ = tokio::signal::ctrl_c().await;
    info!("receive ctrl_c signal");
    Ok(())
}

/// 解包 Infallible 结果类型
///
/// 因为 Infallible 类型永远不会发生，所以这个函数总是返回 Ok 值
///
/// # 参数
/// - `result`: 包含 Infallible 错误的 Result
///
/// # 返回
/// Result 中的成功值
fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
