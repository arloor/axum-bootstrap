use reqwest::Client;

use crate::DynError;

pub async fn init_http_client(http_proxy: &str) -> Result<Client, DynError> {
    let client_builder = Client::builder().pool_max_idle_per_host(20);
    if http_proxy.is_empty() {
        Ok(client_builder.build()?)
    } else {
        Ok(client_builder.proxy(reqwest::Proxy::all(http_proxy)?).build()?)
    }
}
