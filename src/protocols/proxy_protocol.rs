// See http://www.haproxy.org/download/3.0/doc/proxy-protocol.txt for details

use std::net::{SocketAddr, SocketAddrV4};

use bytes::{BufMut, BytesMut};

#[derive(Clone, Copy)]
pub struct ProxyProtocolHeader {
    pub peer_addr: SocketAddr,
    pub local_addr: SocketAddr,
}

impl ProxyProtocolHeader {
    pub fn encode(&self) -> BytesMut {
        let (mut peer_addr, mut local_addr) = (self.peer_addr, self.local_addr);

        if let (SocketAddr::V6(peer), SocketAddr::V6(local)) = (peer_addr, local_addr) {
            if let (Some(peer), Some(local)) = (peer.ip().to_ipv4_mapped(), local.ip().to_ipv4_mapped()) {
                peer_addr = SocketAddr::V4(SocketAddrV4::new(peer, peer_addr.port()));
                local_addr = SocketAddr::V4(SocketAddrV4::new(local, local_addr.port()));
            }
        }

        const SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];
        const VERSION: u8 = 2;
        const CONNECTION_PROXY: u8 = 1;
        const FAMILY_AF_INET: u8 = 1;
        const FAMILY_AF_INET6: u8 = 2;
        const PROTOCOL_STREAM: u8 = 1;

        let mut header = BytesMut::with_capacity(13 + 3 + 36);
        header.put_slice(&SIGNATURE);
        header.put_u8(VERSION << 4 | CONNECTION_PROXY);

        match (peer_addr, local_addr) {
            (SocketAddr::V4(peer), SocketAddr::V4(addr)) => {
                header.put_u8(FAMILY_AF_INET << 4 | PROTOCOL_STREAM);
                header.put_u16(12);
                header.put_slice(&peer.ip().octets());
                header.put_slice(&addr.ip().octets());
                header.put_u16(peer.port());
                header.put_u16(addr.port());
            },
            (SocketAddr::V6(peer), SocketAddr::V6(addr)) => {
                header.put_u8(FAMILY_AF_INET6 << 4 | PROTOCOL_STREAM);
                header.put_u16(36);
                header.put_slice(&peer.ip().octets());
                header.put_slice(&addr.ip().octets());
                header.put_u16(peer.port());
                header.put_u16(addr.port());
            },
            (SocketAddr::V4(_), SocketAddr::V6(_)) | (SocketAddr::V6(_), SocketAddr::V4(_)) => unreachable!(),
        }

        header
    }
}