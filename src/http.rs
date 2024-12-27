use std::sync::LazyLock;

use reqwest::Client;

use crate::{DynError, PARAM};

pub(crate) async fn init_http_client() -> Result<Client, DynError> {
    let client_builder = Client::builder().pool_max_idle_per_host(20);
    let proxy = &PARAM.http_proxy;
    if proxy.is_empty() {
        Ok(client_builder.build()?)
    } else {
        Ok(client_builder.proxy(reqwest::Proxy::all(proxy)?).build()?)
    }
}
