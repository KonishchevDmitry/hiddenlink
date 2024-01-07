use std::fmt::Result;

use prometheus_client::{
    metrics::counter::{Counter, ConstCounter},
    encoding::{DescriptorEncoder, EncodeLabelSet, EncodeMetric},
};

pub const TRANSPORT_LABEL: &str = "transport";

pub fn collect_dropped_packets(encoder: &mut DescriptorEncoder, transport: &str, counter: &Counter) -> Result {
    collect_family(
        encoder, "dropped_packets", "Dropped packets count",
        &[(TRANSPORT_LABEL, transport)], counter)
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