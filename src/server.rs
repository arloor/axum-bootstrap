// #![allow(clippy::unwrap_used)]
use std::{borrow::Borrow, sync::Arc};

use crate::{
    json::StupidValue,
    metrics::{HandleDataErrorLabel, METRIC},
    DynError, PARAM,
};
use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::get,
    Json, Router,
};
use chrono::{Local, NaiveDateTime, NaiveTime};
use log::{info, warn};
use prometheus_client::encoding::text::encode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};

pub(crate) struct AppState {
    #[cfg(feature = "mysql")]
    pub(crate) pool: MySqlPool,
    pub(crate) client: reqwest::Client,
}

pub async fn axum_serve(app_state: AppState) -> Result<(), DynError> {
    // build our application with a route
    let app = Router::new()
        .route("/health", get(|| async { (StatusCode::OK, "OK") }))
        .route("/metrics", get(metrics_handler))
        .route("/data", get(data_handler).post(data_handler))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(Arc::new(app_state));
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", PARAM.port)).await?;
    log::info!("listening on port {}", PARAM.port);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn metrics_handler() -> (StatusCode, String) {
    let mut buffer = String::new();
    if let Err(e) = encode(&mut buffer, &crate::metrics::METRIC.prom_registry) {
        log::error!("Failed to encode metrics: {:?}", e);
    }
    (StatusCode::OK, buffer)
}

async fn data_handler(
    State(state): State<Arc<AppState>>,
    req: Option<Json<DataRequest>>,
) -> (StatusCode, HeaderMap, Json<Response<Vec<Data>>>) {
    METRIC
        .req_count
        .get_or_create(&HandleDataErrorLabel {
            some: "test".to_string(),
        })
        .inc();

    info!("req: {:?}", req);
    if let Some(Json(data)) = req {
        info!("req data: {:?}", data);
    }
    #[cfg(not(feature = "mysql"))]
    return (
        StatusCode::INTERNAL_SERVER_ERROR,
        some_headers(),
        Json(Response::error("mysql not enabled".to_string())),
    );
    #[cfg(feature = "mysql")]
    {
        let pool = state.pool.borrow();
        match sqlx::query!(r"select now() as now_local, now() as now_naive, now() as now_utc;")
            .fetch_one(pool)
            .await
        {
            Ok(row) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                some_headers(),
                Json(Response::success(vec![Data {
                    now_local: row.now_local,
                    now_naive: row.now_naive,
                    now_utc: row.now_utc,
                }])),
            ),
            Err(e) => {
                warn!("query now failed: {:?}", e);
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
struct DataRequest {
    #[serde(
        rename = "startTime",
        with = "crate::json::my_date_format_option",
        default
    )]
    pub(crate) start_time: Option<NaiveDateTime>,
    #[serde(
        rename = "endTime",
        with = "crate::json::my_date_format_option",
        default
    )]
    pub(crate) end_time: Option<NaiveDateTime>,
    #[serde(rename = "distinctCode", default)]
    pub(crate) distinct_code: StupidValue<bool>,
}

#[derive(serde::Serialize, Debug, FromRow)]
pub(crate) struct Data {
    #[serde(with = "crate::json::my_date_format")]
    pub(crate) now_local: NaiveDateTime,
    #[serde(with = "crate::json::my_date_format")]
    pub(crate) now_naive: NaiveDateTime,
    #[serde(with = "crate::json::my_date_format")]
    pub(crate) now_utc: NaiveDateTime,
}

#[derive(Serialize)]
struct Response<T: Serialize> {
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
