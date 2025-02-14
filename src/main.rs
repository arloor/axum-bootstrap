#![deny(warnings)]

use axum_bootstrap::{util::http::init_http_client, TlsParam};

use clap::Parser;
use handler::{build_router, AppState};

mod handler;
mod metrics;
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

#[tokio::main]
pub async fn main() -> Result<(), DynError> {
    // util::env_logger::init_log();
    axum_bootstrap::init_log::tracing::init(env!("CARGO_CRATE_NAME"))?;
    log::info!("init http client...");
    let client = init_http_client(&PARAM.http_proxy).await?;

    #[cfg(feature = "mysql")]
    {
        log::info!("connecting to mysql...");
        let pool: sqlx::Pool<sqlx::MySql> = sqlx_mysql::MySqlPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
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

        axum_bootstrap::axum_serve(
            build_router(AppState { pool, client }),
            PARAM.port as u16,
            match PARAM.tls {
                true => Some(TlsParam {
                    tls: true,
                    cert: PARAM.cert.to_string(),
                    key: PARAM.key.to_string(),
                }),
                false => None,
            },
        )
        .await?;
    }

    #[cfg(not(feature = "mysql"))]
    {
        axum_bootstrap::axum_serve(
            build_router(AppState { client }),
            PARAM.port,
            match PARAM.tls {
                true => Some(TlsParam {
                    tls: true,
                    cert: PARAM.cert.to_string(),
                    key: PARAM.key.to_string(),
                }),
                false => None,
            },
        )
        .await?;
    }

    Ok(())
}
