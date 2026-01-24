//! # IO 工具模块
//!
//! 提供网络 IO 相关的工具函数和类型
//!
//! # 主要组件
//! - `create_dual_stack_listener`: 创建支持 IPv4/IPv6 双栈的监听器
//! - `TimeoutIO`: 为 IO 流添加空闲超时检测

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

/// 创建 IPv4/IPv6 双栈监听器
///
/// 创建一个同时支持 IPv4 和 IPv6 连接的 TCP 监听器
///
/// # 参数
/// - `port`: 监听端口
///
/// # 返回
/// - `Ok(TcpListener)`: 配置好的 TCP 监听器
/// - `Err(io::Error)`: 创建或配置失败
///
/// # 配置说明
/// - 绑定到 `[::]` (所有 IPv6 地址)
/// - 设置 `IPV6_V6ONLY=false`，支持 IPv4 映射
/// - 监听队列大小 (backlog): 1024
/// - 非阻塞模式
/// - 非 Windows 平台启用 `SO_REUSEADDR`
///
/// # 示例
///
/// ```no_run
/// use axum_bootstrap::util::io::create_dual_stack_listener;
///
/// #[tokio::main]
/// async fn main() {
///     let listener = create_dual_stack_listener(8080).await.unwrap();
/// }
/// ```
pub(crate) async fn create_dual_stack_listener(port: u16) -> io::Result<TcpListener> {
    // 创建一个 IPv6 的 socket
    let domain = Domain::IPV6;
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    #[cfg(not(windows))]
    socket.set_reuse_address(true)?; // 设置 reuse_address 以支持快速重启

    // 支持 IPv4 + IPv6 双栈
    socket.set_only_v6(false)?;

    // 绑定 socket 到地址和端口
    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port));
    socket.bind(&addr.into())?;
    socket.listen(1024)?; // 监听，1024 为 backlog 的大小

    // 将 socket2::Socket 转换为 std::net::TcpListener
    let std_listener = std::net::TcpListener::from(socket);
    std_listener.set_nonblocking(true)?;

    TcpListener::from_std(std_listener)
}

use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use pin_project_lite::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{sleep, Instant, Sleep},
};

pin_project! {
    /// 带超时检测的 IO 包装器
    ///
    /// 为底层 IO 流添加空闲超时检测功能。当读写操作在指定时间内
    /// 没有任何数据传输时，会返回 `TimedOut` 错误。
    ///
    /// # 泛型参数
    /// - `T`: 底层 IO 流类型，必须实现 `AsyncRead` 和 `AsyncWrite`
    ///
    /// # 字段
    /// - `inner`: 底层 IO 流
    /// - `timeout`: 超时时长
    /// - `idle_future`: 空闲计时器
    ///
    /// # 行为说明
    /// - 每次成功的读写操作都会重置计时器
    /// - 当操作挂起（Pending）且计时器到期时，返回超时错误
    /// - 超时仅在操作挂起时生效，不会中断正在进行的 IO
    #[derive(Debug)]
    pub struct TimeoutIO<T>
    where
    T: AsyncWrite,
    T: AsyncRead,
    {
        #[pin]
        inner: T,
        timeout:Duration,
        #[pin]
        idle_future:Sleep
    }
}

impl<T> TimeoutIO<T>
where
    T: AsyncWrite + AsyncRead,
{
    /// 创建新的 TimeoutIO 包装器
    ///
    /// # 参数
    /// - `inner`: 底层 IO 流
    /// - `timeout`: 超时时长
    ///
    /// # 返回
    /// 包装后的 TimeoutIO 实例
    ///
    /// # 示例
    ///
    /// ```no_run
    /// use axum_bootstrap::util::io::TimeoutIO;
    /// use tokio::net::TcpStream;
    /// use std::time::Duration;
    ///
    /// async fn example(stream: TcpStream) {
    ///     let timeout_stream = TimeoutIO::new(stream, Duration::from_secs(30));
    /// }
    /// ```
    pub fn new(inner: T, timeout: Duration) -> Self {
        Self {
            inner,
            timeout,
            idle_future: sleep(timeout),
        }
    }

    /// 设置新的超时时长
    ///
    /// # 参数
    /// - `timeout`: 新的超时时长
    ///
    /// # 说明
    /// 此方法会立即重置计时器
    pub fn _set_timeout_pinned(mut self: Pin<&mut Self>, timeout: Duration) {
        *self.as_mut().project().timeout = timeout;
        self.project().idle_future.as_mut().reset(Instant::now() + timeout);
    }
}

/// 实现 AsyncRead trait，添加超时检测
///
/// # 行为
/// - 如果读操作立即完成（Ready），重置计时器并返回结果
/// - 如果读操作挂起（Pending）且计时器到期，返回超时错误
/// - 否则继续等待读操作完成
impl<T> AsyncRead for TimeoutIO<T>
where
    T: AsyncWrite + AsyncRead,
{
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> Poll<Result<(), std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let read_poll = pro.inner.poll_read(cx, buf);

        if read_poll.is_ready() {
            // 读到内容或者读到 EOF 等等，重置计时
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            // 没有读到内容，且已经 timeout，则返回错误
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, format!("read idle for {timeout:?}"))));
        }
        read_poll
    }
}

/// 实现 AsyncWrite trait，添加超时检测
///
/// # 行为
/// 所有写相关操作（write, flush, shutdown, write_vectored）都会：
/// - 如果操作立即完成（Ready），重置计时器并返回结果
/// - 如果操作挂起（Pending）且计时器到期，返回超时错误
/// - 否则继续等待操作完成
impl<T> AsyncWrite for TimeoutIO<T>
where
    T: AsyncWrite + AsyncRead,
{
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_write(cx, buf);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, format!("write idle for {timeout:?}"))));
        }
        write_poll
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_flush(cx);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, format!("write idle for {timeout:?}"))));
        }
        write_poll
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_shutdown(cx);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, format!("write idle for {timeout:?}"))));
        }
        write_poll
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[std::io::IoSlice<'_>]) -> Poll<Result<usize, std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_write_vectored(cx, bufs);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, format!("write idle for {timeout:?}"))));
        }
        write_poll
    }
}
