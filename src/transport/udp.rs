use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use log::{info, error};
use serde_derive::{Serialize, Deserialize};
use tokio::net::UdpSocket;
use tokio_tun::Tun;
use validator::Validate;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::util;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UdpTransportConfig {
    #[validate(length(min = 1))]
    bind_address: String,
    #[validate(length(min = 1))]
    peer_address: String,
}

pub struct UdpTransport {
    peer_address: SocketAddr,
    socket: UdpSocket,
}

impl UdpTransport {
    pub async fn new(config: &UdpTransportConfig, tun: Arc<Tun>) -> GenericResult<Arc<dyn Transport>> {
        let socket = UdpSocket::bind(&config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let peer_address = config.peer_address.parse().map_err(|_| format!(
            "Invalid peer address: {}", config.peer_address))?;

        let transport = Arc::new(UdpTransport {
            peer_address,
            socket,
        });

        info!("[{}] Listening on {}.", transport.name(), config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(tun).await
            });
        }

        Ok(transport)
    }

    async fn handle(&self, tun: Arc<Tun>) {
        let name = self.name();
        let mut buf = BytesMut::zeroed(1500 - 20 - 8); // MTU - IPv4 - UDP

        loop {
            let size = match self.socket.recv_from(&mut buf).await {
                Ok((size, _)) => {
                    if size == 0 {
                        error!("[{}] Got an empty message from the peer.", name);
                        continue;
                    }
                    size
                },
                Err(err) => {
                    error!("[{}] Failed to receive a message from the peer: {}.", name, err);
                    break;
                }
            };

            let packet = &buf[..size];
            util::trace_packet(&name, packet);

            if let Err(err) = tun.send(packet).await {
                error!("[{}] Failed to send the packet to tun device: {}.", name, err);
            }
        }
    }
}

#[async_trait]
impl Transport for UdpTransport {
    fn name(&self) -> String {
        format!("UDP transport to {}", self.peer_address)
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn send(&self, buf: &[u8]) -> EmptyResult {
        self.socket.send_to(buf, self.peer_address).await?;
        Ok(())
    }
}