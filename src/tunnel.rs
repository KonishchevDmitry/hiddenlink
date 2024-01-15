use std::time::Instant;

use prometheus_client::{
    metrics::histogram::{Histogram, exponential_buckets},
    encoding::DescriptorEncoder,
};
use tokio_tun::Tun;

use crate::core::GenericResult;
use crate::metrics;

pub struct Tunnel {
    tun: Tun,
    recv_times: Histogram,
    send_times: Histogram,
}

impl Tunnel {
    pub fn new(tun: Tun) -> Tunnel {
        Tunnel {
            tun,
            // FIXME(konishchev): Alter buckets
            recv_times: Histogram::new(exponential_buckets(0.00001, 5.0, 10)),
            send_times: Histogram::new(exponential_buckets(0.00001, 5.0, 10)),
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
        self.recv_times.observe(start_time.elapsed().as_secs_f64());
        Ok(result.map_err(|e| format!("Failed to read from tun device: {e}"))?)
    }

    pub async fn send(&self, buf: &[u8]) -> GenericResult<usize> {
        let start_time = Instant::now();
        let result = self.tun.send(buf).await;
        self.send_times.observe(start_time.elapsed().as_secs_f64());
        Ok(result.map_err(|e| format!("Failed to send packet to tun device: {e}"))?)
    }

    pub fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        metrics::collect_metric(
            encoder, "tunnel_packet_receive_time", "Packet receive time from tunnel",
            &self.recv_times)?;

        metrics::collect_metric(
            encoder, "tunnel_packet_send_time", "Packet send time to tunnel",
            &self.send_times)?;

        Ok(())
    }
}