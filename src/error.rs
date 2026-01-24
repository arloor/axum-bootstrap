//! # 错误处理模块
//!
//! 提供统一的错误类型 `AppError`，用于在 Axum 应用中处理各种错误
//!
//! # 主要特性
//! - 包装 `anyhow::Error`，提供灵活的错误处理
//! - 自动实现 `IntoResponse`，可直接在路由处理器中返回
//! - 支持 `?` 操作符，自动转换标准错误类型
//! - 统一的错误响应格式 (HTTP 500)
//!
//! # 示例
//!
//! ```no_run
//! use axum_bootstrap::error::AppError;
//! use axum::response::IntoResponse;
//!
//! async fn handler() -> Result<impl IntoResponse, AppError> {
//!     // 可以直接使用 ? 操作符
//!     let _result = std::fs::read_to_string("file.txt")?;
//!     Ok("success")
//! }
//! ```

use anyhow::anyhow;
use std::fmt::Display;

use axum::response::{IntoResponse, Response};
use hyper::StatusCode;

/// 应用程序错误类型
///
/// 包装 `anyhow::Error`，提供统一的错误处理和响应转换
///
/// # 说明
/// 该错误类型会自动将所有错误转换为 HTTP 500 响应，
/// 并记录详细的错误日志（使用 tracing）
#[derive(Debug)]
pub struct AppError(anyhow::Error);

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 实现 Axum 响应转换
///
/// 将错误转换为 HTTP 500 响应，并记录错误日志
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let err = self.0;
        // TraceLayer 已经包含了请求方法、URI 等信息，这里不需要重复记录
        tracing::error!(%err, "error");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("ERROR: {}", &err)).into_response()
    }
}

/// 自动转换其他错误类型为 AppError
///
/// 这使得可以在返回 `Result<_, AppError>` 的函数中直接使用 `?` 操作符
///
/// # 示例
///
/// ```no_run
/// # use axum_bootstrap::error::AppError;
/// fn my_function() -> Result<(), AppError> {
///     // 自动转换 std::io::Error 为 AppError
///     let _content = std::fs::read_to_string("file.txt")?;
///     Ok(())
/// }
/// ```
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl AppError {
    /// 从标准错误类型创建 AppError
    ///
    /// # 参数
    /// - `err`: 任何实现了 `std::error::Error + Send + Sync` 的错误
    ///
    /// # 返回
    /// 包装后的 AppError
    pub fn new<T: std::error::Error + Send + Sync + 'static>(err: T) -> Self {
        Self(anyhow!(err))
    }
}
