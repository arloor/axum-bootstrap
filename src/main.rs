#![deny(warnings)]
#![allow(unused)]
use std::time::Duration;

use clap::Parser;
use handler::AppState;
use util::http::init_http_client;

mod handler;
mod server;
mod util;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// axum脚手架
#[derive(Parser)]
#[command(author, version=None, about, long_about = None)]
pub struct Param {
    #[arg(long, short, value_name = "port", default_value = "4000")]
    port: i16,
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
    util::env_logger::init_log();
    handle_signal()?;
    log::info!("init http client...");
    let client = init_http_client().await?;
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
        server::axum_serve(AppState {
            #[cfg(feature = "mysql")]
            pool,
            client,
        })
        .await?;
    }
    #[cfg(not(feature = "mysql"))]
    {
        server::axum_serve(AppState { client }).await?;
    }

    Ok(())
}

#[cfg(unix)]
fn handle_signal() -> Result<(), DynError> {
    use log::info;
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate_signal = signal(SignalKind::terminate())?;
    tokio::spawn(async move {
        tokio::select! {
            _ = terminate_signal.recv() => {
                info!("receive terminate signal, exit");
                std::process::exit(0);
            },
            _ = tokio::signal::ctrl_c() => {
                info!("ctrl_c => shutdowning");
                std::process::exit(0); // 并不优雅关闭
            },
        };
    });
    Ok(())
}

#[cfg(windows)]
fn handle_signal() -> Result<(), DynError> {
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("ctrl_c => shutdowning");
        std::process::exit(0); // 并不优雅关闭
    });
    Ok(())
}
