use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use log::{info, warn, error};
use prometheus_client::encoding::DescriptorEncoder;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::server::TlsStream;
use tokio_tun::Tun;

use crate::core::EmptyResult;
use crate::transport::Transport;
use crate::transport::http::common::{ConnectionFlags, PacketReader};
use crate::util;

pub struct HiddenlinkConnection {
    name: String,
    flags: ConnectionFlags,
    writer: Mutex<WriteHalf<TlsStream<TcpStream>>>,
    packet_reader: Mutex<PacketReader<ReadHalf<TlsStream<TcpStream>>>>,
}

impl HiddenlinkConnection {
    pub fn new(name: String, flags: ConnectionFlags, preread_data: Bytes, connection: TlsStream<TcpStream>) -> HiddenlinkConnection {
        let (reader, writer) = tokio::io::split(connection);

        HiddenlinkConnection {
            name,
            flags,
            writer: Mutex::new(writer),
            packet_reader:Mutex::new(PacketReader::new(preread_data, reader)),
        }
    }

    pub async fn handle(&self, tun: Arc<Tun>) {
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

            if !self.flags.contains(ConnectionFlags::EGRESS) {
                error!("[{}] Got a packet from non-egress connection.", self.name);
                continue;
            }

            util::trace_packet(&self.name, packet);

            if let Err(err) = tun.send(packet).await {
                error!("[{}] Failed to send packet to tun device: {err}.", self.name);
            }
        }

        // FIXME(konishchev): Shutdown
    }
}

#[async_trait] // FIXME(konishchev): Deprecate it
impl Transport for HiddenlinkConnection {
    fn name(&self) -> &str {
        &self.name
    }

    // FIXME(konishchev): Implement
    // FIXME(konishchev): Check socket buffers?
    fn is_ready(&self) -> bool {
        self.flags.contains(ConnectionFlags::INGRESS)
    }

    // FIXME(konishchev): Implement
    fn collect(&self, _encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        Ok(())
    }

    // FIXME(konishchev): Implement
    // FIXME(konishchev): https://docs.rs/tokio-util/0.7.10/tokio_util/sync/struct.CancellationToken.html on error?
    async fn send(&self, packet: &[u8]) -> EmptyResult {
        let mut writer = self.writer.lock().await;
        Ok(writer.write_all(packet).await?)
    }
}