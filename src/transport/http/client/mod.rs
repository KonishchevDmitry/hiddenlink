mod connection;

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use host_port_pair::HostPortPair;
use log::{trace, debug};
use rustls::ClientConfig;
use rustls::pki_types::DnsName;
use serde_derive::{Serialize, Deserialize};
use tokio_tun::Tun;
use validator::Validate;

use crate::core::{GenericResult, GenericError, EmptyResult};
use crate::transport::{Transport, WeightedTransports, default_transport_weight};
use crate::transport::http::client::connection::{Connection, ConnectionConfig};
use crate::transport::http::common::{ConnectionFlags, MIN_SECRET_LEN};
use crate::transport::http::tls;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpClientTransportConfig {
    pub endpoint: String,

    #[validate(non_control_character)]
    #[validate(length(min = "MIN_SECRET_LEN"))]
    secret: String,

    #[serde(default="HttpClientTransportConfig::default_direction")]
    ingress: bool,

    #[serde(default="HttpClientTransportConfig::default_direction")]
    egress: bool,

    #[validate(range(min = 1, max = 10))]
    #[serde(default = "HttpClientTransportConfig::default_connections")]
    connections: usize,

    #[validate(range(min = 1))]
    #[serde(default="default_transport_weight")]
    connection_min_weight: u16,

    #[validate(range(min = 1))]
    #[serde(default="default_transport_weight")]
    connection_max_weight: u16,
}

impl HttpClientTransportConfig {
    fn default_connections() -> usize {
        1
    }

    fn default_direction() -> bool {
        true
    }
}

pub struct HttpClientTransport {
    name: String,
    connections: WeightedTransports<Connection>,
}

impl HttpClientTransport {
    pub async fn new(name: String, config: &HttpClientTransportConfig, tun: Arc<Tun>) -> GenericResult<Arc<dyn Transport>> {
        let mut flags = ConnectionFlags::empty();
        flags.set(ConnectionFlags::INGRESS, config.ingress);
        flags.set(ConnectionFlags::EGRESS, config.egress);

        let domain = HostPortPair::from_str(&config.endpoint).map_err(GenericError::from).and_then(|host_port| {
            let domain = match host_port {
                HostPortPair::DomainAddress(domain, _port) => domain,
                HostPortPair::SocketAddress(_) => return Err!("domain name must be used"),
            };
            Ok(DnsName::try_from(domain.to_owned())?)
        }).map_err(|e| format!("Invalid endpoint {:?}: {e}", config.endpoint))?;

        if config.connection_min_weight > config.connection_max_weight {
            return Err!("Invalid connection weight configuration");
        }

        let roots = tls::load_roots().map_err(|e| format!(
            "Failed to load root certificates: {}", e))?;

        let client_config = Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth());

        let connection_config = Arc::new(ConnectionConfig {
            endpoint: config.endpoint.clone(),
            domain,
            client_config,
            flags,
            secret: config.secret.clone(),
        });

        let mut connections = WeightedTransports::new();

        for id in 1..=config.connections {
            let connection_name = if config.connections > 1 {
                format!("{name} #{id}")
            } else {
                name.clone()
            };

            let connection = Arc::new(Connection::new(connection_name, connection_config.clone()));

            let weight = connections.add(connection.clone(), config.connection_min_weight, config.connection_max_weight).weight;
            if config.connections > 1 {
                // FIXME(konishchev): Rebalance periodically
                debug!("[{}] #{id} connection is created with weight {weight}.", name);
            }

            let tun = tun.clone();
            tokio::spawn(async move {
                connection.handle(tun).await;
            });
        }

        Ok(Arc::new(HttpClientTransport{
            name,
            connections,
        }))
    }
}

#[async_trait]
impl Transport for HttpClientTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        self.connections.is_ready()
    }

    async fn send(&self, packet: &[u8]) -> EmptyResult {
        let connection = self.connections.select().ok_or(
            "There is no open connections")?;

        if self.connections.len() > 1 {
            trace!("Sending the packet via {}...", connection.name());
        }

        Ok(connection.send(packet).await.map_err(|e| {
            if self.connections.len() > 1 {
                format!("{}: {e}", connection.name())
            } else {
                e.to_string()
            }
        })?)
    }
}