use std::{collections::HashMap, fs::File, io::Read, sync::Arc, time::Duration};

use axum::{
    extract::{Query, State},
    http::HeaderValue,
    routing::get,
    Json, Router,
};
use axum_macros::debug_handler;
use chrono::NaiveDateTime;
use hyper::{HeaderMap, StatusCode};
use log::{info, warn};
use prometheus_client::encoding::text::encode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer, trace::TraceLayer,
};

use crate::{
    util::{
        json::StupidValue,
        metrics::{HandleDataErrorLabel, METRIC},
    },
    DynError, PARAM,
};

pub(crate) struct AppState {
    #[cfg(feature = "mysql")]
    pub(crate) pool: sqlx::MySqlPool,
    pub(crate) client: reqwest::Client,
}

pub(crate) fn build_router(app_state: AppState) -> Router {
    // build our application with a route
    Router::new()
        .route("/", get(|| async { (StatusCode::OK, "OK") }))
        .route("/surge", get(surge))
        .route("/clash", get(clash))
        .route("/metrics", get(metrics_handler))
        .route("/data", get(data_handler).post(data_handler))
        .layer((
            TraceLayer::new_for_http(),
            CorsLayer::permissive(),
            TimeoutLayer::new(Duration::from_secs(10)),
            CompressionLayer::new(),
        ))
        .with_state(Arc::new(app_state))
}

pub(crate) async fn metrics_handler() -> (StatusCode, String) {
    let mut buffer = String::new();
    if let Err(e) = encode(&mut buffer, &METRIC.prom_registry) {
        log::error!("Failed to encode metrics: {:?}", e);
    }
    (StatusCode::OK, buffer)
}

#[debug_handler]
pub(crate) async fn data_handler(
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
        with = "crate::util::json::my_date_format_option",
        default
    )]
    pub(crate) start_time: Option<NaiveDateTime>,
    #[serde(
        rename = "endTime",
        with = "crate::util::json::my_date_format_option",
        default
    )]
    pub(crate) end_time: Option<NaiveDateTime>,
    #[serde(rename = "distinctCode", default)]
    pub(crate) distinct_code: StupidValue<bool>,
}

#[derive(serde::Serialize, Debug, FromRow)]
pub(crate) struct Data {
    #[serde(with = "crate::util::json::my_date_format")]
    pub(crate) now_local: NaiveDateTime,
    #[serde(with = "crate::util::json::my_date_format")]
    pub(crate) now_naive: NaiveDateTime,
    #[serde(with = "crate::util::json::my_date_format")]
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

#[derive(Deserialize)]
struct ReqQuery {
    sub: Option<String>,
}

async fn surge(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReqQuery>,
) -> (StatusCode, HeaderMap, String) {
    if params.sub.is_none(){
        info!("surge sub is none, use the cmd line args");
    }
    let sub = params.sub.unwrap_or(PARAM.surge_sub_url.clone());
    return merge_surge(&state.client, &sub)
        .await
        .map(|res| (StatusCode::OK, some_headers(), res))
        .unwrap_or_else(|e| {
            warn!("merge failed: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                some_headers(),
                e.to_string(),
            )
        });
}

async fn clash(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReqQuery>,
) -> (StatusCode, HeaderMap, String) {
    if params.sub.is_none(){
        info!("clash sub is none, use the cmd line args");
    }
    let sub = params.sub.unwrap_or(PARAM.clash_sub_url.clone());
    return merge_clash(&state.client, &sub)
        .await
        .map(|merged_yaml_string| (StatusCode::OK, some_headers(), merged_yaml_string))
        .unwrap_or_else(|e| {
            warn!("merge failed: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                some_headers(),
                e.to_string(),
            )
        });
}

async fn merge_surge(client: &Client, sub: &str) -> Result<String, DynError> {
    let proxy_info = parse_surge_proxy_info(client, sub).await?;
    let mut mac_conf = File::open(&PARAM.surge_base_conf).map_err(|_| "open private key failed")?;
    let mut buf = String::new();
    mac_conf.read_to_string(&mut buf)?;
    // 分割配置文件成不同段落
    let mut sections: HashMap<&str, String> = HashMap::new();
    let mut section_order = vec![];
    let mut current_section = "";
    let mut section_content = String::new();

    for line in buf.lines() {
        if line.starts_with('[') && line.ends_with(']') {
            if !current_section.is_empty() {
                sections.insert(current_section, section_content.trim().to_string());
                section_order.push(current_section);
            }
            current_section = line.trim_matches(|c| c == '[' || c == ']');
            section_content = String::new();
            section_content.push_str(line);
            section_content.push('\n');
        } else {
            section_content.push_str(line);
            section_content.push('\n');
        }
    }
    if !current_section.is_empty() {
        sections.insert(current_section, section_content.to_string());
    }

    // 处理 Proxy 段落
    let mut proxy_section = sections
        .get("Proxy")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "[Proxy]\n".to_string());
    proxy_section.push('\n');
    for (name, config) in &proxy_info {
        proxy_section.push_str(&format!("{} = {}\n", name, config));
    }
    sections.insert("Proxy", proxy_section);

    // 处理 Proxy Group 段落
    if let Some(mut group_section) = sections.get("Proxy Group").map(|s| s.to_string()) {
        let mut new_group_section = String::new();
        for line in group_section.lines() {
            if line.contains('=') {
                let parts: Vec<&str> = line.splitn(2, '=').collect();
                let group_name = parts[0].trim();
                let mut group_config = parts[1].trim().to_owned();

                // 在现有代理列表的末尾添加新的代理
                if !group_config.is_empty() {
                    let mut config_parts: Vec<&str> =
                        group_config.split(',').map(|s| s.trim()).collect();
                    for (name, _) in &proxy_info {
                        if !config_parts.contains(&name.as_str()) {
                            config_parts.push(name);
                        }
                    }
                    group_config = config_parts.join(", ");
                }

                new_group_section.push_str(&format!("{} = {}\n", group_name, group_config));
            } else {
                new_group_section.push_str(line);
                new_group_section.push('\n');
            }
        }
        sections.insert("Proxy Group", new_group_section);
    }

    // 重新组装配置文件
    let mut result = String::new();

    for &section in &section_order {
        if let Some(content) = sections.get(section) {
            result.push_str(content);
            result.push_str("\n\n");
        }
    }

    Ok(result)
}

async fn parse_surge_proxy_info(
    client: &Client,
    sub: &str,
) -> Result<Vec<(String, String)>, DynError> {
    info!("merge_surge sub: {}", sub);
    let sub_res = client.get(sub).send().await?.text().await?;
    let mut proxy_info = Vec::<(String, String)>::new();
    let mut split = sub_res.split("[Proxy]");
    split.next().ok_or("No [Proxy] section")?;
    let proxy_str = split.next().ok_or("No proxies")?;
    for ele in proxy_str.lines() {
        if ele.is_empty() {
            continue;
        }
        if ele.starts_with("[") {
            break;
        }
        let mut index = ele.find("=").ok_or("no =")?;
        let (k, v) = ele.split_at(index);
        let v = v.split_at(1).1;
        proxy_info.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    Ok(proxy_info)
}

async fn merge_clash(client: &Client, sub: &str) -> Result<String, Box<dyn std::error::Error>> {
    info!("merge_clash sub: {}", sub);
    let sub_res = client.get(sub).send().await?.text().await?;
    let sub_yaml = serde_yaml::from_str::<serde_yaml::Value>(&sub_res)?;
    let sub_proxies = sub_yaml
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .ok_or("No proxies field")?;

    let base = &PARAM.clash_base_url;
    let clash_base = client.get(base).send().await?.text().await?;
    let mut base_yaml = serde_yaml::from_str::<serde_yaml::Value>(&clash_base)?;
    let base_proxies = base_yaml
        .get_mut("proxies")
        .and_then(|p| p.as_sequence_mut())
        .ok_or("No base_proxies")?;
    base_proxies.extend(sub_proxies.clone());

    if let Some(groups) = base_yaml
        .get_mut("proxy-groups")
        .and_then(|g| g.as_sequence_mut())
    {
        for item in sub_proxies {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                for group in &mut *groups {
                    if let Some(ps) = group.get_mut("proxies").and_then(|p| p.as_sequence_mut()) {
                        ps.push(serde_yaml::Value::String(name.to_string()));
                    }
                }
            }
        }
    }

    let merged_yaml_string = serde_yaml::to_string(&base_yaml)?;
    Ok(merged_yaml_string)
}
