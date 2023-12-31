mod connection;

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use host_port_pair::HostPortPair;
use rustls::ClientConfig;
use serde_derive::{Serialize, Deserialize};
use tokio_tun::Tun;
use validator::Validate;

use crate::core::{GenericResult, GenericError, EmptyResult};
use crate::transport::Transport;
use crate::transport::http::client::connection::Connection;
use crate::transport::http::common::ConnectionFlags;
use crate::transport::http::tls;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpClientTransportConfig {
    endpoint: String,
    #[validate(non_control_character)]
    #[validate(length(min = 1))]
    secret: String,
}

pub struct HttpClientTransport {
    name: String,
    connections: Vec<Arc<Connection>>,
}

impl HttpClientTransport {
    pub async fn new(config: &HttpClientTransportConfig, tun: Arc<Tun>) -> GenericResult<Arc<dyn Transport>> {
        let name = format!("HTTP client to {}", config.endpoint);
        let secret = Arc::new(config.secret.clone());

        let domain = HostPortPair::from_str(&config.endpoint).map_err(GenericError::from).and_then(|host_port| {
            match host_port {
                HostPortPair::DomainAddress(domain, _port) => Ok(domain),
                HostPortPair::SocketAddress(_) => Err!("domain name must be used"),
            }
        }).map_err(|e| format!("Invalid endpoint {:?}: {e}", config.endpoint))?;

        let roots = tls::load_roots().map_err(|e| format!(
            "Failed to load root certificates: {}", e))?;

        let client_config = Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth());

        // FIXME(konishchev): Timeouts
        // FIXME(konishchev): Rewrite
        // FIXME(konishchev): unwrap
        let connection = Arc::new(Connection::new(
            0, &config.endpoint, &domain, client_config.clone(), ConnectionFlags::all(), secret.clone()).unwrap());

        let transport = Arc::new(HttpClientTransport{
            name,
            connections: vec![connection.clone()],
        });

        for connection in transport.connections.iter().cloned() {
            let tun = tun.clone();
            tokio::spawn(async move {
                connection.handle(tun).await;
            });
        }

        Ok(transport)
    }
}

#[async_trait]
impl Transport for HttpClientTransport {
    fn name(&self) -> &str {
        &self.name
    }

    // FIXME(konishchev): Implement
    fn is_ready(&self) -> bool {
        self.connections.first().unwrap().is_ready()
    }

    // FIXME(konishchev): Implement
    async fn send(&self, packet: &[u8]) -> EmptyResult {
        self.connections.first().unwrap().send(packet).await
    }
}