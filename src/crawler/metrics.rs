use std::fmt;

use prometheus_client::{
    collector::Collector,
	encoding::DescriptorEncoder,
	metrics::counter::Counter,
	metrics::gauge::Gauge,
    metrics::histogram::Histogram,
};

use crate::metrics;

#[derive(Debug)]
pub struct CrawlerMetrics {
    pub(super) new_connections: Counter,
    pub(super) active_connections: Gauge,
    pub(super) request_times: Histogram,
    pub(super) errors: Counter,
}

impl CrawlerMetrics {
    pub(super) fn new() -> CrawlerMetrics {
        CrawlerMetrics {
            new_connections: Counter::default(),
            active_connections: Gauge::default(),
            request_times: Histogram::new([
                0.010, 0.025, 0.050, 0.075,
                0.100, 0.250, 0.500, 0.750,
                1.0, 2.5, 5.0, 7.5,
                10., 20., 30., 45., 60.,
            ]),
            errors: Counter::default(),
        }
    }
}

impl Collector for CrawlerMetrics {
    fn encode(&self, mut encoder: DescriptorEncoder) -> fmt::Result {
        metrics::collect_metric(&mut encoder, "crawler_new_connections", "New crawler connections counter", &self.new_connections)?;
        metrics::collect_metric(&mut encoder, "crawler_active_connections", "Currently active crawler connections", &self.active_connections)?;
        metrics::collect_metric(&mut encoder, "crawler_request_time", "Crawler request time", &self.request_times)?;
        metrics::collect_metric(&mut encoder, "crawler_errors", "Crawler errors counter", &self.errors)?;
        Ok(())
    }
}