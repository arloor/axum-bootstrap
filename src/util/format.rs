pub struct SocketAddrFormat<'a>(pub &'a std::net::SocketAddr);

impl std::fmt::Display for SocketAddrFormat<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.0.ip().to_canonical(), self.0.port())
    }
}
