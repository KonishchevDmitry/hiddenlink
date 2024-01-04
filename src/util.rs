use std::io;
use std::os::fd::RawFd;
use std::os::raw::c_int;

use itertools::Itertools;
use log::{Level, log_enabled, trace, debug, error};
use nix::ioctl_read_bad;
use prometheus_client::{
	metrics::counter::ConstCounter,
	metrics::gauge::ConstGauge,
	encoding::DescriptorEncoder,
};

use crate::bindings::{self, tcp_info};
use crate::metrics;

// FIXME(konishchev): Read man 7 socket ip udp
// FIXME(konishchev): Review the metrics
pub fn meter_tcp_socket(encoder: &mut DescriptorEncoder, name: &str, fd: RawFd) -> std::fmt::Result {
	let labels = [(metrics::TRANSPORT_LABEL, name)];

	match get_tcp_info(fd) {
		Ok(info) => {
			let state = match info.tcpi_ca_state.into() {
				bindings::tcp_ca_state_TCP_CA_Open => "ok",
				bindings::tcp_ca_state_TCP_CA_Disorder => "disorder",
				bindings::tcp_ca_state_TCP_CA_CWR => "congestion-window-reduction",
				bindings::tcp_ca_state_TCP_CA_Recovery => "fast-recovery",
				bindings::tcp_ca_state_TCP_CA_Loss => "loss-recovery",
				_ => "unknown",
			};

			let state_labels = labels.iter().cloned().chain([("state", state)]).collect_vec();

			metrics::collect_metric(
				encoder, "socket_state", "Current socket state",
				&state_labels, &ConstGauge::new(1))?;

			metrics::collect_metric(
				encoder, "socket_receive_window", "Local advertised receive window",
				&labels, &ConstGauge::<i64>::new(info.tcpi_rcv_wnd.into()))?;

			metrics::collect_metric(
				encoder, "socket_send_window", "Peer's advertised receive window",
				&labels, &ConstGauge::<i64>::new(info.tcpi_snd_wnd.into()))?;

			metrics::collect_metric(
				encoder, "socket_congestion_send_window", "Congestion window for sending",
				&labels, &ConstGauge::<i64>::new(info.tcpi_snd_cwnd.into()))?;

			metrics::collect_metric(
				encoder, "socket_reordered_packets", "Reordered packets",
				&labels, &ConstCounter::<u64>::new(info.tcpi_reord_seen.into()))?;

			metrics::collect_metric(
				encoder, "socket_retransmits", "Total number of retransmissions",
				&labels, &ConstCounter::<u64>::new(info.tcpi_total_retrans.into()))?;

			metrics::collect_metric(
				encoder, "socket_not_sent_bytes", "Bytes we don't try to sent yet",
				&labels, &ConstGauge::<i64>::new(info.tcpi_notsent_bytes.into()))?;

			metrics::collect_metric(
				encoder, "socket_busy_time", "Time actively sending data (non-empty write queue)",
				&labels, &metrics::usecs_to_counter(info.tcpi_busy_time - info.tcpi_rwnd_limited - info.tcpi_sndbuf_limited))?;

			metrics::collect_metric(
				encoder, "socket_stalled_by_receive_window_time", "Time stalled due to insufficient receive window",
				&labels, &metrics::usecs_to_counter(info.tcpi_rwnd_limited))?;

			metrics::collect_metric(
				encoder, "socket_stalled_by_insufficient_send_buffer_time", "Time stalled due to insufficient send buffer",
				&labels, &metrics::usecs_to_counter(info.tcpi_sndbuf_limited))?;
		},
		Err(err) => {
			error!("[{name}] Failed to get TCP socket info: {err}.");
		},
	}

	match get_tcp_unsent_bytes(fd) {
		Ok(unsent_bytes) => {
			metrics::collect_metric(
				encoder, "socket_unsent_bytes", "Bytes we don't sent yet or not acknowledge yet",
				&labels, &ConstGauge::<i64>::new(unsent_bytes))?;
		},
		Err(err) => {
			error!("[{name}] Failed to get TCP unsent bytes info: {err}.");
		},
	}

    Ok(())
}

// See `man 7 tcp` for details
fn get_tcp_info(fd: RawFd) -> io::Result<tcp_info> {
	let mut info = tcp_info::default();

	let expected_size = std::mem::size_of_val(&info) as libc::socklen_t;
	let mut size = expected_size;

	if unsafe {
		libc::getsockopt(fd, libc::IPPROTO_TCP, libc::TCP_INFO, (&mut info as *mut tcp_info).cast(), &mut size)
	} == -1 {
		return Err(io::Error::last_os_error());
	}

	if size != expected_size {
		debug!("Kernel returned `struct tcp_info` of {size} bytes vs {expected_size} expected.");
	}

	Ok(info)
}

fn get_tcp_unsent_bytes(fd: RawFd) -> io::Result<i64> {
	let mut unsent_bytes: c_int = 0;
	unsafe {
		ioctl_siocoutq(fd, &mut unsent_bytes)?;
	}
	Ok(unsent_bytes.into())
}

pub fn trace_packet(source: &str, data: &[u8]) {
    if !log_enabled!(Level::Trace) {
        return
    }

    match data[0] >> 4 {
        4 => {
            match pktparse::ipv4::parse_ipv4_header(data) {
                Ok((_, header)) if Into::<usize>::into(header.length) == data.len() => {
                    trace!("[{}] Got IPv4 packet ({} bytes): {:?} {} -> {}",
                        source, data.len(), header.protocol, header.source_addr, header.dest_addr);
                },
                _ => {
                    error!("[{}] Got an invalid IPv4 packet ({} bytes): {:02x}.",
                        source, data.len(), data.iter().format(" "));
                },
            }
        },
        6 => {
            match pktparse::ipv6::parse_ipv6_header(data) {
                Ok((rest, header)) => if Into::<usize>::into(header.length) == rest.len() {
                    trace!("[{}] Got IPv6 packet ({} bytes): {:?} {} -> {}",
                        source, data.len(), header.next_header, header.source_addr, header.dest_addr);
                },
                _ => {
                    error!("[{}] Got an invalid IPv6 packet ({} bytes): {:02x}.",
                        source, data.len(), data.iter().format(" "));
                }
            }
        },
        _ => {
            error!("[{}] Got an invalid packet ({} bytes): {}.",
                source, data.len(), format_packet(data));
        },
    }
}

pub fn format_packet(data: &[u8]) -> String {
    format!("{:02x}", data.iter().format(" "))
}

// `man 7 tcp`: returns the amount of unsent data in the socket send queue.
ioctl_read_bad!(ioctl_siocoutq, bindings::SIOCOUTQ, c_int);