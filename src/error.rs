use anyhow::anyhow;
use std::fmt::Display;

use axum::response::{IntoResponse, Response};
use hyper::StatusCode;

// Make our own error that wraps `anyhow::Error`.
#[derive(Debug)]
pub struct AppError(anyhow::Error);

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let err = self.0;
        // Because `TraceLayer` wraps each request in a span that contains the request
        // method, uri, etc we don't need to include those details here
        tracing::error!(%err, "error");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("ERROR: {}", &err)).into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl AppError {
    pub fn new<T: std::error::Error + Send + Sync + 'static>(err: T) -> Self {
        Self(anyhow!(err))
    }
}
