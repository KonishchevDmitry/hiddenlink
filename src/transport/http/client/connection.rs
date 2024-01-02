use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use log::{trace, info, warn, error};
use prometheus_client::encoding::{CounterValueEncoder, DescriptorEncoder, EncodeMetric};
use prometheus_client::metrics::gauge::ConstGauge;
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::time;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_tun::Tun;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::transport::http::common::{self, ConnectionFlags, PacketReader, PacketWriter, pre_configure_hiddenlink_socket,
    post_configure_hiddenlink_socket};
use crate::transport::metrics::Labels;
use crate::util;

pub struct ConnectionConfig {
    pub endpoint: String,
    pub domain: DnsName<'static>,
    pub client_config: Arc<ClientConfig>,
    pub flags: ConnectionFlags,
    pub secret: String,
}

pub struct Connection {
    name: String,
    config: Arc<ConnectionConfig>,
    writer: PacketWriter<WriteHalf<ClientTlsStream<TcpStream>>>,
}

impl Connection {
    pub fn new(name: String, config: Arc<ConnectionConfig>) -> Connection {
        let writer = PacketWriter::new(&name);
        Connection {
            name,
            config,
            writer,
        }
    }

    // FIXME(konishchev): Connection TTL?
    pub async fn handle(&self, tun: Arc<Tun>) {
        loop {
            match self.process_connect().await {
                Ok(connection) => {
                    let fd = connection.as_raw_fd();
                    let (reader, writer) = tokio::io::split(connection);
                    self.writer.replace(fd, writer);
                    self.process_connection(reader, tun.clone()).await;
                },
                Err(err) => {
                    warn!("[{}] Failed to establish hiddenlink connection: {err}", self.name);
                },
            };

            self.writer.reset();

            // FIXME(konishchev): Exponential backoff / randomization?
            let timeout = Duration::from_secs(3);
            info!("[{}] Reconnecting in {timeout:?}...", self.name);
            time::sleep(timeout).await;
        }
    }

    async fn process_connect(&self) -> GenericResult<ClientTlsStream<TcpStream>> {
        let endpoint = &self.config.endpoint;
        trace!("[{}] Establishing new connection to {}...", self.name, endpoint);

        let tcp_connection = TcpStream::connect(endpoint).await.map_err(|e| format!(
            "Unable to connect: {e}"))?;
        pre_configure_hiddenlink_socket(&tcp_connection)?;

        let domain = ServerName::DnsName(self.config.domain.clone());
        let tls_connector = TlsConnector::from(self.config.client_config.clone());

        let mut tls_connection = tls_connector.connect(domain, tcp_connection).await.map_err(|e| format!(
            "TLS handshake failed: {e}"))?;

        self.process_handshake(&mut tls_connection).await.map_err(|e| format!(
            "Hiddenlink handshake failed: {e}"))?;

        trace!("[{}] Connected.", self.name);
        Ok(tls_connection)
    }

    async fn process_handshake(&self, connection: &mut ClientTlsStream<TcpStream>) -> EmptyResult {
        // FIXME(konishchev): Send random payload to mimic HTTP client request
        // FIXME(konishchev): Send name?
        connection.write_all(self.config.secret.as_bytes()).await?;
        connection.write_u8(self.config.flags.bits()).await?;
        post_configure_hiddenlink_socket(connection.get_ref().0)?;
        Ok(())
    }

    async fn process_connection(&self, connection: ReadHalf<ClientTlsStream<TcpStream>>, tun: Arc<Tun>) {
        let mut packet_reader = PacketReader::new(Bytes::new(), connection);

        loop {
            let packet = match packet_reader.read().await {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    info!("[{}]: Server has closed the connection.", self.name);
                    break;
                },
                Err(err) => {
                    warn!("[{}]: {err}.", self.name);
                    return;
                }
            };

            if !self.config.flags.contains(ConnectionFlags::INGRESS) {
                error!("[{}] Got a packet from non-ingress connection.", self.name);
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

#[async_trait]
impl Transport for Connection {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        self.config.flags.contains(ConnectionFlags::EGRESS) && self.writer.is_ready()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) {
        self.writer.collect(encoder)
    }

    async fn send(&self, packet: &[u8]) -> EmptyResult {
        self.writer.send(packet).await
    }
}