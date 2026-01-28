//! # HTTP 请求提取器模块
//!
//! 提供各种自定义的 Axum extractor，用于从 HTTP 请求中提取常用信息
//!
//! # 示例
//!
//! ```no_run
//! use axum::{Router, routing::get};
//! use axum_bootstrap::util::extractor::Host;
//!
//! async fn handler(Host(host): Host) -> String {
//!     format!("Request host: {}", host)
//! }
//!
//! let app = Router::new().route("/", get(handler));
//! ```

use axum::{extract::FromRequestParts, http::request::Parts};
use futures_util::io;

use crate::error::AppError;

/// Host extractor
///
/// 从 HTTP 请求中提取 Host 信息，兼容 HTTP/1.x 和 HTTP/2
///
/// # 工作原理
///
/// - **HTTP/1.x**: 从 `Host` header 中读取
/// - **HTTP/2**: 优先从 `:authority` pseudo-header 读取，回退到 `Host` header
///
/// # 示例
///
/// ```no_run
/// use axum::{Router, routing::get};
/// use axum_bootstrap::util::extractor::Host;
///
/// async fn show_host(Host(host): Host) -> String {
///     format!("Your host is: {}", host)
/// }
///
/// let app = Router::new().route("/", get(show_host));
/// ```
///
/// # 错误处理
///
/// 如果请求中没有 Host 信息，将返回 500 错误
///
/// # 可选 Host 提取
///
/// 如果你希望 Host 是可选的（不存在时不报错），可以使用 `Option<Host>`：
///
/// ```no_run
/// use axum::{Router, routing::get};
/// use axum_bootstrap::util::extractor::Host;
///
/// async fn show_host(host: Option<Host>) -> String {
///     match host {
///         Some(Host(h)) => format!("Your host is: {}", h),
///         None => "No host provided".to_string(),
///     }
/// }
///
/// let app = Router::new().route("/", get(show_host));
/// ```
#[derive(Debug, Clone)]
pub struct Host(pub String);

impl<S> FromRequestParts<S> for Host
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // 以Host header为优先
        // HTTP1 要求必须传递 Host header
        // HTTP2 中用于特殊情况的需求，例如反向代理指定 Host
        if let Some(host) = parts.headers.get("host") {
            if let Ok(host_str) = host.to_str() {
                return Ok(Host(host_str.to_string()));
            }
        }

        // HTTP/2 使用 :authority pseudo-header
        // 在 Axum/Hyper 中，:authority 会被转换为 URI 的 authority 部分
        if let Some(authority) = parts.uri.authority() {
            return Ok(Host(authority.to_string()));
        }

        // 无法获取 Host 信息
        Err(AppError::new(io::Error::new(io::ErrorKind::InvalidInput, "Missing Host information in request")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[tokio::test]
    async fn test_host_from_header() {
        let req = Request::builder().uri("/test").header("host", "example.com:8080").body(()).unwrap();

        let (mut parts, _) = req.into_parts();
        let host = Host::from_request_parts(&mut parts, &()).await.unwrap();

        assert_eq!(host.0, "example.com:8080");
    }

    #[tokio::test]
    async fn test_host_from_authority() {
        let req = Request::builder().uri("http://example.com:8080/test").body(()).unwrap();

        let (mut parts, _) = req.into_parts();
        let host = Host::from_request_parts(&mut parts, &()).await.unwrap();

        assert_eq!(host.0, "example.com:8080");
    }

    #[tokio::test]
    async fn test_missing_host() {
        let req = Request::builder().uri("/test").body(()).unwrap();

        let (mut parts, _) = req.into_parts();
        let result = Host::from_request_parts(&mut parts, &()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_authority_precedence() {
        // 当同时存在 URI authority 和 Host header 时，应该优先使用 authority
        let req = Request::builder()
            .uri("http://authority.com:8080/test")
            .header("host", "header.com:9090")
            .body(())
            .unwrap();

        let (mut parts, _) = req.into_parts();
        let host = Host::from_request_parts(&mut parts, &()).await.unwrap();

        assert_eq!(host.0, "authority.com:8080");
    }
}
