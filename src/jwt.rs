//! # JWT 认证模块
//!
//! 提供基于 JWT (JSON Web Token) 的用户认证功能
//!
//! # 主要特性
//! - JWT token 生成和验证
//! - Cookie-based 会话管理
//! - Axum 中间件集成
//! - 泛型 Claims 支持自定义负载
//! - 自动提取认证信息
//!
//! # 使用示例
//!
//! ```no_run
//! use axum::{Router, routing::get};
//! use axum_bootstrap::jwt::{JwtConfig, Claims, ClaimsPayload, jwt_auth_middleware};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let jwt_config = Arc::new(JwtConfig::new("your-secret-key"));
//!
//!     let app = Router::new()
//!         .route("/protected", get(protected_handler))
//!         .layer(axum::middleware::from_fn_with_state(
//!             jwt_config.clone(),
//!             jwt_auth_middleware::<ClaimsPayload>
//!         ))
//!         .with_state(jwt_config);
//! }
//!
//! async fn protected_handler(claims: Claims<ClaimsPayload>) -> String {
//!     format!("Hello, {}!", claims.payload.username)
//! }
//! ```

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, request::Parts},
    middleware::Next,
    response::{Html, Response},
};
use axum_extra::extract::CookieJar;
use cookie::{Cookie, SameSite};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};

/// JWT 过期时间（7天）
const JWT_EXPIRATION_HOURS: i64 = 24 * 7;

/// Cookie 名称常量
const AXUM_BOOTSTRAP_TOKEN: &str = "axum-boostrap-token";

/// 登出时使用的 Cookie (过期的 cookie，用于清除客户端 token)
///
/// # 说明
/// 通过设置 max_age 为负值，浏览器会立即删除该 cookie
pub static LOGOUT_COOKIE: LazyLock<Cookie<'_>> = LazyLock::new(|| {
    Cookie::build((AXUM_BOOTSTRAP_TOKEN, ""))
        .path("/")
        .max_age(time::Duration::seconds(-1))
        .same_site(SameSite::Lax)
        .http_only(true)
        .build()
});

/// JWT 配置
///
/// 存储 JWT 编码和解码所需的密钥
///
/// # 字段
/// - `encoding_key`: 用于生成 JWT token 的密钥
/// - `decoding_key`: 用于验证 JWT token 的密钥
#[derive(Clone)]
pub struct JwtConfig {
    pub encoding_key: EncodingKey,
    pub decoding_key: DecodingKey,
}

impl JwtConfig {
    /// 从密钥字符串创建 JWT 配置
    ///
    /// # 参数
    /// - `secret`: 密钥字符串，用于签名和验证 JWT
    ///
    /// # 返回
    /// 配置好的 JwtConfig 实例
    ///
    /// # 示例
    ///
    /// ```
    /// use axum_bootstrap::jwt::JwtConfig;
    ///
    /// let config = JwtConfig::new("my-secret-key");
    /// ```
    pub fn new(secret: &str) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
        }
    }
}

/// JWT Claims (声明)
///
/// 泛型结构，支持自定义负载类型
///
/// # 泛型参数
/// - `T`: 自定义负载类型，默认为 `ClaimsPayload`
///
/// # 字段
/// - `payload`: 自定义负载数据
/// - `exp`: 过期时间 (Unix 时间戳)，JWT 标准字段
/// - `iat`: 签发时间 (Unix 时间戳)，JWT 标准字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims<T = ClaimsPayload> {
    pub payload: T,
    pub exp: usize,
    pub iat: usize,
}

/// 默认的 Claims 负载
///
/// 包含用户名信息
///
/// # 字段
/// - `username`: 用户名
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimsPayload {
    pub username: String,
}

impl<T> Claims<T> {
    /// 从自定义负载创建 Claims
    ///
    /// 自动设置签发时间 (iat) 和过期时间 (exp)
    ///
    /// # 参数
    /// - `payload`: 自定义负载数据
    ///
    /// # 返回
    /// 新创建的 Claims 实例，过期时间为当前时间 + 7天
    ///
    /// # 示例
    ///
    /// ```
    /// use axum_bootstrap::jwt::{Claims, ClaimsPayload};
    ///
    /// let payload = ClaimsPayload { username: "alice".to_string() };
    /// let claims = Claims::new(payload);
    /// ```
    pub fn new(payload: T) -> Self {
        let now = chrono::Utc::now();
        let exp = (now + chrono::Duration::hours(JWT_EXPIRATION_HOURS)).timestamp() as usize;
        let iat = now.timestamp() as usize;

        Claims { payload, exp, iat }
    }

    /// 将 Claims 编码为 JWT token
    ///
    /// # 参数
    /// - `config`: JWT 配置，包含编码密钥
    ///
    /// # 返回
    /// - `Ok(String)`: 编码后的 JWT token 字符串
    /// - `Err`: 编码失败
    pub(crate) fn encode(&self, config: &JwtConfig) -> Result<String, jsonwebtoken::errors::Error>
    where
        T: Serialize,
    {
        encode(&Header::default(), self, &config.encoding_key)
    }

    /// 将 Claims 转换为 HTTP Cookie
    ///
    /// 生成包含 JWT token 的 HttpOnly Cookie，可直接用于响应
    ///
    /// # 参数
    /// - `jwt_config`: JWT 配置
    ///
    /// # 返回
    /// - `Ok(Cookie)`: 配置好的 Cookie
    /// - `Err`: JWT 编码失败
    ///
    /// # Cookie 属性
    /// - `HttpOnly`: true (防止 JavaScript 访问)
    /// - `SameSite`: Lax (CSRF 保护)
    /// - `Path`: / (全站有效)
    /// - `MaxAge`: 7天
    ///
    /// # 示例
    ///
    /// ```no_run
    /// use axum_bootstrap::jwt::{Claims, ClaimsPayload, JwtConfig};
    ///
    /// let config = JwtConfig::new("secret");
    /// let payload = ClaimsPayload { username: "alice".to_string() };
    /// let claims = Claims::new(payload);
    /// let cookie = claims.to_cookie(&config).unwrap();
    /// ```
    pub fn to_cookie<'a>(&'_ self, jwt_config: &JwtConfig) -> Result<Cookie<'a>, jsonwebtoken::errors::Error>
    where
        T: Serialize,
    {
        // 生成 JWT token
        let token = self.encode(jwt_config)?;

        // 创建 cookie
        Ok(Cookie::build((AXUM_BOOTSTRAP_TOKEN, token))
            .path("/")
            .max_age(time::Duration::days(7))
            .same_site(SameSite::Lax)
            .http_only(true)
            .build())
    }

    /// 从 JWT token 解码为 Claims
    ///
    /// # 参数
    /// - `token`: JWT token 字符串
    /// - `config`: JWT 配置，包含解码密钥
    ///
    /// # 返回
    /// - `Ok(Claims)`: 解码后的 Claims
    /// - `Err`: 解码或验证失败 (token 无效、过期等)
    ///
    /// # 示例
    ///
    /// ```no_run
    /// use axum_bootstrap::jwt::{Claims, ClaimsPayload, JwtConfig};
    ///
    /// let config = JwtConfig::new("secret");
    /// let claims = Claims::<ClaimsPayload>::decode("token_string", &config).unwrap();
    /// ```
    pub fn decode(token: &str, config: &JwtConfig) -> Result<Self, jsonwebtoken::errors::Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        let validation = Validation::default();
        let token_data = decode::<Claims<T>>(token, &config.decoding_key, &validation)?;
        Ok(token_data.claims)
    }
}

/// JWT 认证中间件
///
/// 从 Cookie 中提取并验证 JWT token，将 Claims 存入 request extensions
///
/// # 类型参数
/// - `T`: Claims 负载类型
///
/// # 参数
/// - `config`: JWT 配置 (从 State 提取)
/// - `cookie_jar`: Cookie 容器 (从请求提取)
/// - `request`: HTTP 请求
/// - `next`: 下一个中间件或处理器
///
/// # 返回
/// - `Ok(Response)`: 认证成功，返回处理结果
/// - `Err((StatusCode, Html))`: 认证失败，返回 401 错误
///
/// # 错误
/// - `UNAUTHORIZED (401)`: token 缺失或验证失败
///
/// # 示例
///
/// ```no_run
/// use axum::{Router, routing::get, middleware};
/// use axum_bootstrap::jwt::{JwtConfig, jwt_auth_middleware, ClaimsPayload};
/// use std::sync::Arc;
///
/// let jwt_config = Arc::new(JwtConfig::new("secret"));
/// let app = Router::new()
///     .route("/protected", get(handler))
///     .layer(middleware::from_fn_with_state(
///         jwt_config.clone(),
///         jwt_auth_middleware::<ClaimsPayload>
///     ))
///     .with_state(jwt_config);
///
/// async fn handler() -> &'static str { "OK" }
/// ```
pub async fn jwt_auth_middleware<T>(
    State(config): State<Arc<JwtConfig>>, cookie_jar: CookieJar, mut request: Request, next: Next,
) -> Result<Response, (StatusCode, Html<String>)>
where
    T: for<'de> Deserialize<'de> + Send + Sync + 'static,
    T: Clone,
{
    // 从 cookie 中获取 JWT token
    let token = cookie_jar
        .get(AXUM_BOOTSTRAP_TOKEN)
        .map(|cookie| cookie.value().to_string())
        .ok_or((StatusCode::UNAUTHORIZED, Html("Missing token".to_string())))?;

    // 验证 JWT token
    let claims = Claims::<T>::decode(&token, &config).map_err(|e| {
        log::error!("JWT验证失败: {:?}", e);
        (StatusCode::UNAUTHORIZED, Html("Invalid token".to_string()))
    })?;

    // 将 claims 存入 request extensions，后续 handler 可以通过提取器获取
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// 实现 Claims 作为 Axum 提取器
///
/// 允许在路由处理器中直接提取 Claims
///
/// # 示例
///
/// ```no_run
/// use axum_bootstrap::jwt::{Claims, ClaimsPayload};
///
/// async fn handler(claims: Claims<ClaimsPayload>) -> String {
///     format!("Hello, {}!", claims.payload.username)
/// }
/// ```
impl<S, T> FromRequestParts<S> for Claims<T>
where
    S: Send + Sync,
    T: Send + Sync + 'static,
    T: Clone,
{
    type Rejection = (StatusCode, Html<String>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let claims = parts
            .extensions
            .get::<Claims<T>>()
            .ok_or((StatusCode::UNAUTHORIZED, Html("Missing or invalid token".to_string())))?;

        Ok(claims.clone())
    }
}
