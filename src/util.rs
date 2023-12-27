use itertools::Itertools;
use log::{Level, log_enabled, trace, error};

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