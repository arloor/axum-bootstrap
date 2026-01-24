//! # JWT 认证示例程序
//!
//! 展示如何使用 axum-bootstrap 的 JWT 模块实现用户认证
//!
//! # 主要功能
//! - 基于 JWT 的用户认证
//! - Cookie-based 会话管理
//! - 密码 bcrypt 哈希存储
//! - 受保护的 API 端点
//! - 静态文件服务 (登录页面)
//! - HTTPS 支持
//!
//! # API 端点
//! - `POST /api/login`: 用户登录，返回 JWT cookie
//! - `POST /api/logout`: 用户登出，清除 cookie
//! - `GET /api/me`: 获取当前用户信息 (需认证)
//! - `GET /health`: 健康检查
//!
//! # 启动方式
//!
//! ```bash
//! # HTTP 模式
//! cargo run --example jwt --features jwt -- \
//!   --username admin \
//!   --password secret123 \
//!   --port 8080
//!
//! # HTTPS 模式
//! cargo run --example jwt --features jwt -- \
//!   --username admin \
//!   --password secret123 \
//!   --port 8443 \
//!   --cert cert.pem \
//!   --key privkey.pem
//! ```

#![deny(warnings)]

use std::{sync::Arc, time::Duration};

use axum::{
    Router, middleware,
    routing::{get, post},
};
use axum_bootstrap::{
    TlsParam,
    jwt::{ClaimsPayload, JwtConfig, jwt_auth_middleware},
};

use clap::Parser;
use hyper::StatusCode;
use tower_http::services::ServeDir;

use crate::handler::{get_current_user, login_handler, logout_handler};

/// 动态错误类型别名
type DynError = Box<dyn std::error::Error + Send + Sync>;

/// 命令行参数配置
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 监听端口
    #[arg(short, long, default_value = "8080")]
    port: u16,

    /// 登录用户名
    #[arg(short, long)]
    username: String,

    /// 登录密码 (将使用 bcrypt 哈希存储)
    #[arg(long)]
    password: String,

    /// JWT 签名密钥 (生产环境请更换)
    #[arg(long, default_value = "your-secret-key-change-in-production")]
    jwt_secret: String,

    /// TLS 证书文件路径 (可选)
    #[arg(long)]
    cert: Option<String>,

    /// TLS 私钥文件路径 (可选)
    #[arg(long)]
    key: Option<String>,
}

/// 全局参数实例 (懒加载)
pub(crate) static PARAM: std::sync::LazyLock<Args> = std::sync::LazyLock::new(Args::parse);

/// 项目名称 (编译时获取)
const CARGO_CRATE_NAME: &str = env!("CARGO_CRATE_NAME");

/// 应用状态 (跨请求共享)
#[derive(Clone)]
pub struct AppState {
    /// JWT 配置
    pub jwt_config: JwtConfig,

    /// 有效用户名
    pub username: String,

    /// 密码哈希 (bcrypt)
    pub password_hash: String,
}

/// 程序入口
///
/// # 启动流程
/// 1. 初始化日志系统
/// 2. 生成密码 bcrypt 哈希
/// 3. 创建 JWT 配置
/// 4. 构建路由 (公开 + 受保护)
/// 5. 配置中间件栈
/// 6. 启动服务器
#[tokio::main]
pub async fn main() -> Result<(), DynError> {
    axum_bootstrap::init_log::tracing::init(CARGO_CRATE_NAME)?;
    // axum_bootstrap::init_log::env_logger::init(CARGO_CRATE_NAME);

    // 生成密码哈希
    let password_hash = bcrypt::hash(&PARAM.password, bcrypt::DEFAULT_COST)?;

    let jwt_config = JwtConfig::new(&PARAM.jwt_secret);

    let state = Arc::new(AppState {
        jwt_config: jwt_config.clone(),
        username: PARAM.username.clone(),
        password_hash,
    });

    // 受保护的路由
    let protected_routes = Router::new()
        .route("/api/me", get(get_current_user))
        .layer(middleware::from_fn_with_state(Arc::new(jwt_config.clone()), jwt_auth_middleware::<ClaimsPayload>));

    // 构建应用
    let app = Router::new()
        .route("/api/login", post(login_handler))
        .route("/api/logout", post(logout_handler))
        .route("/health", get(|| async { (StatusCode::OK, "OK") }))
        .merge(protected_routes)
        .fallback_service(ServeDir::new("static")) // 存放登陆页面
        .layer((
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|req: &axum::extract::Request| {
                    let method = req.method();
                    let path = req.uri().path();
                    tracing::info_span!("request", %method, %path)
                })
                .on_failure(()),
            tower_http::cors::CorsLayer::permissive(),
            tower_http::timeout::TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(30)),
            tower_http::compression::CompressionLayer::new()
                .gzip(true)
                .br(true)
                .deflate(true)
                .zstd(true),
        ))
        .with_state(state);

    use axum_bootstrap::generate_shutdown_receiver;
    let server = axum_bootstrap::new_server(PARAM.port, app, generate_shutdown_receiver());
    let server = server
        .with_timeout(Duration::from_secs(120))
        .with_tls_param(match (PARAM.cert.as_ref(), PARAM.key.as_ref()) {
            (Some(cert), Some(key)) => Some(TlsParam {
                tls: true,
                cert: cert.to_string(),
                key: key.to_string(),
            }),
            _ => None,
        });

    server.run().await?;

    Ok(())
}

/// 请求处理器模块
mod handler {
    use std::sync::Arc;

    use axum::{Json, extract::State};
    use axum_bootstrap::jwt::{Claims, ClaimsPayload, LOGOUT_COOKIE};
    use axum_extra::extract::CookieJar;
    use axum_macros::debug_handler;
    use hyper::StatusCode;
    use log::error;
    use serde::{Deserialize, Serialize};

    use crate::AppState;

    /// 用户信息响应结构
    #[derive(Serialize)]
    pub struct UserInfo {
        username: String,
    }

    /// 获取当前用户信息
    ///
    /// 受 JWT 认证保护的端点
    ///
    /// # 认证
    /// 需要有效的 JWT token (通过 cookie)
    ///
    /// # 返回
    /// - `200 OK`: 返回用户信息
    /// - `401 Unauthorized`: token 无效或缺失
    #[debug_handler]
    pub async fn get_current_user(Claims { payload, .. }: Claims) -> Result<Json<UserInfo>, StatusCode> {
        Ok(Json(UserInfo { username: payload.username }))
    }

    /// 登录请求结构
    #[derive(Deserialize, Debug)]
    pub struct LoginRequest {
        username: String,
        password: String,
    }

    /// 登录响应结构
    #[derive(Serialize)]
    pub struct LoginResponse {
        success: bool,
        message: String,
    }

    /// 用户登录处理器
    ///
    /// # 功能
    /// 1. 验证用户名和密码 (bcrypt)
    /// 2. 生成 JWT token
    /// 3. 设置 HttpOnly cookie
    ///
    /// # 请求示例
    /// ```json
    /// {
    ///   "username": "admin",
    ///   "password": "secret123"
    /// }
    /// ```
    ///
    /// # 返回
    /// - `200 OK`: 登录成功，设置 JWT cookie
    /// - `401 Unauthorized`: 用户名或密码错误
    /// - `500 Internal Server Error`: 服务器错误
    pub async fn login_handler(
        State(state): State<Arc<AppState>>, Json(req): Json<LoginRequest>,
    ) -> Result<(StatusCode, CookieJar, Json<LoginResponse>), StatusCode> {
        // 验证用户名
        if req.username != state.username {
            return Ok((
                StatusCode::UNAUTHORIZED,
                CookieJar::new(),
                Json(LoginResponse {
                    success: false,
                    message: "用户名或密码错误".to_string(),
                }),
            ));
        }

        // 验证密码
        let password_valid = bcrypt::verify(&req.password, &state.password_hash).map_err(|e| {
            error!("密码验证失败: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        if !password_valid {
            return Ok((
                StatusCode::UNAUTHORIZED,
                CookieJar::new(),
                Json(LoginResponse {
                    success: false,
                    message: "用户名或密码错误".to_string(),
                }),
            ));
        }

        let cookie = Claims::new(ClaimsPayload { username: req.username })
            .to_cookie(&state.jwt_config)
            .map_err(|e| {
                error!("生成JWT token失败: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let jar = CookieJar::new().add(cookie);

        Ok((
            StatusCode::OK,
            jar,
            Json(LoginResponse {
                success: true,
                message: "登录成功".to_string(),
            }),
        ))
    }

    /// 用户登出处理器
    ///
    /// # 功能
    /// 清除客户端的 JWT cookie
    ///
    /// # 返回
    /// - `200 OK`: 登出成功，清除 cookie
    pub async fn logout_handler() -> Result<(StatusCode, CookieJar, Json<LoginResponse>), StatusCode> {
        let jar = CookieJar::new().add(LOGOUT_COOKIE.clone());

        Ok((
            StatusCode::OK,
            jar,
            Json(LoginResponse {
                success: true,
                message: "已退出登录".to_string(),
            }),
        ))
    }
}
