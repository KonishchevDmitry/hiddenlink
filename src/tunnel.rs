use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use bytes::BytesMut;
use log::{trace, info};
use prometheus_client::collector::Collector;
use prometheus_client::encoding::DescriptorEncoder;
use tokio_tun::Tun;

use crate::config::{Config, TransportConfig};
use crate::core::{GenericResult, EmptyResult};
use crate::transport::{Transport, WeightedTransports};
use crate::transport::http::{HttpClientTransport, HttpServerTransport};
use crate::transport::udp::UdpTransport;
use crate::util;

pub struct Tunnel {
    tun: Arc<Tun>,
    transports: WeightedTransports<dyn Transport>,
}

impl Tunnel {
    // FIXME(konishchev): Configure MTU in networkd
    // FIXME(konishchev): Configure MSS in firewall
    pub async fn new(config: &Config) -> GenericResult<Tunnel> {
        info!("Attaching to {} tun device...", config.name);

        let tun = Arc::new(Tun::builder()
            .name(&config.name)
            .packet_info(false)
            .try_build_mq(2) // A workaround for https://github.com/yaa110/tokio-tun/pull/19
            .map_err(|e| format!("Unable to attach to {:?} tun device: {}", config.name, e))?
            .drain(..).next().unwrap());

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
            let tun = tun.clone();

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

                    HttpClientTransport::new(name, config, tun).await.map_err(|e| format!(
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

                    HttpServerTransport::new(name, config, tun).await.map_err(|e| format!(
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

                    UdpTransport::new(name, config, tun).await.map_err(|e| format!(
                        "Failed to initialize UDP transport: {e}"))?
                },
            };

            transports.add(transport, config.weight, config.weight);
        }

        Ok(Tunnel {tun, transports})
    }

    pub async fn handle(&self) -> EmptyResult {
        info!("Ready and listening to packets.");

        let mtu = self.tun.mtu()
            .map_err(|e| format!("Failed to get tunnel MTU: {}", e))?;

        let mtu = mtu.try_into().map_err(|_| format!(
            "Got an invalid MTU value: {}", mtu))?;

        let mut buf = BytesMut::zeroed(mtu);

        loop {
            let size = self.tun.recv(&mut buf).await.map_err(|e| format!(
                "Failed to read from tun device: {}", e))?;

            let packet = &buf[..size];
            util::trace_packet("tun device", packet);

            // FIXME(konishchev): Add metric
            let Some(transport) = self.transports.select() else {
                trace!("Dropping the packet: there are no active transports.");
                continue;
            };

            trace!("Sending the packet via {}...", transport.name());
            match transport.send(packet).await {
                Ok(_) => {
                    trace!("The packet sent.")
                },
                Err(err) => {
                    // FIXME(konishchev): Add metric
                    trace!("Failed to send the packet via {}: {}.", transport.name(), err);
                }
            }
        }
    }
}

impl Collector for Tunnel {
    fn encode(&self, mut encoder: DescriptorEncoder) -> std::fmt::Result {
        for weighted in self.transports.iter() {
            weighted.transport.collect(&mut encoder)?;
        }
        Ok(())
    }
}

impl Debug for Tunnel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel").finish()
    }
}