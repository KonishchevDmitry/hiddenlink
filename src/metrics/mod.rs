mod encoding;
pub mod server;

use std::sync::Arc;

use prometheus_client::{
    encoding::DescriptorEncoder,
    collector::Collector,
};

pub use encoding::*;

#[derive(Debug)]
pub struct ArcCollector<T: Collector>(Arc<T>);

impl<T: Collector> ArcCollector<T> {
    pub fn new(collector: Arc<T>) -> Box<ArcCollector<T>> {
        Box::new(ArcCollector(collector))
    }
}

impl<T: Collector> Collector for ArcCollector<T> {
    fn encode(&self, encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        self.0.encode(encoder)
    }
}