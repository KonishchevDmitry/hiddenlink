use std::fmt::Result;

use prometheus_client::{
    encoding::{DescriptorEncoder, EncodeLabelSet, EncodeMetric},
    metrics::counter::ConstCounter,
    metrics::histogram::{Histogram, exponential_buckets},
};

pub const TRANSPORT_LABEL: &str = "transport";
pub const CONNECTION_LABEL: &str = "connection";

pub type TransportLabels = [(&'static str, String); 1];

pub fn transport_labels(name: &str) -> TransportLabels {
    [(TRANSPORT_LABEL, name.to_owned())]
}

pub fn send_time_histogram() -> Histogram {
    Histogram::new(exponential_buckets(0.00001, 5.0, 10))
}

pub fn collect_family<M: EncodeMetric, L: EncodeLabelSet>(
    encoder: &mut DescriptorEncoder, name: &str, help: &str, labels: &L, metric: &M,
) -> Result {
    let mut family_encoder = encoder.encode_descriptor(name, help, None, metric.metric_type())?;
    let metric_encoder = family_encoder.encode_family(labels)?;
    metric.encode(metric_encoder)
}

pub fn collect_metric<M: EncodeMetric>(encoder: &mut DescriptorEncoder, name: &str, help: &str, metric: &M) -> Result {
    let metric_encoder = encoder.encode_descriptor(name, help, None, metric.metric_type())?;
    metric.encode(metric_encoder)
}

pub fn usecs_to_counter(usecs: u64) -> ConstCounter<f64> {
    ConstCounter::new(usecs as f64 / 1000000.0)
}