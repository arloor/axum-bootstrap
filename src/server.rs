// #![allow(clippy::unwrap_used)]
use std::{borrow::Borrow, sync::Arc, time::Duration};

use crate::{
    acceptor::{self, create_dual_stack_listener, rust_tls_acceptor, tls_config},
    json::StupidValue,
    metrics::{HandleDataErrorLabel, METRIC},
    DynError, PARAM,
};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::get,
    Json, Router,
};
use axum_macros::debug_handler;
use chrono::{Local, NaiveDateTime, NaiveTime};
use futures_util::pin_mut;
use log::{error, info, warn};
use prometheus_client::encoding::text::encode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tower_service::Service;

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
        .layer((
            TraceLayer::new_for_http(),
            tower_http::cors::CorsLayer::permissive(),
            // Graceful shutdown will wait for outstanding requests to complete. Add a timeout so
            // requests don't hang forever.
            TimeoutLayer::new(Duration::from_secs(10)),
        ))
        .with_state(Arc::new(app_state));
    match PARAM.tls {
        true => {
            use hyper::body::Incoming;
            use hyper_util::rt::{TokioExecutor, TokioIo};
            let tls_acceptor = rust_tls_acceptor(&PARAM.key, &PARAM.cert)?;
            let tcp_listener = create_dual_stack_listener(PARAM.port as u16).await?;
            pin_mut!(tcp_listener);
            loop {
                let tower_service = app.clone();
                let tls_acceptor = tls_acceptor.clone();

                // Wait for new tcp connection
                let (cnx, addr) = tcp_listener.accept().await.unwrap();

                tokio::spawn(async move {
                    // Wait for tls handshake to happen
                    let Ok(stream) = tls_acceptor.accept(cnx).await else {
                        error!("error during tls handshake connection from {}", addr);
                        return;
                    };

                    // Hyper has its own `AsyncRead` and `AsyncWrite` traits and doesn't use tokio.
                    // `TokioIo` converts between them.
                    let stream = TokioIo::new(stream);

                    // Hyper also has its own `Service` trait and doesn't use tower. We can use
                    // `hyper::service::service_fn` to create a hyper `Service` that calls our app through
                    // `tower::Service::call`.
                    let hyper_service =
                        hyper::service::service_fn(move |request: Request<Incoming>| {
                            // We have to clone `tower_service` because hyper's `Service` uses `&self` whereas
                            // tower's `Service` requires `&mut self`.
                            //
                            // We don't need to call `poll_ready` since `Router` is always ready.
                            tower_service.clone().call(request)
                        });

                    let ret = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(stream, hyper_service)
                        .await;

                    if let Err(err) = ret {
                        warn!("error serving connection from {}: {}", addr, err);
                    }
                });
            }
        }
        false => {
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", PARAM.port)).await?;
            log::info!("listening on port {}", PARAM.port);
            axum::serve(listener, app).await?;
        }
    }
    Ok(())
}

async fn metrics_handler() -> (StatusCode, String) {
    let mut buffer = String::new();
    if let Err(e) = encode(&mut buffer, &crate::metrics::METRIC.prom_registry) {
        log::error!("Failed to encode metrics: {:?}", e);
    }
    (StatusCode::OK, buffer)
}

#[debug_handler]
async fn data_handler(
    State(state): State<Arc<AppState>>,
    req: Json<DataRequest>,
) -> (StatusCode, HeaderMap, Json<Response<Vec<Data>>>) {
    METRIC
        .req_count
        .get_or_create(&HandleDataErrorLabel {
            some: "test".to_string(),
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
