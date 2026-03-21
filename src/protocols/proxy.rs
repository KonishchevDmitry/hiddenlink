// Proxies a connection to some upstream TCP server

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
use crate::protocols::http;
use crate::protocols::proxy_protocol::ProxyProtocolHeader;

pub struct ProxySpec {
    pub address: SocketAddr,
    pub proxy_protocol: Option<ProxyProtocolHeader>,
    pub tls: Option<ProxyTlsSpec>,
}

pub struct ProxyTlsSpec {
    pub domain: String,
    pub client_config: Arc<ClientConfig>,
}

pub struct ProxiedConnection<'a, C: AsyncRead + AsyncWrite> {
    name: &'a str,
    preread_data: Bytes,
    client_connection: C,
    upstream_connection: Box<dyn UpstreamConnection>,
}

impl<'a, C: AsyncRead + AsyncWrite> ProxiedConnection<'a, C> {
    pub async fn new(
        name: &'a str, preread_data: Bytes, client_connection: C, spec: ProxySpec,
    ) -> GenericResult<ProxiedConnection<'a, C>> {
        let mut upstream_tcp_connection = TcpStream::connect(spec.address).await.map_err(|e| format!(
            "Unable to connect: {e}"))?;

        if let Some(proxy_protocol) = spec.proxy_protocol {
            upstream_tcp_connection.write_all_buf(&mut proxy_protocol.encode()).await.map_err(|e| format!(
                "Unable to send proxy protocol header: {e}"))?;
        }

        let upstream_connection: Box<dyn UpstreamConnection> = match spec.tls {
            Some(tls) => {
                let domain = DnsName::try_from(tls.domain.as_str()).map_err(|_| format!(
                    "Invalid DNS name: {:?}", tls.domain))?.to_owned();

                let domain = ServerName::DnsName(domain);
                let upstream_tls_connection = TlsConnector::from(tls.client_config)
                    .connect(domain, upstream_tcp_connection).await
                    .map_err(|e| format!("TLS handshake failed: {e}"))?;

                Box::new(upstream_tls_connection)
            },
            None => Box::new(upstream_tcp_connection),
        };

        Ok(ProxiedConnection {
            name,
            preread_data,
            client_connection,
            upstream_connection,
        })
    }

    pub async fn handle(self) {
        let preread_data = self.preread_data;
        let (client_reader, client_writer) = tokio::io::split(self.client_connection);
        let (upstream_reader, upstream_writer) = tokio::io::split(Box::into_pin(self.upstream_connection));

        let client_shutdown = Notify::new();

        match tokio::try_join!(
            async {
                let result = proxy_connection(Some(preread_data), client_reader, upstream_writer).await.map_err(|e| format!(
                    "client -> upstream proxying has been interrupted: {e}"));

                if result.is_ok() {
                    client_shutdown.notify_one();
                }

                result
            },
            async {
                let mut result = proxy_connection(None, upstream_reader, client_writer).await.map_err(|e| format!(
                    "upstream -> client proxying has been interrupted: {e}"));

                if result.is_ok() && time::timeout(http::CONNECTION_TIMEOUT, client_shutdown.notified()).await.is_err() {
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
                trace!("[{}] {err}.", self.name);
            },
        }
    }
}

trait UpstreamConnection: AsyncRead + AsyncWrite + Send + Unpin {
}

impl UpstreamConnection for TcpStream {}
impl UpstreamConnection for ClientTlsStream<TcpStream> {}

async fn proxy_connection<R, W>(preread_data: Option<Bytes>, reader: R, writer: W) -> EmptyResult
    where R: AsyncRead, W: AsyncWrite
{
    tokio::pin!(reader);
    tokio::pin!(writer);

    if let Some(mut preread_data) = preread_data {
        writer.write_all_buf(&mut preread_data).await.map_err(|e| format!(
            "Unable to send preread data: {}", e))?;
    }

    tokio::io::copy(&mut reader, &mut writer).await?;

    writer.shutdown().await.map_err(|e| format!(
        "Unable to shutdown connection: {}", e))?;

    Ok(())
}