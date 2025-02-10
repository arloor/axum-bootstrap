#![allow(unused)]
use std::{io, sync::Arc, time::Duration};

use axum::{extract::State, http::HeaderValue, routing::get, Json, Router};
use axum_macros::debug_handler;
use chrono::NaiveDateTime;
use hyper::{HeaderMap, StatusCode};
use log::info;
use prometheus_client::encoding::text::encode;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use axum_bootstrap::{util::json::StupidValue, AppError};
use tokio::time::sleep;
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer, trace::TraceLayer,
};

use crate::metrics::{HttpReqLabel, METRIC};

pub(crate) struct AppState {
    #[cfg(feature = "mysql")]
    pub(crate) pool: sqlx::MySqlPool,
    pub(crate) client: reqwest::Client,
}

pub(crate) fn build_router(app_state: AppState) -> Router {
    // build our application with a route
    Router::new()
        .route("/", get(|| async { (StatusCode::OK, "OK") }))
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
            TraceLayer::new_for_http(),
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
    Err(AppError::new(io::Error::new(
        io::ErrorKind::Other,
        "MOCK error",
    )))
}

#[debug_handler]
pub(crate) async fn data_handler(
    State(state): State<Arc<AppState>>,
    req: Json<DataRequest>,
) -> (StatusCode, HeaderMap, Json<Response<Vec<Data>>>) {
    METRIC
        .req_count
        .get_or_create(&HttpReqLabel {
            path: "test".to_string(),
        })
        .inc();
    info!("req: {:?}", req);
    #[cfg(not(feature = "mysql"))]
    return (
        StatusCode::INTERNAL_SERVER_ERROR,
        some_headers(),
        Json(Response::error("mysql not enabled".to_string())),
    );
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
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    some_headers(),
                    Json(Response::error(format!("query now failed: {:?}", e))),
                )
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct DataRequest {
    #[serde(
        rename = "startTime",
        with = "axum_bootstrap::util::json::my_date_format_option",
        default
    )]
    pub(crate) start_time: Option<NaiveDateTime>,
    #[serde(
        rename = "endTime",
        with = "axum_bootstrap::util::json::my_date_format_option",
        default
    )]
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
