use std::time::Instant;

use prometheus_client::{
    metrics::counter::Counter,
    metrics::histogram::Histogram,
    encoding::DescriptorEncoder,
};
use tokio_tun::Tun;

use crate::core::GenericResult;
use crate::metrics;

pub struct Tunnel {
    tun: Tun,
    recv_time: Counter<f64>,
    send_times: Histogram,
}

impl Tunnel {
    pub fn new(tun: Tun) -> Tunnel {
        Tunnel {
            tun,
            recv_time: Counter::default(),
            send_times: metrics::send_time_histogram(),
        }
    }

    pub fn mtu(&self) -> GenericResult<usize> {
        let mtu = self.tun.mtu().map_err(|e| format!(
            "Failed to obtain tunnel MTU: {}", e))?;

        Ok(mtu.try_into().map_err(|_| format!(
            "Got an invalid MTU value: {}", mtu))?)
    }

    pub async fn recv(&self, buf: &mut [u8]) -> GenericResult<usize> {
        let start_time = Instant::now();
        let result = self.tun.recv(buf).await;
        let recv_time = start_time.elapsed();

        self.recv_time.inc_by(recv_time.as_secs_f64());
        Ok(result.map_err(|e| format!("Failed to read from tun device: {e}"))?)
    }

    pub async fn send(&self, buf: &[u8]) -> GenericResult<usize> {
        let start_time = Instant::now();
        let result = self.tun.send(buf).await;
        let send_time = start_time.elapsed();

        self.send_times.observe(send_time.as_secs_f64());
        Ok(result.map_err(|e| format!("Failed to send packet to tun device: {e}"))?)
    }

    pub fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        metrics::collect_metric(
            encoder, "tunnel_packet_receive_time", "Packet receive time from tunnel (idle time)",
            &self.recv_time)?;

        metrics::collect_metric(
            encoder, "tunnel_packet_send_time", "Packet send time to tunnel",
            &self.send_times)?;

        Ok(())
    }
}