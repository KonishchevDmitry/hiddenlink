use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Instant;

use bytes::BytesMut;
use log::{trace, info};
use prometheus_client::{
    collector::Collector,
    metrics::family::Family,
    metrics::histogram::{Histogram, exponential_buckets},
    encoding::DescriptorEncoder,
};
use tokio_tun::Tun;

use crate::config::{Config, TransportConfig};
use crate::core::{GenericResult, EmptyResult};
use crate::metrics;
use crate::transport::{Transport, WeightedTransports};
use crate::transport::http::{HttpClientTransport, HttpServerTransport};
use crate::transport::udp::UdpTransport;
use crate::tunnel::Tunnel;
use crate::util;

pub struct Controller {
    tunnel: Arc<Tunnel>,
    transports: WeightedTransports<dyn Transport>,
    transport_send_times: Family<[(&'static str, String); 1], Histogram>,
}

impl Controller {
    // FIXME(konishchev): Configure MTU in networkd
    // FIXME(konishchev): Configure MSS in firewall
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

        let mut udp_transports = 0;
        let mut udp_transport_index = 0;

        for config in &config.transports {
            match &config.transport {
                TransportConfig::HttpClient(_) => http_clients += 1,
                TransportConfig::HttpServer(_) => http_servers += 1,
                TransportConfig::Udp(_) => udp_transports += 1,
            }
        }

        let mut names = HashSet::new();
        let mut transports = WeightedTransports::new();

        for config in &config.transports {
            let tunnel = tunnel.clone();

            let name = config.name.clone();
            if let Some(ref name) = name {
                if name.is_empty() || name.trim() != name || !name.chars().all(|c| {
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

                    HttpClientTransport::new(name, config, tunnel).await.map_err(|e| format!(
                        "Failed to initialize HTTP client transport: {e}"))?
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

                    HttpServerTransport::new(name, config, tunnel).await.map_err(|e| format!(
                      "Failed to initialize HTTP server transport: {e}"))?
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

                    UdpTransport::new(name, config, tunnel).await.map_err(|e| format!(
                        "Failed to initialize UDP transport: {e}"))?
                },
            };

            transports.add(transport, config.weight, config.weight);
        }

        Ok(Controller {
            tunnel,
            transports,
            // FIXME(konishchev): Alter buckets
            transport_send_times: Family::new_with_constructor(|| Histogram::new(exponential_buckets(0.00001, 5.0, 10))),
        })
    }

    pub async fn handle(&self) -> EmptyResult {
        info!("Ready and listening to packets.");

        let mut buf = BytesMut::zeroed(self.tunnel.mtu()?);

        loop {
            let size = self.tunnel.recv(&mut buf).await.map_err(|e| format!(
                "Failed to read from tun device: {}", e))?;

            let packet = &buf[..size];
            util::trace_packet("tun device", packet);

            // FIXME(konishchev): Add metric
            let Some(transport) = self.transports.select() else {
                trace!("Dropping the packet: there are no active transports.");
                continue;
            };

            trace!("Sending the packet via {}...", transport.name());

            let send_start_time = Instant::now();
            let result = transport.send(packet).await;
            let send_time = send_start_time.elapsed();

            match result {
                Ok(_) => {
                    trace!("The packet sent.")
                },
                Err(err) => {
                    // FIXME(konishchev): Add metric
                    trace!("Failed to send the packet via {}: {}.", transport.name(), err);
                }
            }

            // FIXME(konishchev): Eliminate to_owned()?
            let labels = [(metrics::TRANSPORT_LABEL, transport.name().to_owned())];
            self.transport_send_times.get_or_create(&labels).observe(send_time.as_secs_f64());
        }
    }
}

impl Collector for Controller {
    fn encode(&self, mut encoder: DescriptorEncoder) -> std::fmt::Result {
        self.tunnel.collect(&mut encoder)?;

        for weighted in self.transports.iter() {
            weighted.transport.collect(&mut encoder)?;
        }

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