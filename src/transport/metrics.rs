use prometheus_client::{
    encoding::EncodeLabelSet,
    metrics::counter::Counter,
    metrics::family::Family,
};

pub struct MetricFactory;

impl MetricFactory {
    pub fn new_dropped_packets(name: &str) -> Family<Labels, Counter> {
        let mut metric = Family::<Labels, Counter>::default();
        let _ = metric.get_or_create(&Labels {transport: name.to_owned()});
        metric
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct Labels {
    pub transport: String,
}