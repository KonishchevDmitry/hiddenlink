use itertools::Itertools;
use log::{Level, log_enabled, trace, error};

pub fn trace_packet(source: &str, data: &[u8]) {
    if !log_enabled!(Level::Trace) {
        return
    }

    match data[0] >> 4 {
        4 => {
            if let Ok((_, packet)) = pktparse::ipv4::parse_ipv4_header(data) {
                trace!("[{}] Got IPv4 packet ({} bytes): {:?} {} -> {}",
                    source, data.len(), packet.protocol, packet.source_addr, packet.dest_addr);
            } else {
                error!("[{}] Got an invalid IPv4 packet ({} bytes): {:02x}.",
                    source, data.len(), data.iter().format(" "));
            }
        },
        6 => {
            if let Ok((_, packet)) = pktparse::ipv6::parse_ipv6_header(data) {
                trace!("[{}] Got IPv6 packet ({} bytes): {:?} {} -> {}",
                    source, data.len(), packet.next_header, packet.source_addr, packet.dest_addr);
            } else {
                error!("[{}] Got an invalid IPv6 packet ({} bytes): {:02x}.",
                    source, data.len(), data.iter().format(" "));
            }
        },
        _ => {
            error!("[{}] Got an invalid packet ({} bytes): {:02x}.",
                source, data.len(), data.iter().format(" "));
        },
    }
}