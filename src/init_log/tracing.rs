use time::macros::format_description;
use time::UtcOffset;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::DynError;
pub fn init(env_cargo_crate_name: &str) -> Result<(), DynError> {
    let offset = UtcOffset::current_local_offset()?;
    let timer = OffsetTime::new(offset, format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| format!("{env_cargo_crate_name}=debug,tower_http=error").into()),
        )
        .with(
            tracing_subscriber::fmt::layer().with_thread_ids(true).with_ansi(true).with_timer(timer),
            // .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339()), // 需要tracing-subscriber的local-time feature
            // .with_timer(tracing_subscriber::fmt::time::OffsetTime::local_rfc_3339().expect("could not get local offset!")), // 这个需要放在tokio runtime fork thread之前
        )
        .init();
    Ok(())
}
