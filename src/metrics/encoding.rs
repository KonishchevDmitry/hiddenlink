use std::fmt::Result;

use prometheus_client::{
    metrics::counter::ConstCounter,
    encoding::{DescriptorEncoder, EncodeLabelSet, EncodeMetric},
};

pub const TRANSPORT_LABEL: &str = "transport";
pub const CONNECTION_LABEL: &str = "connection";

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