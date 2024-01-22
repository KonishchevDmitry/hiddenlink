mod shadowsocks;

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use log::{info, error};
use prometheus_client::encoding::DescriptorEncoder;
use serde_derive::{Serialize, Deserialize};
use tokio::net::UdpSocket;
use validator::Validate;

use crate::constants;
use crate::core::{GenericResult, EmptyResult};
use crate::metrics::{self, TransportLabels};
use crate::transport::{Transport, MeteredTransport, TransportConnectionStat};
use crate::tunnel::Tunnel;
use crate::util;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UdpTransportConfig {
    bind_address: SocketAddr,
    peer_address: SocketAddr,
}

pub struct UdpTransport {
    name: String,
    labels: TransportLabels,
    socket: UdpSocket,
    peer_address: SocketAddr,
    stat: TransportConnectionStat,
}

impl UdpTransport {
    pub async fn new(name: String, config: &UdpTransportConfig, tunnel: Arc<Tunnel>) -> GenericResult<Arc<dyn MeteredTransport>> {
        let socket = UdpSocket::bind(&config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let labels = metrics::transport_labels(&name);
        let stat = TransportConnectionStat::new(&name, &name);

        let transport = Arc::new(UdpTransport {
            name,
            labels,
            peer_address: config.peer_address,
            socket,
            stat,
        });

        info!("[{}] Listening on {}.", transport.name, config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(tunnel).await
            });
        }

        Ok(transport)
    }

    async fn handle(&self, tunnel: Arc<Tunnel>) {
        let max_size = constants::MTU - constants::IPV4_HEADER_SIZE - constants::UDP_HEADER_SIZE;
        let mut buf = BytesMut::zeroed(max_size + 1);

        loop {
            let size = match self.socket.recv_from(&mut buf).await {
                Ok((size, _)) => {
                    if size == 0 {
                        error!("[{}] Got an empty message from the peer.", self.name);
                        continue;
                    } else if size > max_size {
                        // tokio doesn't support MSG_TRUNC yet, so handle it manually
                        error!("[{}] Got a too big message from the peer. Drop it.", self.name);
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
            self.stat.on_packet_received(packet);
            util::trace_packet(&self.name, packet);

            if let Err(err) = tunnel.send(packet).await {
                error!("[{}] {}.", self.name, err);
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

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        self.stat.collect(encoder)?;
        self.stat.collect_udp_socket(encoder, &self.socket)
    }

    async fn send(&self, buf: &[u8]) -> EmptyResult {
        self.socket.send_to(buf, self.peer_address).await.map_err(|e| {
            self.stat.on_packet_dropped();
            e
        })?;

        self.stat.on_packet_sent(buf);
        Ok(())
    }
}

impl MeteredTransport for UdpTransport {
    fn labels(&self) -> &TransportLabels {
        &self.labels
    }
}