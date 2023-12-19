use std::sync::Arc;

use bytes::BytesMut;
use log::{trace, info};
use tokio_tun::Tun;

use crate::config::{Config, TransportConfig};
use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::transport::https::HttpsServerTransport;
use crate::transport::udp::UdpTransport;
use crate::util;

pub struct Tunnel {
    tun: Arc<Tun>,
    transports: Vec<Arc<dyn Transport>>,
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

        let mut transports = Vec::new();

        for transport_config in &config.transports {
            let transport = match transport_config {
                TransportConfig::HttpsServer(config) => HttpsServerTransport::new(config).await.map_err(|e| format!(
                    "Failed to initialize HTTPS server transport: {}", e))?,

                TransportConfig::Udp(config) => UdpTransport::new(config, tun.clone()).await.map_err(|e| format!(
                    "Failed to initialize UDP transport: {}", e))?,
            };
            transports.push(transport);
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

            let transport = self.transports.first().unwrap();
            if !transport.is_ready() {
                trace!("Dropping the packet: there are no active transports.");
                continue;
            }

            trace!("Sending the packet via {}...", transport.name());
            match transport.send(packet).await {
                Ok(_) => {
                    trace!("The packet sent.")
                },
                Err(err) => {
                    trace!("Failed to send the packet via {}: {}.", transport.name(), err);
                }
            }
        }
    }
}