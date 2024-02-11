use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Instant;

use bytes::BytesMut;
use log::{trace, info};
use prometheus_client::{
    collector::Collector,
    metrics::counter::Counter,
    metrics::family::Family,
    metrics::gauge::ConstGauge,
    metrics::histogram::Histogram,
    encoding::DescriptorEncoder,
};
use tokio_tun::Tun;

use crate::config::{Config, TransportConfig};
use crate::core::{GenericResult, EmptyResult};
use crate::metrics::{self, TransportLabels};
use crate::transport::MeteredTransport;
use crate::transport::http::{HttpClientTransport, HttpServerTransport};
use crate::transport::udp::UdpTransport;
use crate::tunnel::Tunnel;
use crate::util;

const BLACKHOLE_TRANSPORT: &str = "Blackhole";

pub struct Controller {
    tunnel: Arc<Tunnel>,
    transports: Vec<Arc<dyn MeteredTransport>>,
    transport_drops: Family<TransportLabels, Counter>,
    transport_send_times: Family<TransportLabels, Histogram>,
}

impl Controller {
    pub async fn new(config: &Config) -> GenericResult<Controller> {
        info!("Attaching to {} tun device...", config.name);

        let tunnel = Arc::new(Tunnel::new(Tun::builder()
            .name(&config.name)
            .packet_info(false)
            .try_build_mq(2) // A workaround for https://github.com/yaa110/tokio-tun/pull/19
            .map_err(|e| format!("Unable to attach to {:?} tun device: {}", config.name, e))?
            .drain(..).next().unwrap()));

        let mut http_clients = 0;
        let mut http_client_index = 0;

        let mut http_servers = 0;
        let mut http_server_index = 0;

        let mut insecure_udp_transports = 0;
        let mut insecure_udp_transport_index = 0;

        let mut udp_transports = 0;
        let mut udp_transport_index = 0;

        for config in &config.transports {
            match &config.transport {
                TransportConfig::HttpClient(_) => http_clients += 1,
                TransportConfig::HttpServer(_) => http_servers += 1,
                TransportConfig::InsecureUdp(_) => insecure_udp_transports += 1,
                TransportConfig::Udp(_) => udp_transports += 1,
            }
        }

        let mut names = HashSet::new();
        let mut transports = Vec::new();

        for config in &config.transports {
            let tunnel = tunnel.clone();

            let name = config.name.clone();
            if let Some(ref name) = name {
                if name.is_empty() || name.trim() != name || name == BLACKHOLE_TRANSPORT || !name.chars().all(|c| {
                    // Note: '#' is reserved for auto-generated names
                    c.is_ascii_alphanumeric() || c == ' '
                }) {
                    return Err!("Invalid transport name: {name:?}");
                }
            }

            let transport = match &config.transport {
                TransportConfig::HttpClient(config) => {
                    http_client_index += 1;

                    let name = name.unwrap_or_else(|| if http_clients > 1 {
                        format!("HTTP client #{http_client_index}")
                    } else {
                        "HTTP client".to_owned()
                    });

                    if !names.insert(name.clone()) {
                        return Err!("Duplicated transport name: {name:?}");
                    }

                    HttpClientTransport::new(&name, config, tunnel).await.map_err(|e| format!(
                        "Failed to initialize {name:?} transport: {e}"))?
                },

                TransportConfig::HttpServer(config) => {
                    http_server_index += 1;

                    let name = name.unwrap_or_else(|| if http_servers > 1 {
                        format!("HTTP server #{http_server_index}")
                    } else {
                        "HTTP server".to_owned()
                    });

                    if !names.insert(name.clone()) {
                        return Err!("Duplicated transport name: {name:?}");
                    }

                    HttpServerTransport::new(&name, config, tunnel).await.map_err(|e| format!(
                      "Failed to initialize {name:?} transport: {e}"))?
                },

                TransportConfig::InsecureUdp(config) => {
                    insecure_udp_transport_index += 1;

                    let name = name.unwrap_or_else(|| if insecure_udp_transports > 1 {
                        format!("Insecure UDP transport #{insecure_udp_transport_index}")
                    } else {
                        "Insecure UDP transport".to_owned()
                    });

                    if !names.insert(name.clone()) {
                        return Err!("Duplicated transport name: {name:?}");
                    }

                    UdpTransport::new(&name, config, None, tunnel).await.map_err(|e| format!(
                        "Failed to initialize {name:?} transport: {e}"))?
                },

                TransportConfig::Udp(config) => {
                    udp_transport_index += 1;

                    let name = name.unwrap_or_else(|| if udp_transports > 1 {
                        format!("UDP transport #{udp_transport_index}")
                    } else {
                        "UDP transport".to_owned()
                    });

                    if !names.insert(name.clone()) {
                        return Err!("Duplicated transport name: {name:?}");
                    }

                    UdpTransport::new(&name, &config.udp, Some(&config.securer), tunnel).await.map_err(|e| format!(
                        "Failed to initialize {name:?} transport: {e}"))?
                },
            };

            transports.push(transport);
        }

        Ok(Controller {
            tunnel,
            transports,
            transport_drops: Family::default(),
            transport_send_times: Family::new_with_constructor(metrics::send_time_histogram),
        })
    }

    pub async fn handle(&self) -> EmptyResult {
        info!("Ready and listening to packets.");

        let blackhole_labels = metrics::transport_labels(BLACKHOLE_TRANSPORT);
        let _ = self.transport_drops.get_or_create(&blackhole_labels);

        let mut buf = BytesMut::zeroed(self.tunnel.mtu()?);

        loop {
            let size = self.tunnel.recv(&mut buf).await.map_err(|e| format!(
                "Failed to read from tun device: {}", e))?;

            let packet = &mut buf[..size];
            util::trace_packet("tun device", packet);

            let Some(transport) = self.select_transport() else {
                trace!("Dropping the packet: there are no active transports.");
                self.transport_drops.get_or_create(&blackhole_labels).inc();
                continue;
            };

            trace!("Sending the packet via {}...", transport.name());

            let send_start_time = Instant::now();
            let result = transport.send(packet).await;
            let send_time = send_start_time.elapsed();

            match result {
                Ok(_) => trace!("The packet sent."),
                Err(ref err) => trace!("Failed to send the packet via {}: {err}.", transport.name()),
            };

            {
                let drops = self.transport_drops.get_or_create(transport.labels());
                if result.is_err() {
                    drops.inc();
                }
            }

            self.transport_send_times.get_or_create(transport.labels()).observe(send_time.as_secs_f64());
        }
    }

    fn select_transport(&self) -> Option<&dyn MeteredTransport> {
        for transport in &self.transports {
            if transport.is_ready() {
                return Some(transport.as_ref());
            }
        }
        None
    }
}

impl Collector for Controller {
    fn encode(&self, mut encoder: DescriptorEncoder) -> std::fmt::Result {
        self.tunnel.collect(&mut encoder)?;

        for transport in &self.transports {
            let state = if transport.is_ready() {
                "connected"
            } else {
                "unavailable"
            };

            metrics::collect_family(&mut encoder, "transport_state", "Current transport state", &[
                (metrics::TRANSPORT_LABEL, transport.name()),
                ("state", state),
            ], &ConstGauge::new(1))?;

            transport.collect(&mut encoder)?;
        }

        metrics::collect_metric(
            &mut encoder, "transport_packet_drops", "Packet drops by transports",
            &self.transport_drops)?;

        metrics::collect_metric(
            &mut encoder, "transport_packet_send_time", "Packet send time via transports",
            &self.transport_send_times)?;

        Ok(())
    }
}

impl Debug for Controller {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel").finish()
    }
}