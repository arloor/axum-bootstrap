//! # 基础示例程序
//!
//! 展示如何使用 axum-bootstrap 创建一个完整的 Web 服务器
//!
//! # 主要功能
//! - HTTP/HTTPS 服务器
//! - MySQL 数据库连接 (可选)
//! - Prometheus 指标收集
//! - CORS 支持
//! - 请求超时控制
//! - 响应压缩
//! - 请求追踪
//!
//! # 启动方式
//!
//! ```bash
//! # HTTP 模式
//! cargo run --example basic
//!
//! # HTTPS 模式
//! cargo run --example basic -- --tls --cert cert.pem --key privkey.pem
//!
//! # 启用 MySQL 支持
//! cargo run --example basic --features mysql
//! ```

#![deny(warnings)]

use std::time::Duration;

use axum_bootstrap::TlsParam;

use http::init_http_client;

use clap::Parser;

/// 动态错误类型别名
type DynError = Box<dyn std::error::Error + Send + Sync>;

/// 命令行参数配置
#[derive(Parser)]
#[command(author, version=None, about, long_about = None)]
pub struct Param {
    /// 监听端口
    #[arg(long, short, value_name = "port", default_value = "4000")]
    port: u16,

    /// HTTP 代理地址 (用于 reqwest 客户端)
    #[arg(long, value_name = "reqwest client的代理", default_value = "")]
    http_proxy: String,

    /// TLS 证书文件路径
    #[arg(long, value_name = "CERT", default_value = "cert.pem")]
    cert: String,

    /// TLS 私钥文件路径
    #[arg(long, value_name = "KEY", default_value = "privkey.pem")]
    key: String,

    /// 是否启用 HTTPS
    #[arg(short, long, help = "if enable, server will listen on https")]
    tls: bool,
}

/// 全局参数实例 (懒加载)
pub(crate) static PARAM: std::sync::LazyLock<Param> = std::sync::LazyLock::new(Param::parse);

/// 项目名称 (编译时获取)
const CARGO_CRATE_NAME: &str = env!("CARGO_CRATE_NAME");

/// 程序入口
///
/// # 启动流程
/// 1. 初始化日志系统
/// 2. 创建 HTTP 客户端
/// 3. 连接 MySQL 数据库 (如果启用)
/// 4. 构建路由和中间件
/// 5. 启动服务器
#[tokio::main]
pub async fn main() -> Result<(), DynError> {
    axum_bootstrap::init_log::tracing::init(CARGO_CRATE_NAME)?;
    // axum_bootstrap::init_log::env_logger::init(CARGO_CRATE_NAME);
    log::info!("init http client...");
    let client = init_http_client(&PARAM.http_proxy).await?;

    #[cfg(feature = "mysql")]
    {
        log::info!("connecting to mysql...");
        let pool: sqlx::Pool<sqlx::MySql> = sqlx_mysql::MySqlPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(10))
            // .connect("mysql://root:xxxxxx@127.0.0.1:3306/test?ssl-mode=Required&timezone=%2B08:00")
            .connect_with(
                sqlx_mysql::MySqlConnectOptions::new()
                    .host("127.0.0.1")
                    .username("root")
                    .password("xxxxxx")
                    .database("test")
                    .ssl_mode(sqlx_mysql::MySqlSslMode::Required)
                    .timezone(Some(String::from("+08:00"))),
            )
            .await?;
        use axum_bootstrap::generate_shutdown_receiver;
        let server = axum_bootstrap::new_server(PARAM.port, handler::build_router(handler::AppState { client, pool }), register_shutdown_receiver());
        let server = server.with_timeout(Duration::from_secs(120)).with_tls_param(match PARAM.tls {
            true => Some(TlsParam {
                tls: true,
                cert: PARAM.cert.to_string(),
                key: PARAM.key.to_string(),
            }),
            false => None,
        });

        server.run().await?;
    }

    #[cfg(not(feature = "mysql"))]
    {
        use axum_bootstrap::generate_shutdown_receiver;
        let server = axum_bootstrap::new_server(PARAM.port, handler::build_router(handler::AppState { client }), generate_shutdown_receiver());
        let server = server.with_timeout(Duration::from_secs(120)).with_tls_param(match PARAM.tls {
            true => Some(TlsParam {
                tls: true,
                cert: PARAM.cert.to_string(),
                key: PARAM.key.to_string(),
            }),
            false => None,
        });

        server.run().await?;
    }

    Ok(())
}

/// 请求处理器模块
mod handler {
    #![allow(unused)]
    use std::{io, net::SocketAddr, sync::Arc, time::Duration};

    use axum::{
        Json, Router,
        extract::{ConnectInfo, MatchedPath, Request, State},
        http::{self, HeaderValue},
        routing::get,
    };
    use axum_macros::debug_handler;
    use chrono::NaiveDateTime;
    use hyper::{HeaderMap, StatusCode};
    use log::info;
    use prometheus_client::encoding::text::encode;
    use serde::{Deserialize, Serialize};
    use sqlx::FromRow;
    use tracing_subscriber::{prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt};

    use axum_bootstrap::{error::AppError, util::json::StupidValue};
    use tokio::time::sleep;
    use tower_http::{compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer, trace::TraceLayer};

    use crate::metrics::{HttpReqLabel, METRIC};

    /// 应用状态 (跨请求共享)
    pub(crate) struct AppState {
        /// MySQL 连接池 (可选功能)
        #[cfg(feature = "mysql")]
        pub(crate) pool: sqlx::MySqlPool,

        /// HTTP 客户端 (用于发送外部请求)
        pub(crate) client: reqwest::Client,
    }

    /// 构建 Axum 路由和中间件栈
    ///
    /// # 路由列表
    /// - `GET /`: 返回客户端地址
    /// - `GET /time`: 延迟 20 秒后返回 (用于测试超时)
    /// - `GET /metrics`: Prometheus 指标
    /// - `GET /error`: 错误处理示例
    /// - `GET|POST /data`: 数据查询接口
    ///
    /// # 中间件栈 (从下到上)
    /// 1. TraceLayer: 请求追踪
    /// 2. CorsLayer: CORS 支持
    /// 3. TimeoutLayer: 请求超时控制 (30 秒)
    /// 4. CompressionLayer: 响应压缩
    pub(crate) fn build_router(app_state: AppState) -> Router {
        // build our application with a route
        Router::new()
            .route("/", get(|ConnectInfo(addr): ConnectInfo<SocketAddr>| async move { (StatusCode::OK, format!("{addr}")) }))
            .route(
                "/time",
                get(|| async {
                    sleep(Duration::from_secs(20)).await;
                    (StatusCode::OK, "OK")
                }),
            )
            .route("/metrics", get(metrics_handler))
            .route("/error", get(error_func))
            .route("/data", get(data_handler).post(data_handler))
            .layer((
                TraceLayer::new_for_http() // Create our own span for the request and include the matched path. The matched
                    // path is useful for figuring out which handler the request was routed to.
                    .make_span_with(make_span)
                    // By default `TraceLayer` will log 5xx responses but we're doing our specific
                    // logging of errors so disable that
                    .on_failure(()),
                CorsLayer::permissive(),
                TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(30)),
                CompressionLayer::new(),
            ))
            .with_state(Arc::new(app_state))
    }

    /// 为 TraceLayer 创建自定义 span
    ///
    /// # 记录字段
    /// - `method`: HTTP 方法
    /// - `path`: 请求路径
    /// - `matched_path`: 匹配的路由模式
    fn make_span(req: &http::Request<axum::body::Body>) -> tracing::Span {
        let method = req.method();
        let path = req.uri().path();

        // Axum 会自动添加这个扩展
        let matched_path = req.extensions().get::<MatchedPath>().map(|matched_path| matched_path.as_str());

        tracing::debug_span!("recv request", %method, %path, matched_path)
    }

    /// Prometheus 指标处理器
    ///
    /// # 返回
    /// - 文本格式的 Prometheus 指标数据
    pub(crate) async fn metrics_handler() -> Result<(StatusCode, String), AppError> {
        let mut buffer = String::new();
        if let Err(e) = encode(&mut buffer, &METRIC.prom_registry) {
            log::error!("Failed to encode metrics: {e:?}");
            return Err(AppError::new(io::Error::other(e)));
        }
        Ok((StatusCode::OK, buffer))
    }

    /// 错误处理示例函数
    ///
    /// 用于测试错误响应的格式和日志记录
    pub(crate) async fn error_func() -> Result<(StatusCode, String), AppError> {
        Err(AppError::new(io::Error::other("MOCK error")))
    }

    /// 数据查询处理器
    ///
    /// # 功能
    /// - 接收带时间范围的查询请求
    /// - 从 MySQL 查询当前时间 (需要启用 mysql feature)
    /// - 返回 JSON 响应
    ///
    /// # 请求示例
    /// ```json
    /// {
    ///   "startTime": "2024-01-01 00:00:00",
    ///   "endTime": "2024-01-31 23:59:59",
    ///   "distinctCode": true
    /// }
    /// ```
    #[debug_handler]
    pub(crate) async fn data_handler(
        State(state): State<Arc<AppState>>, req: Json<DataRequest>,
    ) -> (StatusCode, HeaderMap, Json<Response<Vec<Data>>>) {
        METRIC.req_count.get_or_create(&HttpReqLabel { path: "test".to_string() }).inc();
        info!("req: {req:?}");
        #[cfg(not(feature = "mysql"))]
        return (StatusCode::INTERNAL_SERVER_ERROR, some_headers(), Json(Response::error("mysql not enabled".to_string())));
        #[cfg(feature = "mysql")]
        {
            use std::borrow::Borrow;
            let pool = state.pool.borrow();
            match sqlx::query!(r"select now() as now_local, now() as now_naive, now() as now_utc;")
                .fetch_one(pool)
                .await
            {
                Ok(row) => (
                    StatusCode::OK,
                    some_headers(),
                    Json(Response::success(vec![Data {
                        now_local: row.now_local,
                        now_naive: row.now_naive,
                        now_utc: row.now_utc,
                    }])),
                ),
                Err(e) => {
                    log::warn!("query now failed: {:?}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, some_headers(), Json(Response::error(format!("query now failed: {:?}", e))))
                }
            }
        }
    }

    /// 数据查询请求结构
    #[derive(Serialize, Deserialize, Debug)]
    pub(crate) struct DataRequest {
        /// 开始时间 (格式: YYYY-MM-DD HH:MM:SS)
        #[serde(rename = "startTime", with = "axum_bootstrap::util::json::my_date_format_option", default)]
        pub(crate) start_time: Option<NaiveDateTime>,

        /// 结束时间 (格式: YYYY-MM-DD HH:MM:SS)
        #[serde(rename = "endTime", with = "axum_bootstrap::util::json::my_date_format_option", default)]
        pub(crate) end_time: Option<NaiveDateTime>,

        /// 是否去重 (支持字符串 "true"/"false" 或布尔值)
        #[serde(rename = "distinctCode", default)]
        pub(crate) distinct_code: StupidValue<bool>,
    }

    /// 数据响应结构 (对应 MySQL 查询结果)
    #[derive(serde::Serialize, Debug, FromRow)]
    pub(crate) struct Data {
        /// 本地时间
        #[serde(with = "axum_bootstrap::util::json::my_date_format")]
        pub(crate) now_local: NaiveDateTime,

        /// Naive 时间 (无时区)
        #[serde(with = "axum_bootstrap::util::json::my_date_format")]
        pub(crate) now_naive: NaiveDateTime,

        /// UTC 时间
        #[serde(with = "axum_bootstrap::util::json::my_date_format")]
        pub(crate) now_utc: NaiveDateTime,
    }

    /// 统一响应结构
    ///
    /// # 字段
    /// - `code`: 状态码 (200=成功, 500=失败)
    /// - `message`: 消息文本
    /// - `data`: 数据负载 (可选)
    #[derive(Serialize)]
    pub(crate) struct Response<T: Serialize> {
        code: i32,
        message: String,
        data: Option<T>,
    }

    impl<T: Serialize> Response<T> {
        /// 创建成功响应
        fn success(data: T) -> Response<T> {
            Response {
                code: 200,
                message: "success".to_string(),
                data: Some(data),
            }
        }

        /// 创建错误响应
        fn error(msg: String) -> Response<T> {
            Response {
                code: 500,
                message: msg,
                data: None,
            }
        }
    }

    /// 创建一些通用的响应头
    ///
    /// # 返回
    /// 包含 CORS 头的 HeaderMap
    pub fn some_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
        headers
    }
}

/// Prometheus 指标模块
mod metrics {
    use std::sync::LazyLock;

    use prometheus_client::{
        encoding::EncodeLabelSet,
        metrics::{counter::Counter, family::Family},
        registry::Registry,
    };

    /// 全局指标实例 (懒加载)
    pub(crate) static METRIC: LazyLock<Metrics> = LazyLock::new(|| {
        let mut prom_registry = Registry::default();
        let req_count = Family::<HttpReqLabel, Counter>::default();
        prom_registry.register("req_count", "help", req_count.clone());
        Metrics { prom_registry, req_count }
    });

    /// 指标集合
    pub(crate) struct Metrics {
        /// Prometheus 注册表
        pub(crate) prom_registry: Registry,

        /// HTTP 请求计数器 (按路径分组)
        pub(crate) req_count: Family<HttpReqLabel, Counter>,
    }

    /// HTTP 请求标签
    ///
    /// 用于区分不同路径的请求指标
    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    pub(crate) struct HttpReqLabel {
        /// 请求路径
        pub(crate) path: String,
    }
}

/// HTTP 客户端模块
mod http {
    use reqwest::Client;

    use crate::DynError;

    /// 初始化 HTTP 客户端
    ///
    /// # 参数
    /// - `http_proxy`: 代理地址 (空字符串表示不使用代理)
    ///
    /// # 返回
    /// - `Ok(Client)`: 配置好的 reqwest 客户端
    /// - `Err(DynError)`: 初始化失败
    ///
    /// # 配置
    /// - 连接池: 每个 host 最多 20 个空闲连接
    pub async fn init_http_client(http_proxy: &str) -> Result<Client, DynError> {
        let client_builder = Client::builder().pool_max_idle_per_host(20);
        if http_proxy.is_empty() {
            Ok(client_builder.build()?)
        } else {
            Ok(client_builder.proxy(reqwest::Proxy::all(http_proxy)?).build()?)
        }
    }
}
