use std::os::fd::AsRawFd;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use log::{info, warn, error};
use prometheus_client::encoding::DescriptorEncoder;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_rustls::server::TlsStream;

use crate::core::EmptyResult;
use crate::transport::{Transport, TransportDirection};
use crate::transport::http::common::{ConnectionFlags, PacketReader, PacketWriter};
use crate::transport::stat::TransportConnectionStat;
use crate::tunnel::Tunnel;
use crate::util;

pub struct HiddenlinkConnection {
    name: String,
    flags: ConnectionFlags,
    writer: PacketWriter<WriteHalf<TlsStream<TcpStream>>>,
    packet_reader: AsyncMutex<PacketReader<ReadHalf<TlsStream<TcpStream>>>>,
}

impl HiddenlinkConnection {
    pub fn new(
        name: String, flags: ConnectionFlags, preread_data: Bytes, connection: TlsStream<TcpStream>,
        stat: Arc<TransportConnectionStat>,
    ) -> HiddenlinkConnection {
        let fd = connection.as_raw_fd();
        let (read_half, write_half) = tokio::io::split(connection);

        let writer = PacketWriter::new(name.clone(), stat.clone());
        writer.replace(&name, fd, write_half);

        HiddenlinkConnection {
            name,
            flags,
            writer,
            packet_reader: AsyncMutex::new(PacketReader::new(preread_data, read_half, stat)),
        }
    }

    pub async fn handle(&self, tunnel: Arc<Tunnel>) {
        let mut packet_reader = self.packet_reader.lock().await;

        let shutdown = loop {
            let packet = match packet_reader.read().await {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    info!("[{}]: Client has closed the connection.", self.name);
                    break true;
                },
                Err(err) => {
                    warn!("[{}]: {err}.", self.name);
                    break false;
                }
            };

            util::trace_packet(&self.name, packet);
            if !self.flags.contains(ConnectionFlags::EGRESS) {
                error!("[{}] Got a packet from non-egress connection.", self.name);
                continue;
            }

            if let Err(err) = tunnel.send(packet).await {
                error!("[{}] {err}.", self.name);
            }
        };

        // FIXME(konishchev): Send random payload?
        self.writer.close(shutdown).await;
    }
}

#[async_trait]
impl Transport for HiddenlinkConnection {
    fn name(&self) -> &str {
        &self.name
    }

    fn direction(&self) -> TransportDirection {
        let mut direction = TransportDirection::empty();

        if self.flags.contains(ConnectionFlags::INGRESS) {
            direction |= TransportDirection::EGRESS;
        }

        if self.flags.contains(ConnectionFlags::EGRESS) {
            direction |= TransportDirection::INGRESS;
        }

        direction
    }

    fn connected(&self) -> bool {
        self.writer.connected()
    }

    fn ready_for_sending(&self) -> bool {
        self.flags.contains(ConnectionFlags::INGRESS) && self.writer.ready_for_sending()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        // Connection stat is collected by the server and preserved between reconnects
        self.writer.collect(encoder)
    }

    async fn send(&self, packet: &mut [u8]) -> EmptyResult {
        self.writer.send(packet).await
    }
}