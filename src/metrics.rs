use std::sync::LazyLock;

use prometheus_client::{
    encoding::EncodeLabelSet,
    metrics::{counter::Counter, family::Family},
    registry::Registry,
};

pub(crate) static METRIC: LazyLock<Metrics> = LazyLock::new(|| {
    let mut prom_registry = Registry::default();
    let req_count = Family::<HttpReqLabel, Counter>::default();
    prom_registry.register("req_count", "help", req_count.clone());
    Metrics { prom_registry, req_count }
});

pub(crate) struct Metrics {
    pub(crate) prom_registry: Registry,
    pub(crate) req_count: Family<HttpReqLabel, Counter>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct HttpReqLabel {
    pub(crate) path: String,
}
