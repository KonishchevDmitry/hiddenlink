use std::sync::Arc;

use bytes::BytesMut;
use log::{trace, info};
use tokio_tun::Tun;

use crate::config::{Config, TransportConfig};
use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::transport::udp::UdpTransport;
use crate::util;

pub struct Tunnel {
    tun: Arc<Tun>,
    transports: Vec<Arc<dyn Transport>>,
}

impl Tunnel {
    // FIXME(konishchev): Create by networkd with proper owner
    pub async fn new(config: &Config) -> GenericResult<Tunnel> {
        info!("Attaching to {} tun device...", config.name);

        let mut tuns = Box::new(Tun::builder()
            .name(&config.name)
            .packet_info(false)
            // FIXME(konishchev): Configure MTU in networkd
            // .address("172.31.0.1".parse()?) // FIXME(konishchev): Do we need to manage it?
            // .netmask("255.255.255.0".parse()?)
            // .up()
            // .try_build()
            // XXX(konishchev): HERE
            .try_build_mq(2) // FIXME(konishchev): https://github.com/yaa110/tokio-tun/pull/19
            .map_err(|e| format!("Unable to create {:?} tun device: {}", config.name, e))?);
            // .drain(..).next().unwrap();

        let tun = Arc::new(tuns.pop().unwrap());

        let mut transports = Vec::new();

        for transport_config in &config.transports {
            let transport = match transport_config {
                TransportConfig::Udp(config) => UdpTransport::new(config, tun.clone()).await.map_err(|e| format!(
                    "Failed to initialize UDP transport: {}", e))?,
            };
            transports.push(transport);
        }

        Ok(Tunnel {tun, transports})
    }

    // FIXME(konishchev): Look at https://github.com/torvalds/linux/blob/master/drivers/net/tun.c
    pub async fn handle(&self) -> EmptyResult {
        // FIXME(konishchev): HERE
        info!("Started.");

        let mtu = self.tun.mtu().map_err(|e| format!(
            "Failed to get tunnel MTU: {}", e))?;

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