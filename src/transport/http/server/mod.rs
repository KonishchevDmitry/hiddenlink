mod hiddenlink_connection;
mod proxied_connection;
mod server_connection;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex};
use std::net::SocketAddr;

use async_trait::async_trait;
use bytes::BytesMut;
use itertools::Itertools;
use log::{trace, info, error};
use nix::sys::resource::Resource;
use prometheus_client::encoding::DescriptorEncoder;
use prometheus_client::metrics::gauge::Gauge;
use rustls::ClientConfig;
use serde_derive::{Serialize, Deserialize};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use validator::Validate;

use crate::core::{GenericResult, EmptyResult};
use crate::metrics::{self, TransportLabels};
use crate::transport::{Transport, MeteredTransport, WeightedTransports, TransportConnectionStat};
use crate::transport::http::common::MIN_SECRET_LEN;
use crate::transport::http::server::hiddenlink_connection::HiddenlinkConnection;
use crate::transport::http::server::server_connection::ServerConnection;
use crate::transport::http::tls::{self, TlsDomains, TlsDomainConfig};
use crate::tunnel::Tunnel;

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
    labels: TransportLabels,

    secret: Arc<String>,
    domains: Arc<TlsDomains>,

    upstream_address: SocketAddr,
    upstream_client_config: Arc<ClientConfig>,

    proxied_connections_count: Arc<Gauge>,
    proxied_connections_semaphore: Arc<Semaphore>,

    connections: Arc<Mutex<HiddenlinkConnections>>,
}

impl HttpServerTransport {
    pub async fn new(name: &str, config: &HttpServerTransportConfig, tunnel: Arc<Tunnel>) -> GenericResult<Arc<dyn MeteredTransport>> {
        let labels = metrics::transport_labels(name);
        let secret = Arc::new(config.secret.clone());
        let domains = Arc::new(TlsDomains::new(&config.default_domain, &config.additional_domains)?);

        let roots = tls::load_roots().map_err(|e| format!(
            "Failed to load root certificates: {}", e))?;

        let upstream_client_config = Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth());

        let (max_open_files, _hard_limit) = nix::sys::resource::getrlimit(Resource::RLIMIT_NOFILE).map_err(|e| format!(
            "Failed to obtain max open files limit: {e}"))?;

        let max_proxied_connections = std::cmp::min(
            (max_open_files / 2).try_into().unwrap(),
            Semaphore::MAX_PERMITS
        );

        let listener = TcpListener::bind(config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let transport = Arc::new(HttpServerTransport{
            name: name.to_owned(),
            labels,

            secret,
            domains,

            upstream_address: config.upstream_address,
            upstream_client_config,

            proxied_connections_count: Arc::new(Gauge::default()),
            proxied_connections_semaphore: Arc::new(Semaphore::new(max_proxied_connections)),

            connections: Default::default(),
        });

        info!("[{}] Listening on {} with {} proxied connections limit.",
            transport.name, config.bind_address, max_proxied_connections);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(listener, tunnel).await;
            });
        }

        Ok(transport)
    }

    async fn handle(&self, listener: TcpListener, tunnel: Arc<Tunnel>) {
        loop {
            let proxied_connection_permit = self.proxied_connections_semaphore.clone().acquire_owned().await.unwrap();

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

            let tunnel = tunnel.clone();
            let server_name = self.name.clone();
            let connections = self.connections.clone();
            let proxied_connections = self.proxied_connections_count.clone();

            let server_connection = ServerConnection::new(
                peer_addr, self.secret.clone(), self.domains.clone(),
                self.upstream_address, self.upstream_client_config.clone());

            proxied_connections.inc();
            tokio::spawn(async move {
                let result = server_connection.handle(connection).await;

                proxied_connections.dec();
                drop(proxied_connection_permit);

                let Some(request) = result else {
                    return
                };

                let stat = connections.lock().unwrap().stat.entry(request.name.clone()).or_insert_with(|| {
                    Arc::new(TransportConnectionStat::new(&server_name, &request.name))
                }).clone();

                let connection = Arc::new(HiddenlinkConnection::new(
                    request.name, request.flags, request.preread_data, request.connection, stat,
                ));

                // FIXME(konishchev): Get weight from client
                connections.lock().unwrap().active.add(connection.clone(), 100, 100);
                connection.handle(tunnel).await;
                connections.lock().unwrap().active.remove(connection);
            });
        }
    }
}

#[async_trait]
impl Transport for HttpServerTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        self.connections.lock().unwrap().active.is_ready()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        metrics::collect_metric(
            encoder, "open_proxied_connections", "The number of currently open proxied connections",
            self.proxied_connections_count.as_ref())?;

        let (connections, stats) = {
            let connections = self.connections.lock().unwrap();
            (
                connections.active.iter().map(|weighted| weighted.transport.clone()).collect_vec(),
                connections.stat.values().cloned().collect_vec(),
            )
        };

        for stat in stats {
            stat.collect(encoder)?;
        }

        for connection in connections {
            connection.collect(encoder)?;
        }

        Ok(())
    }

    async fn send(&self, packet: &mut BytesMut) -> EmptyResult {
        let connection = self.connections.lock().unwrap().active.select().ok_or(
            "There is no open connections")?;

        trace!("Sending the packet via {}...", connection.name());

        Ok(connection.send(packet).await.map_err(|e| format!(
            "{}: {e}", connection.name()))?)
    }
}

impl MeteredTransport for HttpServerTransport {
    fn labels(&self) -> &TransportLabels {
        &self.labels
    }
}

#[derive(Default)]
struct HiddenlinkConnections {
    active: WeightedTransports<HiddenlinkConnection>,
    stat: HashMap<String, Arc<TransportConnectionStat>>,
}