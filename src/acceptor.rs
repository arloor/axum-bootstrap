use std::{fs::File, io, net::SocketAddr, sync::Arc};

use crate::DynError;
pub fn tls_config(key: &String, cert: &String) -> Result<Arc<ServerConfig>, DynError> {
    use std::io::{self, BufReader};
    let key_file = File::open(key).map_err(|_| "open private key failed")?;
    let cert_file = File::open(cert).map_err(|_| "open cert failed")?;
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<io::Result<Vec<rustls_pki_types::CertificateDer<'static>>>>()?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))?
        .ok_or("can not find any pem in key file")?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    config.alpn_protocols = vec![
        b"h2".to_vec(),       // http2
        b"http/1.1".to_vec(), // http1.1
    ];
    Ok(Arc::new(config))
}

pub fn rust_tls_acceptor(
    key: &String,
    cert: &String,
) -> Result<tokio_rustls::TlsAcceptor, DynError> {
    Ok(tls_config(key, cert)?.into())
}

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;
use tokio_rustls::rustls::ServerConfig;

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
