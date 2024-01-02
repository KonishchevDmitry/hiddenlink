mod hiddenlink_connection;
mod proxied_connection;
mod server_connection;

use std::io::ErrorKind;
use std::sync::{Arc, Mutex};
use std::net::SocketAddr;

use async_trait::async_trait;
use log::{trace, info, error};
use prometheus_client::encoding::DescriptorEncoder;
use rustls::ClientConfig;
use serde_derive::{Serialize, Deserialize};
use tokio::net::TcpListener;
use tokio_tun::Tun;
use validator::Validate;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::{Transport, WeightedTransports};
use crate::transport::http::common::MIN_SECRET_LEN;
use crate::transport::http::server::hiddenlink_connection::HiddenlinkConnection;
use crate::transport::http::server::server_connection::ServerConnection;
use crate::transport::http::tls::{self, TlsDomains, TlsDomainConfig};

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpServerTransportConfig {
    bind_address: SocketAddr,
    upstream_address: SocketAddr,
    default_domain: TlsDomainConfig,

    #[serde(default)]
    additional_domains: Vec<TlsDomainConfig>,

    #[validate(non_control_character)]
    #[validate(length(min = "MIN_SECRET_LEN"))]
    secret: String,
}

pub struct HttpServerTransport {
    name: String,

    secret: Arc<String>,
    domains: Arc<TlsDomains>,

    upstream_address: SocketAddr,
    upstream_client_config: Arc<ClientConfig>,

    connections: Arc<Mutex<WeightedTransports<HiddenlinkConnection>>>,
}

// FIXME(konishchev): Consider to: SIOCOUTQ, TCP_INFO
impl HttpServerTransport {
    pub async fn new(config: &HttpServerTransportConfig, tun: Arc<Tun>) -> GenericResult<Arc<dyn Transport>> {
        let name = format!("HTTP server on {}", config.bind_address);
        let secret = Arc::new(config.secret.clone());
        let domains = Arc::new(TlsDomains::new(&config.default_domain, &config.additional_domains)?);

        let roots = tls::load_roots().map_err(|e| format!(
            "Failed to load root certificates: {}", e))?;

        let upstream_client_config = Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth());

        let listener = TcpListener::bind(config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let transport = Arc::new(HttpServerTransport{
            name,

            secret,
            domains,

            upstream_address: config.upstream_address,
            upstream_client_config,

            connections: Arc::new(Mutex::new(WeightedTransports::new())),
        });

        info!("[{}] Listening on {}.", transport.name, config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(listener, tun).await;
            });
        }

        Ok(transport)
    }

    async fn handle(&self, listener: TcpListener, tun: Arc<Tun>) {
        loop {
            // FIXME(konishchev): Add semaphore

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

            trace!("[{}] Connection accepted from {}.", self.name, peer_addr);
            let server_connection = ServerConnection::new(
                peer_addr, self.secret.clone(), self.domains.clone(),
                self.upstream_address, self.upstream_client_config.clone());

            {
                let tun = tun.clone();
                let connections = self.connections.clone();

                tokio::spawn(async move {
                    if let Some(connection) = server_connection.handle(connection).await {
                        let connection = Arc::new(connection);

                        // FIXME(konishchev): Get weight from client
                        connections.lock().unwrap().add(connection.clone(), 100, 100);
                        connection.handle(tun).await;
                        connections.lock().unwrap().remove(connection);
                    }
                });
            }
        }
    }
}

#[async_trait]
impl Transport for HttpServerTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        self.connections.lock().unwrap().is_ready()
    }

    // FIXME(konishchev): Implement
    fn collect(&self, _encoder: &mut DescriptorEncoder) {
    }

    async fn send(&self, packet: &[u8]) -> EmptyResult {
        let connection = self.connections.lock().unwrap().select().ok_or(
            "There is no open connections")?;

        trace!("Sending the packet via {}...", connection.name());

        Ok(connection.send(packet).await.map_err(|e| format!(
            "{}: {e}", connection.name()))?)
    }
}