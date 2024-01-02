use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use bytes::BytesMut;
use log::{trace, info};
use prometheus_client::collector::Collector;
use prometheus_client::encoding::{EncodeMetric, DescriptorEncoder};
use prometheus_client::metrics::counter::ConstCounter;
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

        let mut transports = WeightedTransports::new();

        for config in &config.transports {
            let tun = tun.clone();
            let name = None; // FIXME(konishchev): Support

            let transport = match &config.transport {
                TransportConfig::HttpClient(config) => {
                    let name = name.unwrap_or_else(|| format!("HTTP client to {}", config.endpoint));
                    HttpClientTransport::new(name, config, tun).await.map_err(|e| format!(
                        "Failed to initialize HTTP client transport: {e}"))?
                },

                TransportConfig::HttpServer(config) => HttpServerTransport::new(config, tun).await.map_err(|e| format!(
                    "Failed to initialize HTTP server transport: {e}"))?,

                TransportConfig::Udp(config) => UdpTransport::new(config, tun).await.map_err(|e| format!(
                    "Failed to initialize UDP transport: {e}"))?,
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

// XXX(konishchev): Implement
impl Collector for Tunnel {
    fn encode(&self, mut encoder: DescriptorEncoder) -> std::fmt::Result {
        let counter = ConstCounter::new(42);
        let metric_encoder = encoder.encode_descriptor(
            "my_counter",
            "some help",
            None,
            counter.metric_type(),
        )?;
        counter.encode(metric_encoder)?;
        Ok(())
    }
}

impl Debug for Tunnel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel").finish()
    }
}