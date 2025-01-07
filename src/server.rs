// #![allow(clippy::unwrap_used)]
use std::{borrow::Borrow, sync::Arc, time::Duration};

use crate::{
    acceptor::{self, create_dual_stack_listener, rust_tls_acceptor, tls_config, TlsAcceptor},
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
use tokio::{sync::broadcast, time};
use tokio_rustls::rustls::ServerConfig;
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tower_service::Service;

pub(crate) struct AppState {
    #[cfg(feature = "mysql")]
    pub(crate) pool: MySqlPool,
    pub(crate) client: reqwest::Client,
}

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);

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
    log::info!("listening on port {}, use_tls: {}", PARAM.port, PARAM.tls);
    match PARAM.tls {
        true => serve_tls(&app).await?,
        false => axum::serve(create_dual_stack_listener(PARAM.port as u16).await?, app).await?,
    }
    Ok(())
}

async fn serve_tls(app: &Router) -> Result<(), DynError> {
    let (tx, _rx) = broadcast::channel::<Arc<ServerConfig>>(10);
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        info!("update tls config every {:?}", REFRESH_INTERVAL);
        loop {
            time::sleep(REFRESH_INTERVAL).await;
            if let Ok(new_acceptor) = tls_config(&PARAM.key, &PARAM.cert) {
                info!("update tls config");
                if let Err(e) = tx.send(new_acceptor) {
                    warn!("send tls config error:{}", e);
                }
            }
        }
    });
    let mut rx = tx_clone.subscribe();
    use hyper::body::Incoming;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    let mut acceptor = TlsAcceptor::new(
        tls_config(&PARAM.key, &PARAM.cert)?,
        create_dual_stack_listener(PARAM.port as u16).await?,
    );
    pin_mut!(acceptor);
    loop {
        tokio::select! {
            message = rx.recv() => {
                #[allow(clippy::expect_used)]
                let new_config = message.expect("Channel should not be closed");
                // Replace the acceptor with the new one
                acceptor.replace_config(new_config);
                info!("replaced tls config");
            }
            conn = acceptor.accept() => {
                match conn {
                    Ok((conn,client_socket_addr)) => {
                        let tower_service = app.clone();
                        tokio::spawn(async move {
                            // Hyper has its own `AsyncRead` and `AsyncWrite` traits and doesn't use tokio.
                            // `TokioIo` converts between them.
                            let stream = TokioIo::new(conn);

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

                            let ret =
                                hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                                    .serve_connection_with_upgrades(stream, hyper_service)
                                    .await;

                            if let Err(err) = ret {
                                warn!("error serving connection from {}: {}", client_socket_addr, err);
                            }
                        });
                    }
                    Err(err) => {
                        warn!("Error accepting connection: {}", err);
                    }
                }
            }
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
