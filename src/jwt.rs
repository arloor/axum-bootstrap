use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, request::Parts},
    middleware::Next,
    response::{Html, Response},
};
use axum_extra::extract::CookieJar;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// JWT过期时间（7天）
const JWT_EXPIRATION_HOURS: i64 = 24 * 7;

#[derive(Clone)]
pub struct JwtConfig {
    pub encoding_key: EncodingKey,
    pub decoding_key: DecodingKey,
}

impl JwtConfig {
    pub fn new(secret: &str) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims<T = ClaimsPayload> {
    pub payload: T, // 自定义负载
    pub exp: usize, // 过期时间, 必须。用于验证是否过期
    pub iat: usize, // 签发时间
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimsPayload {
    pub username: String,
}

impl<T> Claims<T> {
    /// 从payload创建Claims
    pub fn new(payload: T) -> Self {
        let now = chrono::Utc::now();
        let exp = (now + chrono::Duration::hours(JWT_EXPIRATION_HOURS)).timestamp() as usize;
        let iat = now.timestamp() as usize;

        Claims { payload, exp, iat }
    }

    /// 将Claims编码为JWT token
    pub fn encode(&self, config: &JwtConfig) -> Result<String, jsonwebtoken::errors::Error>
    where
        T: Serialize,
    {
        encode(&Header::default(), self, &config.encoding_key)
    }

    /// 从JWT token解码为Claims
    pub fn decode(token: &str, config: &JwtConfig) -> Result<Self, jsonwebtoken::errors::Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        let validation = Validation::default();
        let token_data = decode::<Claims<T>>(token, &config.decoding_key, &validation)?;
        Ok(token_data.claims)
    }
}

/// JWT认证中间件
pub async fn jwt_auth_middleware<T>(
    State(config): State<Arc<JwtConfig>>, cookie_jar: CookieJar, mut request: Request, next: Next,
) -> Result<Response, (StatusCode, Html<String>)>
where
    T: for<'de> Deserialize<'de> + Send + Sync + 'static,
    T: Clone,
{
    // 从cookie中获取JWT token
    let token = cookie_jar
        .get("token")
        .map(|cookie| cookie.value().to_string())
        .ok_or((StatusCode::UNAUTHORIZED, Html("Missing token".to_string())))?;

    // 验证JWT token
    let claims = Claims::<T>::decode(&token, &config).map_err(|e| {
        log::error!("JWT验证失败: {:?}", e);
        (StatusCode::UNAUTHORIZED, Html("Invalid token".to_string()))
    })?;

    // 将claims存入request extensions，后续handler可以使用
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

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
