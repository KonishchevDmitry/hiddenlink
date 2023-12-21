use std::net::SocketAddr;
use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::core::{GenericResult, EmptyResult};

pub struct ProxyConnection {
    addr: SocketAddr,
    tls_connection: TlsStream<TcpStream>,
}

impl ProxyConnection {
    pub async fn new(config: Arc<ClientConfig>, addr: SocketAddr, domain: &str) -> GenericResult<ProxyConnection> {
        let domain = DnsName::try_from(domain).map_err(|_| format!(
            "Invalid DNS name: {:?}", domain))?.to_owned();

        let tcp_connection = TcpStream::connect(addr).await.map_err(|e| format!(
            "Unable to connect to {}: {}", addr, e))?;

        let tls_connector = TlsConnector::from(config);
        let tls_connection = tls_connector.connect(ServerName::DnsName(domain), tcp_connection).await.map_err(|e| format!(
            "TLS handshake failed: {}", e))?;

        Ok(ProxyConnection {addr, tls_connection})
    }

    // XXX(konishchev): HERE
    // pub async fn handle(&self) -> EmptyResult {

    //     stream.write_all(content.as_bytes()).await?;

    //     let (mut reader, mut writer) = split(stream);

    //     tokio::select! {
    //         ret = copy(&mut reader, &mut stdout) => {
    //             ret?;
    //         },
    //         ret = copy(&mut stdin, &mut writer) => {
    //             ret?;
    //             writer.shutdown().await?
    //         }
    //     }

    //     Ok(())
    // }
}