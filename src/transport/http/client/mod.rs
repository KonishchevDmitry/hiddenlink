mod connection;

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use host_port_pair::HostPortPair;
use rustls::ClientConfig;
use serde_derive::{Serialize, Deserialize};
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
    endpoint: String,
    domain: String,
    config: Arc<ClientConfig>,
    secret: Arc<String>,
}

impl HttpClientTransport {
    pub async fn new(config: &HttpClientTransportConfig) -> GenericResult<Arc<dyn Transport>> {
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

        let transport = Arc::new(HttpClientTransport{
            name,
            endpoint: config.endpoint.clone(),
            domain,
            config: client_config,
            secret,
        });

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle().await;
            });
        }

        Ok(transport)
    }

    async fn handle(&self) {
        // FIXME(konishchev): Timeouts
        // FIXME(konishchev): Rewrite
        let connection = Connection::new(
            0, &self.endpoint, &self.domain, self.config.clone(), ConnectionFlags::all(), self.secret.clone()).unwrap();
        connection.handle().await;
    }
}

#[async_trait]
impl Transport for HttpClientTransport {
    fn name(&self) -> &str {
        &self.name
    }

    // FIXME(konishchev): Implement
    fn is_ready(&self) -> bool {
        false
    }

    // FIXME(konishchev): Implement
    async fn send(&self, _: &[u8]) -> EmptyResult {
        todo!()
    }
}