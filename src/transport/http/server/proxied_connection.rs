use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use log::trace;
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use crate::core::{GenericResult, EmptyResult};

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
            async {
                proxy_connection(Some(preread_data), client_reader, upstream_writer).await.map_err(|e| format!(
                    "client -> upstream proxying has been interrupted: {}", e))
            },
            async {
                proxy_connection(None, Some(upstream_reader), client_writer).await.map_err(|e| format!(
                    "upstream -> client proxying has been interrupted: {}", e))
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

#[derive(Clone, Copy)]
pub struct ProxyProtocolHeader {
    pub peer_addr: SocketAddr,
    pub local_addr: SocketAddr,
}

impl ProxyProtocolHeader {
    // See http://www.haproxy.org/download/3.0/doc/proxy-protocol.txt for details
    fn encode(&self) -> BytesMut {
        let (mut peer_addr, mut local_addr) = (self.peer_addr, self.local_addr);

        if let (SocketAddr::V6(peer), SocketAddr::V6(local)) = (peer_addr, local_addr) {
            if let (Some(peer), Some(local)) = (peer.ip().to_ipv4_mapped(), local.ip().to_ipv4_mapped()) {
                peer_addr = SocketAddr::V4(SocketAddrV4::new(peer, peer_addr.port()));
                local_addr = SocketAddr::V4(SocketAddrV4::new(local, local_addr.port()));
            }
        }

        const SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];
        const VERSION: u8 = 2;
        const CONNECTION_PROXY: u8 = 1;
        const FAMILY_AF_INET: u8 = 1;
        const FAMILY_AF_INET6: u8 = 2;
        const PROTOCOL_STREAM: u8 = 1;

        let mut header = BytesMut::with_capacity(13 + 3 + 36);
        header.put_slice(&SIGNATURE);
        header.put_u8(VERSION << 4 | CONNECTION_PROXY);

        match (peer_addr, local_addr) {
            (SocketAddr::V4(peer), SocketAddr::V4(addr)) => {
                header.put_u8(FAMILY_AF_INET << 4 | PROTOCOL_STREAM);
                header.put_u16(12);
                header.put_slice(&peer.ip().octets());
                header.put_slice(&addr.ip().octets());
                header.put_u16(peer.port());
                header.put_u16(addr.port());
            },
            (SocketAddr::V6(peer), SocketAddr::V6(addr)) => {
                header.put_u8(FAMILY_AF_INET6 << 4 | PROTOCOL_STREAM);
                header.put_u16(36);
                header.put_slice(&peer.ip().octets());
                header.put_slice(&addr.ip().octets());
                header.put_u16(peer.port());
                header.put_u16(addr.port());
            },
            (SocketAddr::V4(_), SocketAddr::V6(_)) | (SocketAddr::V6(_), SocketAddr::V4(_)) => unreachable!(),
        }

        header
    }
}