use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use log::trace;
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use crate::core::GenericResult;

// Represents a proxied connection to real web server
pub struct ProxiedConnection<'a, C: AsyncRead + AsyncWrite> {
    name: &'a str,
    preread_data: Bytes,
    client_connection: C,
    upstream_connection: ClientTlsStream<TcpStream>,
    use_preread_data_only: bool,
}

impl<'a, C: AsyncRead + AsyncWrite> ProxiedConnection<'a, C> {
    pub async fn new(
        name: &'a str, preread_data: Bytes, client_connection: C, use_preread_data_only: bool,
        upstream_client_config: Arc<ClientConfig>, upstream_addr: SocketAddr, upstream_domain: &str,
    ) -> GenericResult<ProxiedConnection<'a, C>> {
        let domain = DnsName::try_from(upstream_domain).map_err(|_| format!(
            "Invalid DNS name: {:?}", upstream_domain))?.to_owned();

        let upstream_tcp_connection = TcpStream::connect(upstream_addr).await.map_err(|e| format!(
            "Unable to connect: {}", e))?;

        let domain = ServerName::DnsName(domain);
        let upstream_tls_connector = TlsConnector::from(upstream_client_config);

        let upstream_tls_connection = upstream_tls_connector.connect(domain, upstream_tcp_connection).await.map_err(|e| format!(
            "TLS handshake failed: {}", e))?;

        Ok(ProxiedConnection {
            name,
            preread_data,
            client_connection,
            upstream_connection: upstream_tls_connection,
            use_preread_data_only,
        })
    }

    pub async fn handle(self) {
        let (client_reader, client_writer) = tokio::io::split(self.client_connection);
        let (upstream_reader, upstream_writer) = tokio::io::split(self.upstream_connection);

        let preread_data = self.preread_data;
        let client_reader = (!self.use_preread_data_only).then_some(client_reader);

        match tokio::try_join!(
            proxy_connection(Some(preread_data), client_reader, upstream_writer),
            proxy_connection(None, Some(upstream_reader), client_writer),
        ) {
            Ok(_) => {
                trace!("[{}] The connection has been successfully proxied.", self.name);
            },
            Err(err) => {
                trace!("[{}] Connection proxying has been interrupted: {}.", self.name, err);
            },
        }
    }
}

async fn proxy_connection<R, W>(preread_data: Option<Bytes>, reader: Option<R>, writer: W) -> std::io::Result<()>
    where R: AsyncRead, W: AsyncWrite
{
    tokio::pin!(writer);

    if let Some(mut preread_data) = preread_data {
        writer.write_all_buf(&mut preread_data).await?;
    }

    if let Some(reader) = reader {
        tokio::pin!(reader);
        tokio::io::copy(&mut reader, &mut writer).await?;
    }

    writer.shutdown().await
}