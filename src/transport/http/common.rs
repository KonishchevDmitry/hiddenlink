use std::marker::Unpin;
use std::os::fd::RawFd;
use std::os::raw::{c_int, c_ulong};
use std::time::Duration;

use bitflags::bitflags;
use bytes::{Bytes, BytesMut, Buf};
use nix::ioctl_read_bad;
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;

use crate::{constants, util};
use crate::core::{GenericResult, EmptyResult};

pub const MIN_SECRET_LEN: usize = 10;

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

enum PacketReaderState {
    ReadSize{to_drop: usize},
    ReadPacket{packet_size: usize},
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

pub struct PacketWriter<C: AsyncWriteExt + Unpin> {
    fd: RawFd,
    connection: AsyncMutex<C>,
}

impl<C: AsyncWriteExt + Unpin> PacketWriter<C> {
    pub fn new(fd: RawFd, connection: C) -> PacketWriter<C> {
        PacketWriter {
            fd,
            connection: AsyncMutex::new(connection),
        }
    }

    pub async fn send(&self, packet: &[u8]) -> EmptyResult {
        let mut connection = self.connection.lock().await;
        // FIXME(konishchev): ioctl_siocoutq: 84 -> 106
        connection.write_all(packet).await?;
        Ok(())
    }
}

pub fn pre_configure_hiddenlink_socket(connection: &TcpStream) -> EmptyResult {
    configure_socket_timeout(connection, Duration::from_secs(10), Some(
        TcpKeepalive::new()
            .with_time(Duration::from_secs(5))
            .with_interval(Duration::from_secs(1))
            .with_retries(5)
    ))
}

pub fn post_configure_hiddenlink_socket(connection: &TcpStream) -> EmptyResult {
    // When hiddenlink handshake is completed, all subsequent writes to the socket will contain actual packets, so
    // disable Nagle algorithm to not delay them. But please note that small packets can actually reveal our tunnel
    // when we use simplex HTTPS masking.
    Ok(connection.set_nodelay(true).map_err(|e| format!(
        "Failed to set TCP_NODELAY socket option: {e}"))?)
}

pub fn configure_socket_timeout(connection: &TcpStream, timeout: Duration, keep_alive: Option<TcpKeepalive>) -> EmptyResult {
    let socket = SockRef::from(connection);

    socket.set_tcp_user_timeout(Some(timeout)).map_err(|e| format!(
        "Failed to set TCP_USER_TIMEOUT socket option: {e}"))?;

    if let Some(keep_alive) = keep_alive {
        socket.set_tcp_keepalive(&keep_alive).map_err(|e| format!(
            "Failed to configure TCP keepalive: {e}"))?;
    }

    Ok(())
}

// man 7 tcp: returns the amount of unsent data in the socket send queue.
const SIOCOUTQ: c_ulong = 0x5411;
ioctl_read_bad!(ioctl_siocoutq, SIOCOUTQ, c_int);