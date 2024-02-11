use std::sync::Arc;

use async_trait::async_trait;
use bitflags::bitflags;
use prometheus_client::encoding::DescriptorEncoder;
use rand::Rng;

use crate::core::EmptyResult;
use crate::metrics::TransportLabels;

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

pub type TransportWeight = u16;

pub struct WeightedTransports<T: Transport + ?Sized> {
    transports: Vec<WeightedTransport<T>>,
}

impl<T: Transport + ?Sized> WeightedTransports<T> {
    pub fn new() -> WeightedTransports<T> {
        WeightedTransports {
            transports: Vec::new(),
        }
    }

    pub fn add(&mut self, transport: Arc<T>, min_weight: TransportWeight, max_weight: TransportWeight) -> &WeightedTransport<T> {
        self.transports.push(WeightedTransport {
            transport,
            weight: rand::thread_rng().gen_range(min_weight..=max_weight),
        });
        self.transports.last().unwrap()
    }

    pub fn remove(&mut self, transport: Arc<T>) {
        self.transports.swap_remove(self.transports.iter().position(|weighted| {
            Arc::ptr_eq(&weighted.transport, &transport)
        }).unwrap());
    }

    pub fn len(&self) -> usize {
        self.transports.len()
    }

    pub fn ready_for_sending(&self) -> bool {
        self.transports.iter().any(|weighted| weighted.transport.ready_for_sending())
    }

    pub fn iter(&self) -> impl Iterator<Item=&WeightedTransport<T>> {
        self.transports.iter()
    }

    // FIXME(konishchev): Rewrite
    pub fn select(&self) -> Option<Arc<T>> {
        let mut ready_transports = Vec::with_capacity(self.transports.len());
        let mut total_weight = 0u32;

        for transport in &self.transports {
            if transport.transport.ready_for_sending() {
                ready_transports.push(transport);
                total_weight += u32::from(transport.weight);
            }
        }

        if ready_transports.is_empty() {
            return None;
        }

        let mut current_weight = 0u32;
        let selected_weight = rand::thread_rng().gen_range(0..total_weight);

        for transport in ready_transports {
            current_weight += u32::from(transport.weight);
            if current_weight > selected_weight {
                return Some(transport.transport.clone());
            }
        }

        unreachable!();
    }
}

impl<T: Transport + ?Sized> Default for WeightedTransports<T> {
    fn default() -> Self {
        WeightedTransports::new()
    }
}

pub struct WeightedTransport<T: Transport + ?Sized> {
    pub transport: Arc<T>,
    pub weight: TransportWeight,
}

pub fn default_transport_weight() -> u16 {
    100
}