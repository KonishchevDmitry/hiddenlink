use std::fmt;

use prometheus_client::{
    collector::Collector,
	encoding::DescriptorEncoder,
	metrics::counter::Counter,
    metrics::histogram::Histogram,
};

use crate::metrics;

#[derive(Debug)]
pub struct CrawlerMetrics {
    pub(super) errors: Counter,
    pub(super) request_times: Histogram,
}

impl CrawlerMetrics {
    pub(super) fn new() -> CrawlerMetrics {
        CrawlerMetrics {
            errors: Counter::default(),
            request_times: Histogram::new([
                0.010, 0.025, 0.050, 0.075,
                0.100, 0.250, 0.500, 0.750,
                1.0, 2.5, 5.0, 7.5,
                10., 20., 30., 45., 60.,
            ]),
        }
    }
}

impl Collector for CrawlerMetrics {
    fn encode(&self, mut encoder: DescriptorEncoder) -> fmt::Result {
        metrics::collect_metric(&mut encoder, "crawler_errors", "Total crawler errors", &self.errors)?;
        metrics::collect_metric(&mut encoder, "crawler_request_time", "Crawler request time", &self.request_times)?;
        Ok(())
    }
}