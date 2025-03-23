mod connection;

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use host_port_pair::HostPortPair;
use log::trace;
use prometheus_client::encoding::DescriptorEncoder;
use rustls::ClientConfig;
use rustls::pki_types::DnsName;
use serde_derive::{Serialize, Deserialize};
use tokio::time::Duration;
use validator::Validate;

use crate::core::{GenericResult, GenericError, EmptyResult};
use crate::metrics::{TransportLabels, self};
use crate::transport::{Transport, TransportDirection, MeteredTransport};
use crate::transport::connections::TransportConnections;
use crate::transport::http::client::connection::{Connection, ConnectionConfig};
use crate::transport::http::common::{self, ConnectionFlags, MIN_SECRET_LEN};
use crate::transport::http::tls;
use crate::transport::stat::TransportConnectionStat;
use crate::tunnel::Tunnel;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpClientTransportConfig {
    endpoint: String,

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

    #[serde(default)]
    #[serde(with = "humantime_serde")]
    connection_min_ttl: Option<Duration>,

    #[serde(default)]
    #[serde(with = "humantime_serde")]
    connection_max_ttl: Option<Duration>,
}

impl HttpClientTransportConfig {
    fn default_direction() -> bool {
        true
    }

    fn default_connections() -> usize {
        1
    }
}

pub struct HttpClientTransport {
    name: String,
    labels: TransportLabels,
    connections: TransportConnections<Connection>,
}

impl HttpClientTransport {
    pub async fn new(name: &str, config: &HttpClientTransportConfig, tunnel: Arc<Tunnel>) -> GenericResult<Arc<dyn MeteredTransport>> {
        let labels = metrics::transport_labels(name);

        let mut flags = ConnectionFlags::empty();
        flags.set(ConnectionFlags::INGRESS, config.ingress);
        flags.set(ConnectionFlags::EGRESS, config.egress);

        let secret = common::encode_secret_for_http1(&config.secret);

        let min_ttl = config.connection_min_ttl.unwrap_or(Duration::from_secs(1));
        if min_ttl < Duration::from_secs(1) {
            return Err!("Too small connection minimal TTL: {min_ttl:?}");
        }
        if let Some(max_ttl) = config.connection_max_ttl {
            if max_ttl < min_ttl {
                return Err!("Too small connection maximum TTL ({max_ttl:?}) - it's less than minimal TTL ({min_ttl:?})");
            }
        }

        let domain = HostPortPair::from_str(&config.endpoint).map_err(GenericError::from).and_then(|host_port| {
            let domain = match host_port {
                HostPortPair::DomainAddress(domain, _port) => domain,
                HostPortPair::SocketAddress(_) => return Err!("domain name must be used"),
            };
            Ok(DnsName::try_from(domain.to_owned())?)
        }).map_err(|e| format!("Invalid endpoint {:?}: {e}", config.endpoint))?;

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
            secret,
            min_ttl,
            max_ttl: config.connection_max_ttl,
        });

        let mut connections = TransportConnections::new();

        for id in 1..=config.connections {
            let connection_name = if config.connections > 1 {
                format!("{name} #{id}")
            } else {
                name.to_owned()
            };

            let stat = Arc::new(TransportConnectionStat::new(name, &connection_name));
            let connection = Arc::new(Connection::new(connection_name, connection_config.clone(), stat));
            assert!(connections.add(connection.clone()));

            let tunnel = tunnel.clone();
            tokio::spawn(async move {
                connection.handle(tunnel).await;
            });
        }

        Ok(Arc::new(HttpClientTransport{
            name: name.to_owned(),
            labels,
            connections,
        }))
    }
}

#[async_trait]
impl Transport for HttpClientTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn direction(&self) -> TransportDirection {
        self.connections.iter().next().unwrap().direction()
    }

    fn connected(&self) -> bool {
        self.connections.iter().all(|connection| connection.connected())
    }

    fn ready_for_sending(&self) -> bool {
        self.connections.ready_for_sending()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        for connection in self.connections.iter() {
            connection.collect(encoder)?;
        }
        Ok(())
    }

    async fn send(&self, packet: &mut [u8]) -> EmptyResult {
        let connection = self.connections.select(packet).ok_or(
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

impl MeteredTransport for HttpClientTransport {
    fn labels(&self) -> &TransportLabels {
        &self.labels
    }
}