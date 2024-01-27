use std::os::fd::AsRawFd;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use log::{info, warn, error};
use prometheus_client::encoding::DescriptorEncoder;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_rustls::server::TlsStream;

use crate::core::EmptyResult;
use crate::transport::{Transport, TransportConnectionStat};
use crate::transport::http::common::{ConnectionFlags, PacketReader, PacketWriter};
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
        writer.replace(fd, write_half);

        HiddenlinkConnection {
            name,
            flags,
            writer,
            packet_reader: AsyncMutex::new(PacketReader::new(preread_data, read_half, stat)),
        }
    }

    pub async fn handle(&self, tunnel: Arc<Tunnel>) {
        let mut packet_reader = self.packet_reader.lock().await;

        loop {
            let packet = match packet_reader.read().await {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    info!("[{}]: Client has closed the connection.", self.name);
                    break;
                },
                Err(err) => {
                    warn!("[{}]: {err}.", self.name);
                    return;
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
        }

        // FIXME(konishchev): Shutdown
    }
}

#[async_trait]
impl Transport for HiddenlinkConnection {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        self.flags.contains(ConnectionFlags::INGRESS) && self.writer.is_ready()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        // Connection stat is collected by the server and preserved between reconnects
        self.writer.collect(encoder)
    }

    async fn send(&self, packet: &mut BytesMut) -> EmptyResult {
        self.writer.send(packet).await
    }
}