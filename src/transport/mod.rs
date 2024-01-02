use std::sync::Arc;

use async_trait::async_trait;
use rand::Rng;

use crate::core::EmptyResult;

pub mod http;
pub mod udp;

#[async_trait]
pub trait Transport: Send + Sync {
    fn name(&self) -> &str;
    fn is_ready(&self) -> bool;
    async fn send(&self, packet: &[u8]) -> EmptyResult;
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

    pub fn is_ready(&self) -> bool {
        self.transports.iter().any(|weighted| weighted.transport.is_ready())
    }

    // FIXME(konishchev): Rewrite
    pub fn select(&self) -> Option<Arc<T>> {
        let mut ready_transports = Vec::with_capacity(self.transports.len());
        let mut total_weight = 0u32;

        for transport in &self.transports {
            if transport.transport.is_ready() {
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

pub struct WeightedTransport<T: Transport + ?Sized> {
    pub transport: Arc<T>,
    pub weight: TransportWeight,
}

pub fn default_transport_weight() -> u16 {
    100
}