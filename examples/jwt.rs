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

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 监听端口
    #[arg(short, long, default_value = "8080")]
    port: u16,

    /// 用户名
    #[arg(short, long)]
    username: String,

    /// 密码
    #[arg(long)]
    password: String,

    /// JWT 密钥
    #[arg(long, default_value = "your-secret-key-change-in-production")]
    jwt_secret: String,

    /// TLS 证书路径
    #[arg(long)]
    cert: Option<String>,

    /// TLS 私钥路径
    #[arg(long)]
    key: Option<String>,
}

// 可以在这里进行一些预处理
pub(crate) static PARAM: std::sync::LazyLock<Args> = std::sync::LazyLock::new(Args::parse);
const CARGO_CRATE_NAME: &str = env!("CARGO_CRATE_NAME");

#[derive(Clone)]
pub struct AppState {
    pub jwt_config: JwtConfig,
    pub username: String,
    pub password_hash: String,
}

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

mod handler {
    use std::sync::Arc;

    use axum::{Json, extract::State};
    use axum_bootstrap::jwt::{Claims, ClaimsPayload};
    use axum_extra::extract::{
        CookieJar,
        cookie::{Cookie, SameSite},
    };
    use axum_macros::debug_handler;
    use hyper::StatusCode;
    use log::error;
    use serde::{Deserialize, Serialize};

    use crate::AppState;

    #[derive(Serialize)]
    pub struct UserInfo {
        username: String,
    }

    #[debug_handler]
    pub async fn get_current_user(Claims { payload, .. }: Claims) -> Result<Json<UserInfo>, StatusCode> {
        Ok(Json(UserInfo { username: payload.username }))
    }

    #[derive(Deserialize, Debug)]
    pub struct LoginRequest {
        username: String,
        password: String,
    }

    #[derive(Serialize)]
    pub struct LoginResponse {
        success: bool,
        message: String,
    }

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

        // 生成JWT token
        let token = Claims::new(ClaimsPayload { username: req.username })
            .encode(&state.jwt_config)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // 创建cookie
        let cookie = Cookie::build(("token", token))
            .path("/")
            .max_age(time::Duration::days(7))
            .same_site(SameSite::Lax)
            .http_only(true)
            .build();

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

    pub async fn logout_handler() -> Result<(StatusCode, CookieJar, Json<LoginResponse>), StatusCode> {
        let cookie = Cookie::build(("token", ""))
            .path("/")
            .max_age(time::Duration::seconds(-1))
            .same_site(SameSite::Lax)
            .http_only(true)
            .build();

        let jar = CookieJar::new().add(cookie);

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
