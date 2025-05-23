use std::{fs::File, io, net::SocketAddr, sync::Arc};

use crate::DynError;
pub fn tls_config(key: &String, cert: &String) -> Result<Arc<ServerConfig>, DynError> {
    use std::io::{self, BufReader};
    let key_file = File::open(key).map_err(|_| "open private key failed")?;
    let cert_file = File::open(cert).map_err(|_| "open cert failed")?;
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file)).collect::<io::Result<Vec<rustls_pki_types::CertificateDer<'static>>>>()?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))?.ok_or("can not find any pem in key file")?;

    let mut config = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;
    config.alpn_protocols = vec![
        b"h2".to_vec(),       // http2
        b"http/1.1".to_vec(), // http1.1
    ];
    Ok(Arc::new(config))
}

#[allow(dead_code)]
pub fn rust_tls_acceptor(key: &String, cert: &String) -> Result<tokio_rustls::TlsAcceptor, DynError> {
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

/// A TLS acceptor that can be used with hyper servers.
pub struct TlsAcceptor<L = TcpListener> {
    config: Arc<ServerConfig>,
    listener: L,
}

/// An Acceptor for the `https` scheme.
impl TlsAcceptor {
    /// Creates a new `TlsAcceptor` from a `ServerConfig` and a `TcpListener`.
    pub fn new(config: Arc<ServerConfig>, listener: TcpListener) -> Self {
        Self { config, listener }
    }
    /// Replaces the Tls Acceptor configuration, which will be used for new connections.
    ///
    /// This can be used to change the certificate used at runtime.
    pub fn replace_config(&mut self, new_config: Arc<ServerConfig>) {
        self.config = new_config;
    }

    /// Accepts a new connection.
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

/// A TLS stream constructed by a [`TlsAcceptor`].
// tokio_rustls::server::TlsStream doesn't expose constructor methods,
// so we have to TlsAcceptor::accept and handshake to have access to it
// TlsStream implements AsyncRead/AsyncWrite by handshaking with tokio_rustls::Accept first
pub struct TlsStream<C = TcpStream> {
    state: State<C>,
}

impl<C: AsyncRead + AsyncWrite + Unpin> TlsStream<C> {
    fn new(stream: C, config: Arc<ServerConfig>) -> Self {
        let accept = tokio_rustls::TlsAcceptor::from(config).accept(stream);
        Self {
            state: State::Handshaking(accept),
        }
    }
    /// Returns a reference to the underlying IO stream.
    ///
    /// This should always return `Some`, except if an error has already been yielded.
    pub fn _io(&self) -> Option<&C> {
        match &self.state {
            State::Handshaking(accept) => accept.get_ref(),
            State::Streaming(stream) => Some(stream.get_ref().0),
        }
    }

    /// Returns a reference to the underlying [`rustls::ServerConnection'].
    ///
    /// This will start yielding `Some` only after the handshake has completed.
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

enum State<C> {
    Handshaking(tokio_rustls::Accept<C>),
    Streaming(tokio_rustls::server::TlsStream<C>),
}
