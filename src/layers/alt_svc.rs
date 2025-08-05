use std::task::{Context, Poll};
use axum::response::Response;
use http::{HeaderValue, Request};
use tower::{Layer, Service};
use futures_util::future::BoxFuture;

/// Alt-Svc header layer for HTTP/3 advertisement
#[derive(Clone)]
pub struct AltSvcLayer {
    /// The port for HTTP/3 service
    port: u16,
    /// Whether TLS is enabled
    tls_enabled: bool,
    /// Max age for Alt-Svc cache (default: 86400 seconds / 24 hours)
    max_age: u32,
}

impl AltSvcLayer {
    /// Create a new Alt-Svc layer
    pub fn new(port: u16, tls_enabled: bool) -> Self {
        Self {
            port,
            tls_enabled,
            max_age: 86400, // 24 hours
        }
    }

    /// Set the max age for Alt-Svc cache
    pub fn with_max_age(mut self, max_age: u32) -> Self {
        self.max_age = max_age;
        self
    }
}

impl<S> Layer<S> for AltSvcLayer {
    type Service = AltSvcService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AltSvcService {
            inner,
            port: self.port,
            tls_enabled: self.tls_enabled,
            max_age: self.max_age,
        }
    }
}

#[derive(Clone)]
pub struct AltSvcService<S> {
    inner: S,
    port: u16,
    tls_enabled: bool,
    max_age: u32,
}

impl<S, ReqBody> Service<Request<ReqBody>> for AltSvcService<S>
where
    S: Service<Request<ReqBody>, Response = Response> + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let future = self.inner.call(request);
        let port = self.port;
        let tls_enabled = self.tls_enabled;
        let max_age = self.max_age;

        Box::pin(async move {
            let mut response = future.await?;
            
            // Only add Alt-Svc header if TLS is enabled and HTTP/3 feature is available
            #[cfg(feature = "http3")]
            {
                if tls_enabled {
                    let alt_svc_value = format!("h3=\":{}\"; ma={}", port, max_age);
                    if let Ok(header_value) = HeaderValue::from_str(&alt_svc_value) {
                        response.headers_mut().insert("Alt-Svc", header_value);
                    }
                }
            }

            Ok(response)
        })
    }
}