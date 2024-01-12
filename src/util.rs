use std::io;
use std::os::fd::{AsFd, AsRawFd};
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

// FIXME(konishchev): Read man 7 udp
pub fn meter_tcp_socket<F: AsFd>(encoder: &mut DescriptorEncoder, name: &str, fd: &F) -> std::fmt::Result {
	// XXX(konishchev): + transport
	let labels = [(metrics::CONNECTION_LABEL, name)];

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

			metrics::collect_family(
				encoder, "socket_state", "Current socket state",
				&state_labels, &ConstGauge::new(1))?;

			// FIXME(konishchev): Not available yet
			metrics::collect_family(
				encoder, "socket_receive_window_bytes", "Local advertised receive window",
				&labels, &ConstGauge::<i64>::new(info.tcpi_rcv_wnd.into()))?;

			metrics::collect_family(
				encoder, "socket_send_window_bytes", "Peer's advertised receive window",
				&labels, &ConstGauge::<i64>::new(info.tcpi_snd_wnd.into()))?;

			metrics::collect_family(
				encoder, "socket_reordered_packets", "Count of received reordered packets",
				&labels, &ConstCounter::<u64>::new(info.tcpi_reord_seen.into()))?;

			metrics::collect_family(
				encoder, "socket_retransmits", "The number of segments we've retransmitted",
				&labels, &ConstCounter::<u64>::new(info.tcpi_total_retrans.into()))?;

			metrics::collect_family(
				encoder, "socket_send_congestion_window", "Congestion window for sending",
				&labels, &ConstGauge::<i64>::new(info.tcpi_snd_cwnd.into()))?;

			// XXX(konishchev): HERE
			metrics::collect_family(
				encoder, "socket_not_sent_bytes", "Bytes we don't try to sent yet",
				&labels, &ConstGauge::<i64>::new(info.tcpi_notsent_bytes.into()))?;

			// For some reason tcpi_busy_time includes other metrics, so restore the actual value here
			// (see https://github.com/torvalds/linux/commit/efd90174167530c67a54273fd5d8369c87f9bd32)
			let busy_time = info.tcpi_busy_time - info.tcpi_rwnd_limited - info.tcpi_sndbuf_limited;

			metrics::collect_family(
				encoder, "socket_busy_seconds", "Time during which send queue was not empty and we were actively sending the data",
				&labels, &metrics::usecs_to_counter(busy_time))?;

			metrics::collect_family(
				encoder, "socket_stalled_by_receive_window_seconds", "Time during which sending was stalled due to insufficient receive window",
				&labels, &metrics::usecs_to_counter(info.tcpi_rwnd_limited))?;

			metrics::collect_family(
				encoder, "socket_stalled_by_insufficient_send_buffer_seconds", "Time during which sending was stalled due to insufficient send buffer",
				&labels, &metrics::usecs_to_counter(info.tcpi_sndbuf_limited))?;
		},
		Err(err) => {
			error!("[{name}] Failed to get TCP socket info: {err}.");
		},
	}

	match get_tcp_unsent_bytes(fd) {
		Ok(unsent_bytes) => {
			// XXX(konishchev): HERE
			metrics::collect_family(
				encoder, "socket_unsent_bytes", "Bytes we don't sent yet or not acknowledge yet",
				&labels, &ConstGauge::<i64>::new(unsent_bytes))?;
		},
		Err(err) => {
			error!("[{name}] Failed to get TCP unsent bytes info: {err}.");
		},
	}

	match nix::sys::socket::getsockopt(fd, nix::sys::socket::sockopt::RcvBuf) {
		Ok(size) => {
			metrics::collect_family(
				encoder, "socket_receive_buffer_bytes", "Size of socket's receive buffer",
				&labels, &ConstGauge::<i64>::new(size as i64))?;
		},
		Err(err) => {
			error!("[{name}] Failed to get receive buffer size: {err}.");
		},
	}

	match nix::sys::socket::getsockopt(fd, nix::sys::socket::sockopt::SndBuf) {
		Ok(size) => {
			metrics::collect_family(
				encoder, "socket_send_buffer_bytes", "Size of socket's send buffer",
				&labels, &ConstGauge::<i64>::new(size as i64))?;
		},
		Err(err) => {
			error!("[{name}] Failed to get send buffer size: {err}.");
		},
	}

    Ok(())
}

// See `man 7 tcp` for details
fn get_tcp_info<F: AsFd>(fd: &F) -> io::Result<tcp_info> {
	let mut info = tcp_info::default();

	let expected_size = std::mem::size_of_val(&info) as libc::socklen_t;
	let mut size = expected_size;

	if unsafe {
		libc::getsockopt(fd.as_fd().as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_INFO, (&mut info as *mut tcp_info).cast(), &mut size)
	} == -1 {
		return Err(io::Error::last_os_error());
	}

	if size != expected_size {
		debug!("Kernel returned `struct tcp_info` of {size} bytes vs {expected_size} expected.");
	}

	Ok(info)
}

fn get_tcp_unsent_bytes<F: AsFd>(fd: &F) -> io::Result<i64> {
	let mut unsent_bytes: c_int = 0;
	unsafe {
		ioctl_siocoutq(fd.as_fd().as_raw_fd(), &mut unsent_bytes)?;
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

// SIOCOUTQ and SIOCOUTQNSD are both ioctl() commands used in TCP sockets. SIOCOUTQ returns the sum of both the unsent and unacked data in a TCP socket’s send queue, while SIOCOUTQNSD only returns the amount of unsent data in the socket send queue 1.
// XXX(konishchev): SIOCINQ
// `man 7 tcp`: returns the amount of unsent data in the socket send queue.
ioctl_read_bad!(ioctl_siocoutq, bindings::SIOCOUTQ, c_int);