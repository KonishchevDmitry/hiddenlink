use std::collections::{HashMap, hash_map::Entry};
use std::sync::Arc;

use crate::transport::Transport;

pub struct TransportConnections<T: Transport + ?Sized> {
    connections: HashMap<String, Arc<T>>,
}

impl<T: Transport + ?Sized> TransportConnections<T> {
    pub fn new() -> TransportConnections<T> {
        TransportConnections {
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

    pub fn select(&self, _packet: &[u8]) -> Option<Arc<T>> {
        let mut ready_connections = Vec::with_capacity(self.connections.len());

        for connection in self.connections.values() {
            if connection.ready_for_sending() {
                ready_connections.push(connection);
            }
        }

        if ready_connections.len() <= 1 {
            return ready_connections.first().map(|&connection| connection.clone());
        }

        // FIXME(konishchev): Implement
        return ready_connections.first().map(|&connection| connection.clone());
    }
}

impl<T: Transport + ?Sized> Default for TransportConnections<T> {
    fn default() -> Self {
        TransportConnections::new()
    }
}