use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;
use tokio_rustls::rustls::{ServerConfig, ServerConnection};

pub(crate) async fn create_dual_stack_listener(port: u16) -> io::Result<TcpListener> {
    // 创建一个IPv6的socket
    let domain = Domain::IPV6;
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?; // 设置reuse_address以支持快速重启

    // 支持ipv4 + ipv6双栈
    socket.set_only_v6(false)?;
    // 绑定socket到地址和端口
    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port));
    socket.bind(&addr.into())?;
    socket.listen(1024)?; // 监听，1024为backlog的大小

    // 将socket2::Socket转换为std::net::TcpListener
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
    /// enhance inner tcp stream with prometheus counter
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
    pub fn new(inner: T, timeout: Duration) -> Self {
        Self {
            inner,
            timeout,
            idle_future: sleep(timeout),
        }
    }
    /// set timeout
    pub fn _set_timeout_pinned(mut self: Pin<&mut Self>, timeout: Duration) {
        *self.as_mut().project().timeout = timeout;
        self.project()
            .idle_future
            .as_mut()
            .reset(Instant::now() + timeout);
    }
}

impl<T> AsyncRead for TimeoutIO<T>
where
    T: AsyncWrite + AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let read_poll = pro.inner.poll_read(cx, buf);
        if read_poll.is_ready() {
            // 读到内容或者读到EOF等等,重置计时
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            // 没有读到内容，且已经timeout，则返回错误
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("read idle for {:?}", timeout),
            )));
        }
        read_poll
    }
}

impl<T> AsyncWrite for TimeoutIO<T>
where
    T: AsyncWrite + AsyncRead,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_write(cx, buf);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("write idle for {:?}", timeout),
            )));
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
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("write idle for {:?}", timeout),
            )));
        }
        write_poll
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_shutdown(cx);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("write idle for {:?}", timeout),
            )));
        }
        write_poll
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        let pro = self.project();
        let idle_feature = pro.idle_future;
        let timeout: &mut Duration = pro.timeout;
        let write_poll = pro.inner.poll_write_vectored(cx, bufs);
        if write_poll.is_ready() {
            idle_feature.reset(Instant::now() + *timeout);
        } else if idle_feature.poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("write idle for {:?}", timeout),
            )));
        }
        write_poll
    }
}
