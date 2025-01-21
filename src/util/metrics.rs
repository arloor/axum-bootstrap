use std::sync::LazyLock;

use prometheus_client::{
    encoding::EncodeLabelSet,
    metrics::{counter::Counter, family::Family},
    registry::Registry,
};

pub static METRIC: LazyLock<Metrics> = LazyLock::new(|| {
    let mut prom_registry = Registry::default();
    let req_count = Family::<HandleDataErrorLabel, Counter>::default();
    prom_registry.register("req_count", "help", req_count.clone());
    Metrics {
        prom_registry,
        req_count,
    }
});

pub struct Metrics {
    pub prom_registry: Registry,
    pub req_count: Family<HandleDataErrorLabel, Counter>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HandleDataErrorLabel {
    pub some: String,
}
