use std::sync::Arc;

use log::{trace, warn};
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::http::common::ConnectionFlags;

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

    pub async fn handle(&self) {
        if let Err(err) = self.process_connection().await {
            warn!("[{}] Failed to establish hiddenlink connection: {err}", self.name);
        }
    }

    async fn process_connection(&self) -> EmptyResult {
        trace!("[{}] Establishing new connection...", self.name);

        let tcp_connection = TcpStream::connect(&self.endpoint).await.map_err(|e| format!(
            "Unable to connect: {e}"))?;

        let domain = ServerName::DnsName(self.domain.clone());
        let tls_connector = TlsConnector::from(self.config.clone());

        let tls_connection = tls_connector.connect(domain, tcp_connection).await.map_err(|e| format!(
            "TLS handshake failed: {e}"))?;

        self.process_handshake(tls_connection).await.map_err(|e| format!(
            "Hiddenlink handshake failed: {e}"))?;

        Ok(())
    }

    async fn process_handshake(&self, mut connection: ClientTlsStream<TcpStream>) -> EmptyResult {
        connection.write_all(self.secret.as_bytes()).await?;
        connection.write_u8(self.flags.bits()).await?;
        // let (reader, mut writer) = tokio::io::split(connection);
        Ok(())
    }
}