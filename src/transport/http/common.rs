use std::marker::Unpin;
use std::os::fd::{RawFd, BorrowedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bitflags::bitflags;
use bytes::{Bytes, BytesMut, Buf};
use prometheus_client::encoding::DescriptorEncoder;
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;

use crate::{constants, util};
use crate::core::{GenericResult, EmptyResult};
use crate::transport::{Transport, TransportDirection};
use crate::transport::stat::TransportConnectionStat;

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
    stat: Arc<TransportConnectionStat>,
}

enum PacketReaderState {
    ReadSize{to_drop: usize},
    ReadPacket{packet_size: usize},
}

impl<C: AsyncReadExt + Unpin> PacketReader<C> {
    pub fn new(preread_data: Bytes, connection: C, stat: Arc<TransportConnectionStat>) -> PacketReader<C> {
        let mut buf = BytesMut::new(); // FIXME(konishchev): Set initial capacity
        buf.extend(&preread_data);

        PacketReader {
            buf,
            connection,
            state: PacketReaderState::ReadSize{to_drop: 0},
            stat,
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
                let packet = &self.buf[..data_size];
                self.stat.on_packet_received(packet);
                self.state = PacketReaderState::ReadSize{to_drop: data_size};
                return Ok(Some(packet));
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

pub struct PacketWriter<C: AsyncWriteExt + Send + Sync + Unpin> {
    name: String,
    writer: Mutex<Option<Arc<PacketWriterConnection<C>>>>,
    stat: Arc<TransportConnectionStat>,
}

impl<C: AsyncWriteExt + Send + Sync + Unpin> PacketWriter<C> {
    pub fn new(name: String, stat: Arc<TransportConnectionStat>) -> PacketWriter<C> {
        PacketWriter {
            name,
            writer: Mutex::default(),
            stat,
        }
    }

    pub fn replace(&self, fd: RawFd, connection: C) {
        self.writer.lock().unwrap().replace(Arc::new(PacketWriterConnection {
            fd,
            connection: AsyncMutex::new(connection),
        }));
    }

    pub fn reset(&self) {
        // FIXME(konishchev): Shutdown
        self.writer.lock().unwrap().take();
    }
 }

#[async_trait]
 impl<C: AsyncWriteExt + Send + Sync + Unpin> Transport for PacketWriter<C> {
    fn name(&self) ->  &str {
        &self.name
    }

    fn direction(&self) -> TransportDirection {
        TransportDirection::EGRESS
    }

    fn connected(&self) -> bool {
        self.writer.lock().unwrap().is_some()
    }

    fn ready_for_sending(&self) -> bool {
        self.connected()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        if let Some(writer) = self.writer.lock().unwrap().clone() {
            self.stat.collect_tcp_socket(encoder, &writer.fd())?;
        }
        Ok(())
    }

    // FIXME(konishchev): Check socket buffers?
    async fn send(&self, packet: &mut [u8]) -> EmptyResult {
        let writer = self.writer.lock().unwrap().as_ref().ok_or_else(|| {
            self.stat.on_packet_dropped();
            "Connection is closed"
        })?.clone();

        writer.connection.lock().await.write_all(packet).await.map_err(|e| {
            self.stat.on_packet_dropped();
            e
        })?;

        self.stat.on_packet_sent(packet);
        // FIXME(konishchev): ioctl_siocoutq: 84 -> 106
        Ok(())
    }
}

struct PacketWriterConnection<C> {
    fd: RawFd,
    connection: AsyncMutex<C>,
}

impl<C> PacketWriterConnection<C> {
    fn fd(&self) -> BorrowedFd {
        unsafe {
            BorrowedFd::borrow_raw(self.fd)
        }
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

// FIXME(konishchev): echo tcp_bbr > /etc/modules-load.d/bbr.conf +https://stackoverflow.com/questions/59265004/how-to-change-tcp-congestion-control-algorithm-using-setsockopt-call-from-chttps://stackoverflow.com/questions/59265004/how-to-change-tcp-congestion-control-algorithm-using-setsockopt-call-from-c
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