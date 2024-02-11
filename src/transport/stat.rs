use std::os::fd::AsFd;

use itertools::Itertools;
use log::error;
use prometheus_client::{
	metrics::counter::{Counter, ConstCounter},
	metrics::gauge::ConstGauge,
	encoding::DescriptorEncoder,
};

use crate::bindings;
use crate::metrics;
use crate::util;

pub struct TransportConnectionStat {
    name: String,
    labels: [(&'static str, String); 2],

    received_packets: Counter,
    received_data: Counter,

    sent_packets: Counter,
    sent_data: Counter,

    dropped_packets: Counter,
}

impl TransportConnectionStat {
    pub fn new(transport_name: &str, connection_name: &str) -> TransportConnectionStat {
        TransportConnectionStat {
            name: connection_name.to_owned(),
            labels: [
                (metrics::TRANSPORT_LABEL, transport_name.to_owned()),
                (metrics::CONNECTION_LABEL, connection_name.to_owned()),
            ],

            received_packets: Counter::default(),
            received_data: Counter::default(),

            sent_packets: Counter::default(),
            sent_data: Counter::default(),

            dropped_packets: Counter::default(),
        }
    }

    pub fn on_packet_received(&self, packet: &[u8]) {
        self.received_packets.inc();
        self.received_data.inc_by(packet.len().try_into().unwrap());
    }

    pub fn on_packet_sent(&self, packet: &[u8]) {
        self.sent_packets.inc();
        self.sent_data.inc_by(packet.len().try_into().unwrap());
    }

    pub fn on_packet_dropped(&self) {
        self.dropped_packets.inc();
    }

    pub fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        metrics::collect_family(
            encoder, "connection_received_packets", "Total received packets",
            &self.labels, &self.received_packets)?;

        metrics::collect_family(
            encoder, "connection_received_bytes", "Total received data",
            &self.labels, &self.received_data)?;

        metrics::collect_family(
            encoder, "connection_sent_packets", "Total sent packets",
            &self.labels, &self.sent_packets)?;

        metrics::collect_family(
            encoder, "connection_sent_bytes", "Total sent data",
            &self.labels, &self.sent_data)?;

        metrics::collect_family(
            encoder, "connection_dropped_packets", "Total dropped packets",
            &self.labels, &self.dropped_packets)?;

        Ok(())
    }

    pub fn collect_tcp_socket<F: AsFd>(&self, encoder: &mut DescriptorEncoder, fd: &F) -> std::fmt::Result {
        match util::get_tcp_info(fd) {
            Ok(info) => {
                let state = match info.tcpi_ca_state.into() {
                    bindings::tcp_ca_state_TCP_CA_Open => "ok",
                    bindings::tcp_ca_state_TCP_CA_Disorder => "disorder",
                    bindings::tcp_ca_state_TCP_CA_CWR => "congestion-window-reduction",
                    bindings::tcp_ca_state_TCP_CA_Recovery => "fast-recovery",
                    bindings::tcp_ca_state_TCP_CA_Loss => "loss-recovery",
                    _ => "unknown",
                };

                let state_labels = self.labels.iter().cloned().chain([("state", state.to_owned())]).collect_vec();

                metrics::collect_family(
                    encoder, "socket_state", "Current socket state",
                    &state_labels, &ConstGauge::new(1))?;

                // FIXME(konishchev): Not available yet
                // metrics::collect_family(
                //     encoder, "socket_receive_window_bytes", "Local advertised receive window",
                //     &self.labels, &ConstGauge::<i64>::new(info.tcpi_rcv_wnd.into()))?;

                metrics::collect_family(
                    encoder, "socket_send_window_bytes", "Peer's advertised receive window",
                    &self.labels, &ConstGauge::<i64>::new(info.tcpi_snd_wnd.into()))?;

                metrics::collect_family(
                    encoder, "socket_reordered_packets", "Count of received reordered packets",
                    &self.labels, &ConstCounter::<u64>::new(info.tcpi_reord_seen.into()))?;

                metrics::collect_family(
                    encoder, "socket_retransmits", "The number of segments we've retransmitted",
                    &self.labels, &ConstCounter::<u64>::new(info.tcpi_total_retrans.into()))?;

                metrics::collect_family(
                    encoder, "socket_send_congestion_window", "Congestion window for sending",
                    &self.labels, &ConstGauge::<i64>::new(info.tcpi_snd_cwnd.into()))?;

                metrics::collect_family(
                    encoder, "socket_not_sent_bytes", "The amount of data we haven't tried to send yet",
                    &self.labels, &ConstGauge::<i64>::new(info.tcpi_notsent_bytes.into()))?;

                // For some reason tcpi_busy_time includes other metrics, so restore the actual value here
                // (see https://github.com/torvalds/linux/commit/efd90174167530c67a54273fd5d8369c87f9bd32)
                let busy_time = info.tcpi_busy_time - info.tcpi_rwnd_limited - info.tcpi_sndbuf_limited;

                metrics::collect_family(
                    encoder, "socket_busy_seconds", "Time during which send queue was not empty and we were actively sending the data",
                    &self.labels, &metrics::usecs_to_counter(busy_time))?;

                metrics::collect_family(
                    encoder, "socket_stalled_by_receive_window_seconds", "Time during which sending was stalled due to insufficient receive window",
                    &self.labels, &metrics::usecs_to_counter(info.tcpi_rwnd_limited))?;

                metrics::collect_family(
                    encoder, "socket_stalled_by_insufficient_send_buffer_seconds", "Time during which sending was stalled due to insufficient send buffer",
                    &self.labels, &metrics::usecs_to_counter(info.tcpi_sndbuf_limited))?;
            },
            Err(err) => {
                error!("[{}] Failed to get TCP socket info: {err}.", self.name);
            },
        }

        match util::get_tcp_socket_unread_bytes(fd) {
            Ok(unread_bytes) => {
                metrics::collect_family(
                    encoder, "socket_not_read_bytes", "The amount of queued unread data in the receive buffer",
                    &self.labels, &ConstGauge::<i64>::new(unread_bytes))?;
            },
            Err(err) => {
                error!("[{}] Failed to obtain TCP socket unread bytes info: {err}.", self.name);
            },
        }

        match util::get_socket_unsent_bytes(fd) {
            Ok(unsent_bytes) => {
                metrics::collect_family(
                    encoder, "socket_not_transferred_bytes", "The amount of unsent data in the socket send queue (unacked or which we haven't tried to send yet)",
                    &self.labels, &ConstGauge::<i64>::new(unsent_bytes))?;
            },
            Err(err) => {
                error!("[{}] Failed to get TCP unsent bytes info: {err}.", self.name);
            },
        }

        self.collect_ip_socket(encoder, fd)
    }

    pub fn collect_udp_socket<F: AsFd>(&self, encoder: &mut DescriptorEncoder, fd: &F) -> std::fmt::Result {
        match util::get_socket_unsent_bytes(fd) {
            Ok(unsent_bytes) => {
                metrics::collect_family(
                    encoder, "socket_not_sent_bytes", "The amount of data in the local send queue",
                    &self.labels, &ConstGauge::<i64>::new(unsent_bytes))?;
            },
            Err(err) => {
                error!("[{}] Failed to obtain UDP socket unsent bytes info: {err}.", self.name);
            },
        }

        self.collect_ip_socket(encoder, fd)
    }

    fn collect_ip_socket<F: AsFd>(&self, encoder: &mut DescriptorEncoder, fd: &F) -> std::fmt::Result {
        match nix::sys::socket::getsockopt(fd, nix::sys::socket::sockopt::RcvBuf) {
            Ok(size) => {
                metrics::collect_family(
                    encoder, "socket_receive_buffer_bytes", "Socket receive buffer size",
                    &self.labels, &ConstGauge::<i64>::new(size as i64))?;
            },
            Err(err) => {
                error!("[{}] Failed to obtain socket receive buffer size: {err}.", self.name);
            },
        }

        match nix::sys::socket::getsockopt(fd, nix::sys::socket::sockopt::SndBuf) {
            Ok(size) => {
                metrics::collect_family(
                    encoder, "socket_send_buffer_bytes", "Socket send buffer size",
                    &self.labels, &ConstGauge::<i64>::new(size as i64))?;
            },
            Err(err) => {
                error!("[{}] Failed to get send buffer size: {err}.", self.name);
            },
        }

        Ok(())
    }
}