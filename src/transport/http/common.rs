use std::marker::Unpin;

use bitflags::bitflags;
use bytes::{Bytes, BytesMut, Buf};
use tokio::io::AsyncReadExt;

use crate::{constants, util};
use crate::core::GenericResult;

bitflags! {
    #[derive(Clone, Copy)]
    pub struct ConnectionFlags: u8 {
        const INGRESS = 1 << 0;
        const EGRESS  = 1 << 1;
    }
}

pub struct PacketReader<C: AsyncReadExt + Unpin> {
    buf: BytesMut,
    connection: C,
    state: PacketReaderState,
}

impl<C: AsyncReadExt + Unpin> PacketReader<C> {
    pub fn new(preread_data: Bytes, connection: C) -> PacketReader<C> {
        let mut buf = BytesMut::new(); // FIXME(konishchev): Set initial capacity
        buf.extend(&preread_data);

        PacketReader {
            buf,
            connection,
            state: PacketReaderState::ReadSize{to_drop: 0},
        }
    }

    pub async fn read(&mut self) -> GenericResult<Option<&[u8]>> {
        loop {
            let (is_packet, data_size, max_size) = match self.state {
                PacketReaderState::ReadSize{to_drop} => {
                    self.buf.advance(to_drop);
                    self.state = PacketReaderState::ReadSize{to_drop: 0};
                    (false, 6, constants::MTU)
                },
                PacketReaderState::ReadPacket{packet_size} => (true, packet_size, packet_size),
            };

            while self.buf.len() < data_size {
                self.buf.reserve(max_size - self.buf.len());

                let read_size = self.connection.read_buf(&mut self.buf).await.map_err(|e| format!(
                    "Connection is broken: {e}"))?;

                if read_size == 0 {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    return Err!("Connection has been unexpectedly closed by peer");
                }
            }

            if is_packet {
                self.state = PacketReaderState::ReadSize{to_drop: data_size};
                return Ok(Some(&self.buf[..data_size]));
            }

            let version = self.buf[0] >> 4;
            let (min_size, size) = match version {
                4 => (constants::IPV4_HEADER_SIZE, u16::from_be_bytes([self.buf[2], self.buf[3]]).into()),
                6 => (constants::IPV6_HEADER_SIZE, usize::from(u16::from_be_bytes([self.buf[4], self.buf[5]])) + constants::IPV6_HEADER_SIZE),
                _ => return Err!("Got an invalid packet: {}", util::format_packet(&self.buf)),
            };

            if size < min_size || size > constants::MTU {
                return Err!("Got IPv{version} packet with an invalid size: {size}");
            }

            self.state = PacketReaderState::ReadPacket{packet_size: size};
        }
    }
}

enum PacketReaderState {
    ReadSize{to_drop: usize},
    ReadPacket{packet_size: usize},
}