#![deny(warnings)]

use std::time::Duration;

use axum_bootstrap::{util::http::init_http_client, TlsParam};

use clap::Parser;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// axum脚手架
#[derive(Parser)]
#[command(author, version=None, about, long_about = None)]
pub struct Param {
    #[arg(long, short, value_name = "port", default_value = "4000")]
    port: u16,
    #[arg(long, value_name = "reqwest client的代理", default_value = "")]
    http_proxy: String,
    #[arg(long, value_name = "CERT", default_value = "cert.pem")]
    cert: String,
    #[arg(long, value_name = "KEY", default_value = "privkey.pem")]
    key: String,
    #[arg(short, long, help = "if enable, server will listen on https")]
    tls: bool,
}

// 可以在这里进行一些预处理
pub(crate) static PARAM: std::sync::LazyLock<Param> = std::sync::LazyLock::new(Param::parse);
const CARGO_CRATE_NAME: &str = env!("CARGO_CRATE_NAME");
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

        axum_bootstrap::new_server(
            PARAM.port,
            match PARAM.tls {
                true => Some(TlsParam {
                    tls: true,
                    cert: PARAM.cert.to_string(),
                    key: PARAM.key.to_string(),
                }),
                false => None,
            },
            handler::build_router(handler::AppState { client, pool }),
        )
        .with_timeout(Duration::from_secs(120))
        .run()
        .await?;
    }

    #[cfg(not(feature = "mysql"))]
    {
        axum_bootstrap::new_server(
            PARAM.port,
            match PARAM.tls {
                true => Some(TlsParam {
                    tls: true,
                    cert: PARAM.cert.to_string(),
                    key: PARAM.key.to_string(),
                }),
                false => None,
            },
            handler::build_router(handler::AppState { client }),
        )
        .with_timeout(Duration::from_secs(120))
        .run()
        .await?;
    }

    Ok(())
}

mod handler {
    #![allow(unused)]
    use std::{io, net::SocketAddr, sync::Arc, time::Duration};

    use axum::{
        extract::{ConnectInfo, MatchedPath, Request, State},
        http::HeaderValue,
        routing::get,
        Json, Router,
    };
    use axum_macros::debug_handler;
    use chrono::NaiveDateTime;
    use hyper::{HeaderMap, StatusCode};
    use log::info;
    use prometheus_client::encoding::text::encode;
    use serde::{Deserialize, Serialize};
    use sqlx::FromRow;
    use tracing_subscriber::{prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt};

    use axum_bootstrap::{util::json::StupidValue, AppError};
    use tokio::time::sleep;
    use tower_http::{compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer, trace::TraceLayer};

    use crate::metrics::{HttpReqLabel, METRIC};

    pub(crate) struct AppState {
        #[cfg(feature = "mysql")]
        pub(crate) pool: sqlx::MySqlPool,
        pub(crate) client: reqwest::Client,
    }

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
                    .make_span_with(|req: &Request| {
                        let method = req.method();
                        let path = req.uri().path();

                        // axum automatically adds this extension.
                        let matched_path = req.extensions().get::<MatchedPath>().map(|matched_path| matched_path.as_str());

                        tracing::debug_span!("request", %method, %path, matched_path)
                    })
                    // By default `TraceLayer` will log 5xx responses but we're doing our specific
                    // logging of errors so disable that
                    .on_failure(()),
                CorsLayer::permissive(),
                TimeoutLayer::new(Duration::from_secs(30)),
                CompressionLayer::new(),
            ))
            .with_state(Arc::new(app_state))
    }

    pub(crate) async fn metrics_handler() -> Result<(StatusCode, String), AppError> {
        let mut buffer = String::new();
        if let Err(e) = encode(&mut buffer, &METRIC.prom_registry) {
            log::error!("Failed to encode metrics: {:?}", e);
            return Err(AppError::new(io::Error::new(io::ErrorKind::Other, e)));
        }
        Ok((StatusCode::OK, buffer))
    }

    pub(crate) async fn error_func() -> Result<(StatusCode, String), AppError> {
        Err(AppError::new(io::Error::new(io::ErrorKind::Other, "MOCK error")))
    }

    #[debug_handler]
    pub(crate) async fn data_handler(
        State(state): State<Arc<AppState>>, req: Json<DataRequest>,
    ) -> (StatusCode, HeaderMap, Json<Response<Vec<Data>>>) {
        METRIC.req_count.get_or_create(&HttpReqLabel { path: "test".to_string() }).inc();
        info!("req: {:?}", req);
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

    #[derive(Serialize, Deserialize, Debug)]
    pub(crate) struct DataRequest {
        #[serde(rename = "startTime", with = "axum_bootstrap::util::json::my_date_format_option", default)]
        pub(crate) start_time: Option<NaiveDateTime>,
        #[serde(rename = "endTime", with = "axum_bootstrap::util::json::my_date_format_option", default)]
        pub(crate) end_time: Option<NaiveDateTime>,
        #[serde(rename = "distinctCode", default)]
        pub(crate) distinct_code: StupidValue<bool>,
    }

    #[derive(serde::Serialize, Debug, FromRow)]
    pub(crate) struct Data {
        #[serde(with = "axum_bootstrap::util::json::my_date_format")]
        pub(crate) now_local: NaiveDateTime,
        #[serde(with = "axum_bootstrap::util::json::my_date_format")]
        pub(crate) now_naive: NaiveDateTime,
        #[serde(with = "axum_bootstrap::util::json::my_date_format")]
        pub(crate) now_utc: NaiveDateTime,
    }

    #[derive(Serialize)]
    pub(crate) struct Response<T: Serialize> {
        code: i32,
        message: String,
        data: Option<T>,
    }

    impl<T: Serialize> Response<T> {
        fn success(data: T) -> Response<T> {
            Response {
                code: 200,
                message: "success".to_string(),
                data: Some(data),
            }
        }

        fn error(msg: String) -> Response<T> {
            Response {
                code: 500,
                message: msg,
                data: None,
            }
        }
    }

    pub fn some_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
        headers
    }
}

mod metrics {
    use std::sync::LazyLock;

    use prometheus_client::{
        encoding::EncodeLabelSet,
        metrics::{counter::Counter, family::Family},
        registry::Registry,
    };

    pub(crate) static METRIC: LazyLock<Metrics> = LazyLock::new(|| {
        let mut prom_registry = Registry::default();
        let req_count = Family::<HttpReqLabel, Counter>::default();
        prom_registry.register("req_count", "help", req_count.clone());
        Metrics { prom_registry, req_count }
    });

    pub(crate) struct Metrics {
        pub(crate) prom_registry: Registry,
        pub(crate) req_count: Family<HttpReqLabel, Counter>,
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    pub(crate) struct HttpReqLabel {
        pub(crate) path: String,
    }
}
