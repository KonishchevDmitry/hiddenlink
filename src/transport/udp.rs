use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use log::{info, error};
use serde_derive::{Serialize, Deserialize};
use tokio::net::UdpSocket;
use tokio_tun::Tun;
use validator::Validate;

use crate::constants;
use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::util;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UdpTransportConfig {
    bind_address: SocketAddr,
    peer_address: SocketAddr,
}

pub struct UdpTransport {
    name: String,
    socket: UdpSocket,
    peer_address: SocketAddr,
}

impl UdpTransport {
    pub async fn new(config: &UdpTransportConfig, tun: Arc<Tun>) -> GenericResult<Arc<dyn Transport>> {
        let socket = UdpSocket::bind(&config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let transport = Arc::new(UdpTransport {
            name: format!("UDP transport to {}", config.peer_address),
            peer_address: config.peer_address,
            socket,
        });

        info!("[{}] Listening on {}.", transport.name, config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(tun).await
            });
        }

        Ok(transport)
    }

    async fn handle(&self, tun: Arc<Tun>) {
        let mut buf = BytesMut::zeroed(constants::MTU - constants::IPV4_HEADER_SIZE - constants::UDP_HEADER_SIZE);

        loop {
            let size = match self.socket.recv_from(&mut buf).await {
                Ok((size, _)) => {
                    if size == 0 {
                        error!("[{}] Got an empty message from the peer.", self.name);
                        continue;
                    }
                    size
                },
                Err(err) => {
                    error!("[{}] Failed to receive a message from the peer: {}.", self.name, err);
                    break;
                }
            };

            let packet = &buf[..size];
            util::trace_packet(&self.name, packet);

            if let Err(err) = tun.send(packet).await {
                error!("[{}] Failed to send packet to tun device: {}.", self.name, err);
            }
        }
    }
}

#[async_trait]
impl Transport for UdpTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn send(&self, buf: &[u8]) -> EmptyResult {
        self.socket.send_to(buf, self.peer_address).await?;
        Ok(())
    }
}