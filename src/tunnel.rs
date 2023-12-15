use bytes::BytesMut;
use tokio_tun::Tun;

use crate::{Config, transport};
use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::transport::udp::UdpTransport;
use crate::util;

pub struct Tunnel {
    tun: Tun,
    transports: Vec<Box<dyn Transport>>,
}

impl Tunnel {
    // FIXME(konishchev): Create by networkd with proper owner
    pub async fn new(config: &Config) -> GenericResult<Tunnel> {
        let tun = Tun::builder()
            .name(&config.name)
            .packet_info(false)
            .address("172.31.0.1".parse()?) // FIXME(konishchev): Do we need to manage it?
            .netmask("255.255.255.0".parse()?)
            .up()
            .try_build()?;

        // dbg!(&config.transports);

        let transports = vec![
            UdpTransport::new("127.0.0.1:1234").await?,
        ];

        Ok(Tunnel {tun, transports})
    }

    // FIXME(konishchev): Look at https://github.com/torvalds/linux/blob/master/drivers/net/tun.c
    pub async fn handle(&self) -> EmptyResult {
        let mtu = self.tun.mtu().map_err(|e| format!(
            "Failed to get tunnel MTU: {}", e))?;

        let mtu = mtu.try_into().map_err(|_| format!(
            "Got an invalid MTU value: {}", mtu))?;

        let mut buf = BytesMut::zeroed(mtu);

        loop {
            let size = self.tun.recv(&mut buf).await.unwrap();
            util::trace_packet("tun device", &buf[..size]);
        }

        // let (mut reader, mut _writer) = tokio::io::split(tun);

        // // Writer: simply clone Arced Tun.
        // let tun_c = tun.clone();
        // tokio::spawn(async move{
        //     let buf = b"data to be written";
        //     tun_c.send_all(buf).await.unwrap();
        // });
    }
}