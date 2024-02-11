use std::collections::{HashMap, hash_map::Entry};
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;

use log::error;

use crate::transport::Transport;
use crate::util;

pub struct TransportConnections<T: Transport + ?Sized> {
    random_state: RandomState,
    connections: HashMap<String, Arc<T>>,
}

impl<T: Transport + ?Sized> TransportConnections<T> {
    pub fn new() -> TransportConnections<T> {
        TransportConnections {
            random_state: RandomState::new(),
            connections: HashMap::new(),
        }
    }

    pub fn add(&mut self, connection: Arc<T>) -> bool {
        self.connections.insert(connection.name().to_owned(), connection).is_none()
    }

    pub fn remove(&mut self, connection: Arc<T>) {
        if let Entry::Occupied(entry) = self.connections.entry(connection.name().to_owned()) {
            if Arc::ptr_eq(entry.get(), &connection) {
                entry.remove();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.connections.len()
    }

    pub fn ready_for_sending(&self) -> bool {
        self.connections.values().any(|connection| connection.ready_for_sending())
    }

    pub fn iter(&self) -> impl Iterator<Item=&Arc<T>> {
        self.connections.values()
    }

    pub fn select(&self, packet: &[u8]) -> Option<Arc<T>> {
        let mut ready_connections = Vec::with_capacity(self.connections.len());

        for connection in self.connections.values() {
            if connection.ready_for_sending() {
                ready_connections.push(connection);
            }
        }

        if ready_connections.len() <= 1 {
            return ready_connections.first().map(|&connection| connection.clone());
        }

        let addresses = match packet[0] >> 4 {
            // IPv4: source + destination addresses
            4 if packet.len() >= 20 => &packet[12..20],

            // IPv6: source + destination addresses
            6 if packet.len() >= 40 => &packet[8..40],

            _ => {
                error!("Got an invalid IP packet: {}.", util::format_packet(packet));
                return Some(ready_connections[0].clone())
            },
        };

        let hash = self.random_state.hash_one(addresses);
        let index = hash % ready_connections.len() as u64;
        Some(ready_connections[index as usize].clone())
    }
}

impl<T: Transport + ?Sized> Default for TransportConnections<T> {
    fn default() -> Self {
        TransportConnections::new()
    }
}