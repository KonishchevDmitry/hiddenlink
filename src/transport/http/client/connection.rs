use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use log::{trace, info, warn, error};
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_tun::Tun;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::http::common::{ConnectionFlags, PacketReader};
use crate::util;

pub struct Connection {
    name: String,
    endpoint: String,
    domain: DnsName<'static>,
    config: Arc<ClientConfig>,
    flags: ConnectionFlags,
    secret: Arc<String>,
}

impl Connection {
    pub fn new(
        id: u64, endpoint: &str, domain: &str, config: Arc<ClientConfig>, flags: ConnectionFlags, secret: Arc<String>,
    ) -> GenericResult<Connection> {
        Ok(Connection {
            name: format!("HTTP connection #{id} to {endpoint}"),
            endpoint: endpoint.to_owned(),
            domain: DnsName::try_from(domain.to_owned())?,
            config,
            flags,
            secret,
        })
    }

    pub async fn handle(&self, tun: Arc<Tun>) {
        loop {
            match self.process_connect().await {
                Ok(connection) => {
                    self.process_connection(connection, tun.clone()).await;
                },
                Err(err) => {
                    warn!("[{}] Failed to establish hiddenlink connection: {err}", self.name);
                },
            };

            // FIXME(konishchev): Exponential backoff?
            let timeout = Duration::from_secs(3);
            info!("[{}] Reconnecting in {timeout:?}...", self.name);
            time::sleep(timeout).await;
        }
    }

    async fn process_connect(&self) -> GenericResult<ClientTlsStream<TcpStream>> {
        trace!("[{}] Establishing new connection...", self.name);

        let tcp_connection = TcpStream::connect(&self.endpoint).await.map_err(|e| format!(
            "Unable to connect: {e}"))?;

        let domain = ServerName::DnsName(self.domain.clone());
        let tls_connector = TlsConnector::from(self.config.clone());

        let mut tls_connection = tls_connector.connect(domain, tcp_connection).await.map_err(|e| format!(
            "TLS handshake failed: {e}"))?;

        self.process_handshake(&mut tls_connection).await.map_err(|e| format!(
            "Hiddenlink handshake failed: {e}"))?;

        Ok(tls_connection)
    }

    async fn process_handshake(&self, connection: &mut ClientTlsStream<TcpStream>) -> EmptyResult {
        // FIXME(konishchev): Send random payload to mimic HTTP client request
        connection.write_all(self.secret.as_bytes()).await?;
        connection.write_u8(self.flags.bits()).await?;
        Ok(())
    }

    async fn process_connection(&self, connection: ClientTlsStream<TcpStream>, tun: Arc<Tun>) {
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

            if !self.flags.contains(ConnectionFlags::INGRESS) {
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