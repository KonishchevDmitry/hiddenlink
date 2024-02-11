use async_trait::async_trait;
use bitflags::bitflags;
use prometheus_client::encoding::DescriptorEncoder;

use crate::core::EmptyResult;
use crate::metrics::TransportLabels;

pub mod connections;
pub mod http;
pub mod stat;
pub mod udp;

bitflags! {
    #[derive(Clone, Copy)]
    pub struct TransportDirection: usize {
        const INGRESS = 1 << 0;
        const EGRESS  = 1 << 1;
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    fn name(&self) -> &str;
    fn direction(&self) -> TransportDirection;
    fn connected(&self) -> bool;
    fn ready_for_sending(&self) -> bool;
    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result;
    async fn send(&self, packet: &mut [u8]) -> EmptyResult;
}

pub trait MeteredTransport: Transport {
    fn labels(&self) -> &TransportLabels;
}