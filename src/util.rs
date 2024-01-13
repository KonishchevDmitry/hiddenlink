use std::io;
use std::os::fd::{AsFd, AsRawFd};
use std::os::raw::c_int;

use itertools::Itertools;
use log::{Level, log_enabled, trace, debug, error};
use nix::ioctl_read_bad;

use crate::bindings::{self, tcp_info};

// See `man 7 tcp` for details
pub fn get_tcp_info<F: AsFd>(fd: &F) -> io::Result<tcp_info> {
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

pub fn get_tcp_socket_unread_bytes<F: AsFd>(fd: &F) -> io::Result<i64> {
	let mut unread_bytes: c_int = 0;
	unsafe {
		ioctl_siocinq(fd.as_fd().as_raw_fd(), &mut unread_bytes)?;
	}
	Ok(unread_bytes.into())
}

pub fn get_socket_unsent_bytes<F: AsFd>(fd: &F) -> io::Result<i64> {
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

// `man 7 tcp`: returns the amount of queued unread data in the receive buffer
//
// We don't have the same ioctl for UDP, but we might get it through /proc/net/udp
// (https://github.com/torvalds/linux/blob/e0f9f0e0737f47f643a66c6db158af61818336bc/net/ipv4/udp.c#L3091)
ioctl_read_bad!(ioctl_siocinq, bindings::SIOCINQ, c_int);

// `man 7 tcp`: returns the amount of unsent data in the socket send queue (unacked or which we haven't tried to send yet)
// `man 7 udp`: returns the amount of data in the local send queue
ioctl_read_bad!(ioctl_siocoutq, bindings::SIOCOUTQ, c_int);