//! # TLS 配置模块
//!
//! 提供 TLS/HTTPS 服务器配置和连接处理功能
//!
//! # 主要组件
//! - `tls_config`: 从证书文件创建 TLS 配置
//! - `TlsAcceptor`: TLS 连接接受器，支持动态配置更新
//! - `TlsStream`: TLS 流处理，自动处理握手和数据传输
//!
//! # 支持的协议
//! - TLS 1.2, TLS 1.3 (通过 rustls)
//! - HTTP/2 (h2)
//! - HTTP/1.1

use std::{io, net::SocketAddr, sync::Arc};

/// 从证书和私钥文件创建 TLS 服务器配置
///
/// # 参数
/// - `key`: 私钥文件路径
/// - `cert`: 证书文件路径（PEM 格式）
///
/// # 返回
/// - `Ok(Arc<ServerConfig>)`: 配置好的 TLS 服务器配置
/// - `Err(std::io::Error)`: 文件读取或解析失败
///
/// # 支持的 ALPN 协议
/// - HTTP/2 (h2)
/// - HTTP/1.1
///
/// # 示例
///
/// ```no_run
/// use axum_bootstrap::util::tls::tls_config;
///
/// let config = tls_config(
///     &"privkey.pem".to_string(),
///     &"cert.pem".to_string()
/// ).unwrap();
/// ```
pub fn tls_config(key: &String, cert: &String) -> Result<Arc<ServerConfig>, std::io::Error> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};

    // 安装默认加密提供者 (如果尚未设置)
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let certs = CertificateDer::pem_file_iter(cert)
        .map_err(|_| io::Error::other("open cert failed"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| io::Error::other("invalid cert pem"))?;

    let key = PrivateKeyDer::from_pem_file(key).map_err(|_| io::Error::other("failed to read private key"))?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(std::io::Error::other)?;
    config.alpn_protocols = vec![
        b"h2".to_vec(),       // HTTP/2
        b"http/1.1".to_vec(), // HTTP/1.1
    ];
    Ok(Arc::new(config))
}

/// 从证书和私钥文件创建 tokio_rustls TlsAcceptor
///
/// 这是 `tls_config` 的便捷包装器
///
/// # 参数
/// - `key`: 私钥文件路径
/// - `cert`: 证书文件路径
///
/// # 返回
/// - `Ok(tokio_rustls::TlsAcceptor)`: TLS 接受器
/// - `Err(std::io::Error)`: 创建失败
#[allow(dead_code)]
pub fn rust_tls_acceptor(key: &String, cert: &String) -> Result<tokio_rustls::TlsAcceptor, std::io::Error> {
    Ok(tls_config(key, cert)?.into())
}

use core::task::{Context, Poll};
use std::future::Future;
use std::pin::Pin;

use futures_util::ready;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::rustls::{ServerConfig, ServerConnection};

/// TLS 连接接受器
///
/// 用于 Hyper 服务器的 TLS 接受器，支持动态配置更新
///
/// # 泛型参数
/// - `L`: 监听器类型，默认为 `TcpListener`
///
/// # 字段
/// - `config`: TLS 服务器配置
/// - `listener`: TCP 监听器
pub struct TlsAcceptor<L = TcpListener> {
    config: Arc<ServerConfig>,
    listener: L,
}

impl TlsAcceptor {
    /// 创建新的 TLS 接受器
    ///
    /// # 参数
    /// - `config`: TLS 服务器配置
    /// - `listener`: TCP 监听器
    ///
    /// # 返回
    /// 新创建的 TlsAcceptor 实例
    pub fn new(config: Arc<ServerConfig>, listener: TcpListener) -> Self {
        Self { config, listener }
    }

    /// 替换 TLS 配置
    ///
    /// 新配置将用于后续建立的所有新连接，已有连接不受影响
    ///
    /// # 参数
    /// - `new_config`: 新的 TLS 配置
    ///
    /// # 使用场景
    /// 可用于运行时更新证书，无需重启服务器
    pub fn replace_config(&mut self, new_config: Arc<ServerConfig>) {
        self.config = new_config;
    }

    /// 接受新的 TLS 连接
    ///
    /// # 返回
    /// - `Ok((TlsStream, SocketAddr))`: 成功接受连接，返回 TLS 流和客户端地址
    /// - `Err(io::Error)`: 接受连接失败
    pub async fn accept(&mut self) -> Result<(TlsStream, SocketAddr), io::Error> {
        let (sock, addr) = self.listener.accept().await?;
        Ok((TlsStream::new(sock, self.config.clone()), addr))
    }
}

impl<C, L> From<(C, L)> for TlsAcceptor
where
    C: Into<Arc<ServerConfig>>,
    L: Into<TcpListener>,
{
    fn from((config, listener): (C, L)) -> Self {
        Self::new(config.into(), listener.into())
    }
}

/// TLS 流处理器
///
/// 由 [`TlsAcceptor`] 创建的 TLS 流，自动处理握手和数据传输
///
/// # 状态机制
/// TlsStream 内部维护两个状态：
/// - `Handshaking`: TLS 握手进行中
/// - `Streaming`: 握手完成，正常数据传输
///
/// # 实现说明
/// tokio_rustls::server::TlsStream 不公开构造方法，
/// 因此我们通过 TlsAcceptor::accept 和握手来访问它。
/// TlsStream 实现 AsyncRead/AsyncWrite，首次操作时自动完成握手。
///
/// # 泛型参数
/// - `C`: 底层连接类型，默认为 `TcpStream`
pub struct TlsStream<C = TcpStream> {
    state: State<C>,
}

impl<C: AsyncRead + AsyncWrite + Unpin> TlsStream<C> {
    /// 创建新的 TLS 流
    ///
    /// # 参数
    /// - `stream`: 底层 TCP 流
    /// - `config`: TLS 配置
    fn new(stream: C, config: Arc<ServerConfig>) -> Self {
        let accept = tokio_rustls::TlsAcceptor::from(config).accept(stream);
        Self {
            state: State::Handshaking(accept),
        }
    }

    /// 获取底层 IO 流的引用
    ///
    /// # 返回
    /// - `Some(&C)`: 底层流的引用
    /// - `None`: 已发生错误
    ///
    /// # 说明
    /// 通常总是返回 Some，除非已经产生过错误
    pub fn _io(&self) -> Option<&C> {
        match &self.state {
            State::Handshaking(accept) => accept.get_ref(),
            State::Streaming(stream) => Some(stream.get_ref().0),
        }
    }

    /// 获取底层 rustls ServerConnection 的引用
    ///
    /// # 返回
    /// - `Some(&ServerConnection)`: 握手完成后的 TLS 连接
    /// - `None`: 握手尚未完成
    pub fn _connection(&self) -> Option<&ServerConnection> {
        match &self.state {
            State::Handshaking(_) => None,
            State::Streaming(stream) => Some(stream.get_ref().1),
        }
    }
}

impl<C: AsyncRead + AsyncWrite + Unpin> AsyncRead for TlsStream<C> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context, buf: &mut ReadBuf) -> Poll<io::Result<()>> {
        let pin = self.get_mut();
        let accept = match &mut pin.state {
            State::Handshaking(accept) => accept,
            State::Streaming(stream) => return Pin::new(stream).poll_read(cx, buf),
        };

        let mut stream = match ready!(Pin::new(accept).poll(cx)) {
            Ok(stream) => stream,
            Err(err) => return Poll::Ready(Err(err)),
        };

        let result = Pin::new(&mut stream).poll_read(cx, buf);
        pin.state = State::Streaming(stream);
        result
    }
}

impl<C: AsyncRead + AsyncWrite + Unpin> AsyncWrite for TlsStream<C> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let pin = self.get_mut();
        let accept = match &mut pin.state {
            State::Handshaking(accept) => accept,
            State::Streaming(stream) => return Pin::new(stream).poll_write(cx, buf),
        };

        let mut stream = match ready!(Pin::new(accept).poll(cx)) {
            Ok(stream) => stream,
            Err(err) => return Poll::Ready(Err(err)),
        };

        let result = Pin::new(&mut stream).poll_write(cx, buf);
        pin.state = State::Streaming(stream);
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.state {
            State::Handshaking(_) => Poll::Ready(Ok(())),
            State::Streaming(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.state {
            State::Handshaking(_) => Poll::Ready(Ok(())),
            State::Streaming(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// TLS 流的内部状态
///
/// # 变体
/// - `Handshaking`: TLS 握手进行中
/// - `Streaming`: 握手完成，进入数据传输状态
enum State<C> {
    Handshaking(tokio_rustls::Accept<C>),
    Streaming(tokio_rustls::server::TlsStream<C>),
}
