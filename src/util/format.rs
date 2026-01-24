//! # 格式化工具模块
//!
//! 提供各种数据类型的格式化工具

/// Socket 地址格式化包装器
///
/// 将 `SocketAddr` 格式化为规范的 IP 地址和端口格式
///
/// # 格式说明
/// - IPv4: `192.168.1.1 8080`
/// - IPv6: `::1 8080` (使用 to_canonical 转换)
///
/// # 示例
///
/// ```
/// use axum_bootstrap::util::format::SocketAddrFormat;
/// use std::net::SocketAddr;
///
/// let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
/// println!("{}", SocketAddrFormat(&addr));
/// // 输出: 127.0.0.1 8080
/// ```
pub struct SocketAddrFormat<'a>(pub &'a std::net::SocketAddr);

impl std::fmt::Display for SocketAddrFormat<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.0.ip().to_canonical(), self.0.port())
    }
}
