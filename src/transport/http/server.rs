use std::io::ErrorKind;
use std::sync::Arc;
use std::net::SocketAddr;

use async_trait::async_trait;
use log::{trace, info, error};
use rustls::ClientConfig;
use serde_derive::{Serialize, Deserialize};
use tokio::net::TcpListener;
use validator::Validate;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::transport::http::server_connection::ServerConnection;
use crate::transport::http::tls::{self, TlsDomains, TlsDomainConfig};

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpServerTransportConfig {
    bind_address: SocketAddr,
    default_domain: TlsDomainConfig,
    #[serde(default)]
    additional_domains: Vec<TlsDomainConfig>,
}

pub struct HttpServerTransport {
    name: String,
    domains: Arc<TlsDomains>,
    client_config: Arc<ClientConfig>,
}

impl HttpServerTransport {
    pub async fn new(config: &HttpServerTransportConfig) -> GenericResult<Arc<dyn Transport>> {
        let name = format!("HTTP server on {}", config.bind_address);
        let domains = Arc::new(TlsDomains::new(&config.default_domain, &config.additional_domains)?);

        let roots = tls::load_roots().map_err(|e| format!(
            "Failed to load root certificates: {}", e))?;

        let client_config = Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth());

        let listener = TcpListener::bind(config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let transport = Arc::new(HttpServerTransport{
            name,
            domains,
            client_config,
        });

        info!("[{}] Listening on {}.", transport.name, config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(listener).await;
            });
        }

        Ok(transport)
    }

    async fn handle(&self, listener: TcpListener) {
        loop {
            // FIXME(konishchev): Add semaphore
            // FIXME(konishchev): Timeouts?

            let (connection, peer_addr) = match listener.accept().await {
                Ok(result) => result,
                Err(err) => {
                    if err.kind() == ErrorKind::ConnectionAborted {
                        trace!("[{}] Unable to accept a connection: it has been aborted.", self.name);
                    } else {
                        error!("[{}] Failed to accept a connection: {}.", self.name, err)
                    }
                    continue;
                },
            };

            // FIXME(konishchev): Rewrite all below
            trace!("[{}] Connection accepted from {}.", self.name, peer_addr);
            let server_connection = ServerConnection::new(peer_addr, self.domains.clone());

            tokio::spawn(async move {
                if let Err(err) = server_connection.handle(connection).await {
                    error!("[{}] {}.", peer_addr, err);
                }
            });
        }
    }
}

#[async_trait]
impl Transport for HttpServerTransport {
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