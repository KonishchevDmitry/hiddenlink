use std::sync::Arc;

use bytes::Bytes;
use log::{trace, info, warn};
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::http::common::{ConnectionFlags, PacketReader};
use crate::util;

pub struct ClientConnection {
    name: String,
    endpoint: String,
    domain: DnsName<'static>,
    config: Arc<ClientConfig>,
    flags: ConnectionFlags,
    secret: Arc<String>,
}

impl ClientConnection {
    pub fn new(
        id: u64, endpoint: &str, domain: &str, config: Arc<ClientConfig>, flags: ConnectionFlags, secret: Arc<String>,
    ) -> GenericResult<ClientConnection> {
        Ok(ClientConnection {
            name: format!("HTTP connection #{id} to {endpoint}"),
            endpoint: endpoint.to_owned(),
            domain: DnsName::try_from(domain.to_owned())?,
            config,
            flags,
            secret,
        })
    }

    // XXX(konishchev): Rewrite
    pub async fn handle(&self) {
        // let mut buf = BytesMut::with_capacity(4096); // FIXME(konishchev): Capacity

        let connection = match self.process_connection().await {
            Ok(connection) => connection,
            Err(err) => {
                warn!("[{}] Failed to establish hiddenlink connection: {err}", self.name);
                return;
            },
        };

        let mut packet_reader = PacketReader::new(Bytes::new(), connection);

        loop {
            let packet = match packet_reader.read().await {
                Ok(Some(packet)) => {
                    packet
                },
                Ok(None) => {
                    info!("[{}]: The server has closed the connection.", self.name);
                    break;
                },
                Err(err) => {
                    warn!("[{}]: {err}", self.name);
                    return;
                }
            };

            util::trace_packet(&self.name, packet);

            // let size = connection.read_buf(&mut buf).await.unwrap();
            // trace!("Got {}/{} bytes.", size, buf.len());
            // buf.advance(cnt)
            // buf.reserve(additional)
        }
    }

    async fn process_connection(&self) -> GenericResult<ClientTlsStream<TcpStream>> {
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
        // let (reader, mut writer) = tokio::io::split(connection);
        Ok(())
    }
}