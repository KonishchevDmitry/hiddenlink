use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use log::trace;
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use crate::core::{GenericResult, EmptyResult};
use crate::protocols::proxy_protocol::ProxyProtocolHeader;
use crate::transport::http::common::CONNECTION_TIMEOUT;

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
        name: &'a str, client_connection: C, preread_data: Bytes, use_preread_data_only: bool,
        upstream_client_config: Arc<ClientConfig>, upstream_addr: SocketAddr, upstream_domain: &str,
        proxy_protocol: Option<ProxyProtocolHeader>,
    ) -> GenericResult<ProxiedConnection<'a, C>> {
        let domain = DnsName::try_from(upstream_domain).map_err(|_| format!(
            "Invalid DNS name: {:?}", upstream_domain))?.to_owned();

        let mut upstream_tcp_connection = TcpStream::connect(upstream_addr).await.map_err(|e| format!(
            "Unable to connect: {}", e))?;

        if let Some(proxy_protocol) = proxy_protocol {
            upstream_tcp_connection.write_all_buf(&mut proxy_protocol.encode()).await.map_err(|e| format!(
                "Unable to send proxy protocol header: {}", e))?;
        }

        let domain = ServerName::DnsName(domain);
        let upstream_tls_connection = TlsConnector::from(upstream_client_config)
            .connect(domain, upstream_tcp_connection).await
            .map_err(|e| format!("TLS handshake failed: {}", e))?;

        Ok(ProxiedConnection {
            name,
            preread_data,
            client_connection,
            upstream_connection: upstream_tls_connection,
            use_preread_data_only,
        })
    }

    pub async fn handle(self) {
        // Here we rely on:
        // * TCP_USER_TIMEOUT to be sure that we don't hang on writes to client connection infinitely.
        // * Upstream server timeouts: since we mimic it here and fully trust it, we proxy connections infinitely until
        //   it decide to close the connection.

        let (client_reader, client_writer) = tokio::io::split(self.client_connection);
        let (upstream_reader, upstream_writer) = tokio::io::split(self.upstream_connection);

        let preread_data = self.preread_data;
        let client_reader = (!self.use_preread_data_only).then_some(client_reader);

        let client_shutdown = Notify::new();

        match tokio::try_join!(
            async {
                let result = proxy_connection(Some(preread_data), client_reader, upstream_writer).await.map_err(|e| format!(
                    "client -> upstream proxying has been interrupted: {}", e));

                if result.is_ok() {
                    client_shutdown.notify_one();
                }

                result
            },
            async {
                let mut result = proxy_connection(None, Some(upstream_reader), client_writer).await.map_err(|e| format!(
                    "upstream -> client proxying has been interrupted: {}", e));

                if result.is_ok() && time::timeout(CONNECTION_TIMEOUT, client_shutdown.notified()).await.is_err() {
                    result = Err!(concat!(
                        "The client hasn't shutdown the connection after server connection shutdown. ",
                        "Forcibly close it"));
                }

                result
            },
        ) {
            Ok(_) => {
                trace!("[{}] The connection has been successfully proxied.", self.name);
            },
            Err(err) => {
                trace!("[{}] {}.", self.name, err);
            },
        }
    }
}

async fn proxy_connection<R, W>(preread_data: Option<Bytes>, reader: Option<R>, writer: W) -> EmptyResult
    where R: AsyncRead, W: AsyncWrite
{
    tokio::pin!(writer);

    if let Some(mut preread_data) = preread_data {
        writer.write_all_buf(&mut preread_data).await.map_err(|e| format!(
            "Unable to send preread data: {}", e))?;
    }

    if let Some(reader) = reader {
        tokio::pin!(reader);
        tokio::io::copy(&mut reader, &mut writer).await?;
    }

    writer.shutdown().await.map_err(|e| format!(
        "Unable to shutdown connection: {}", e))?;

    Ok(())
}