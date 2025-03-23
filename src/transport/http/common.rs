use std::marker::Unpin;
use std::io::ErrorKind;
use std::os::fd::{RawFd, BorrowedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bitflags::bitflags;
use bytes::{Bytes, BytesMut, Buf};
use log::{trace, error};
use num::ToPrimitive;
use prometheus_client::encoding::DescriptorEncoder;
use rand::{Rng, distributions::{Alphanumeric, Distribution, uniform::{SampleRange, SampleUniform}}};
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::oneshot::{Receiver, Sender};
use tokio::task::JoinHandle;
use urlencoding;

use crate::{constants, util};
use crate::core::{GenericResult, EmptyResult};
use crate::transport::{Transport, TransportDirection};
use crate::transport::stat::TransportConnectionStat;

pub const MIN_SECRET_LEN: u64 = 10;
pub const HEADER_SUFFIX: &[u8] = "hiddenlink!".as_bytes();
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60); // Standard nginx timeout

// Available protocols:
// https://www.iana.org/assignments/tls-extensiontype-values/tls-extensiontype-values.xhtml#alpn-protocol-ids
pub const ALPN_HTTP1: &[u8] = "http/1.1".as_bytes();
pub const ALPN_HTTP2: &[u8] = "h2".as_bytes();

bitflags! {
    #[derive(Clone, Copy)]
    pub struct ConnectionFlags: u8 {
        const INGRESS = 1 << 0;
        const EGRESS  = 1 << 1;
    }
}

pub struct PacketReader<C: AsyncReadExt + Unpin> {
    buf: BytesMut,
    data_size: usize,
    packet_size: Option<usize>,
    preread_data: Option<Bytes>,
    connection: C,
    error_receiver: Receiver<String>,
    stat: Arc<TransportConnectionStat>,
}

impl<C: AsyncReadExt + Unpin> PacketReader<C> {
    pub fn new(preread_data: Bytes, connection: C, error_receiver: Receiver<String>, stat: Arc<TransportConnectionStat>) -> PacketReader<C> {
        let max_packet_size = constants::MTU;

        // Since TLS connection is already buffered, we don't use any BytesMut features like BytesMut::advance() and use
        // it as a simplest buffer to avoid extra copying.
        let mut buf = BytesMut::with_capacity(max_packet_size);
        unsafe { buf.set_len(max_packet_size); }

        let preread_data = if preread_data.is_empty() {
            None
        } else {
            Some(preread_data)
        };

        PacketReader {
            buf,
            data_size: 0,
            packet_size: None,
            preread_data,
            connection,
            error_receiver,
            stat,
        }
    }

    pub async fn read(&mut self) -> GenericResult<Option<&[u8]>> {
        loop {
            if !self.read_data(self.packet_size.unwrap_or(6)).await? {
                return Ok(None);
            }

            if let Some(packet_size) = self.packet_size.take() {
                self.data_size = 0;
                let packet = &self.buf[..packet_size];
                self.stat.on_packet_received(packet);
                return Ok(Some(packet));
            }

            // A fake HTTP response before connection shutdown
            if self.buf[0] == 0 {
                let size = 3 + usize::from(u16::from_be_bytes([self.buf[1], self.buf[2]]));
                if size > self.buf.len() {
                    return Err!("Got an invalid fake HTTP response size: {size}");
                }

                assert!(self.read_data(size).await?);
                self.data_size = 0;

                if self.read_data(1).await? {
                    return Err!("Got an unexpected fake HTTP response not followed by connection shutdown");
                }

                return Ok(None);
            }

            let version = self.buf[0] >> 4;
            let (min_size, size) = match version {
                4 => (constants::IPV4_HEADER_SIZE, u16::from_be_bytes([self.buf[2], self.buf[3]]).into()),
                6 => (constants::IPV6_HEADER_SIZE, usize::from(u16::from_be_bytes([self.buf[4], self.buf[5]])) + constants::IPV6_HEADER_SIZE),
                _ => return Err!("Got an invalid packet header: {}", util::format_packet(&self.buf[..self.data_size])),
            };

            if size < min_size || size > self.buf.len() {
                return Err!("Got IPv{version} packet with an invalid size: {size}");
            }

            self.packet_size.replace(size);
        }
    }

    async fn read_data(&mut self, size: usize) -> GenericResult<bool> {
        while self.data_size < size {
            let read_size = match self.preread_data.as_mut() {
                Some(preread_data) => {
                    let read_size = std::cmp::min(size - self.data_size, preread_data.len());

                    self.buf[self.data_size..self.data_size + read_size].copy_from_slice(&preread_data[..read_size]);
                    preread_data.advance(read_size);
                    if preread_data.is_empty() {
                        self.preread_data = None;
                    }

                    read_size
                },
                None => tokio::select! {
                    biased;

                    result = self.connection.read(&mut self.buf[self.data_size..size]) => result,

                    Ok(result) = &mut self.error_receiver => {
                        return Err(result.into());
                    },
                }.map_err(|e| format!("Connection is broken: {e}"))?,
            };

            if read_size == 0 {
                if self.data_size == 0 {
                    return Ok(false);
                }
                return Err!("Connection has been unexpectedly closed by peer");
            }

            self.data_size += read_size;
        }

        Ok(true)
    }
}

pub struct PacketWriter<C: AsyncWriteExt + Send + Sync + Unpin + 'static> {
    name: String,
    writer: Mutex<Option<Arc<PacketWriterConnection<C>>>>,
    stat: Arc<TransportConnectionStat>,
}

impl<C: AsyncWriteExt + Send + Sync + Unpin + 'static> PacketWriter<C> {
    pub fn new(name: String, stat: Arc<TransportConnectionStat>) -> PacketWriter<C> {
        PacketWriter {
            name,
            writer: Mutex::default(),
            stat,
        }
    }

    pub fn replace(&self, name: &str, fd: RawFd, connection: C, error_sender: Sender<String>) -> (Arc<PacketWriterConnection<C>>, Option<JoinHandle<()>>) {
        let connection = Arc::new(PacketWriterConnection {
            fd,
            name: name.to_owned(),
            connection: AsyncMutex::new(PacketWriterConnectionHandle {
                socket: connection,
                error_sender: Some(error_sender),
            }),
        });

        let previous_connection = self.writer.lock().unwrap().replace(connection.clone());

        (connection, previous_connection.map(|connection| {
            tokio::spawn(async move {
                connection.shutdown(None).await;
            })
        }))
    }

    pub async fn close_matching(&self, handle: Arc<PacketWriterConnection<C>>, shutdown: bool) {
        let writer = {
            let mut writer = self.writer.lock().unwrap();

            match *writer {
                Some(ref current) if Arc::ptr_eq(current, &handle) => writer.take().unwrap(),
                _ => return,
            }
        };

        if shutdown {
            writer.shutdown(None).await;
        }
    }

    pub async fn close(&self, shutdown_with_payload: Option<&[u8]>) {
        let writer = self.writer.lock().unwrap().take();

        if let Some(writer) = writer {
            if let Some(payload) = shutdown_with_payload {
                writer.shutdown(Some(payload)).await;
            }
        }
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
        let writer = self.writer.lock().unwrap().clone();

        if let Some(writer) = writer {
            self.stat.collect_tcp_socket(encoder, &writer.fd())?;
        }

        Ok(())
    }

    async fn send(&self, packet: &mut [u8]) -> EmptyResult {
        let writer = self.writer.lock().unwrap().clone();

        let writer = writer.ok_or_else(|| {
            self.stat.on_packet_dropped();
            "Connection is closed"
        })?;

        {
            let mut connection = writer.connection.lock().await;
            if connection.error_sender.is_none() {
                return Err!("Connection is closed");
            }

            connection.socket.write_all(packet).await.inspect_err(|e| {
                let _ = connection.error_sender.take().unwrap().send(format!("The connection is broken: {e}"));
                self.stat.on_packet_dropped();
            })?;
        }

        self.stat.on_packet_sent(packet);
        Ok(())
    }
}

pub struct PacketWriterConnection<C: AsyncWriteExt + Unpin> {
    fd: RawFd,
    name: String,
    connection: AsyncMutex<PacketWriterConnectionHandle<C>>,
}

impl<C: AsyncWriteExt + Unpin> PacketWriterConnection<C> {
    fn fd(&self) -> BorrowedFd {
        unsafe {
            BorrowedFd::borrow_raw(self.fd)
        }
    }

    async fn shutdown(&self, payload: Option<&[u8]>) {
        self.connection.lock().await.shutdown(&self.name, payload).await;
    }
}

struct PacketWriterConnectionHandle<C: AsyncWriteExt + Unpin> {
    socket: C,
    error_sender: Option<Sender<String>>,
}

impl<C: AsyncWriteExt + Unpin> PacketWriterConnectionHandle<C> {
    async fn shutdown(&mut self, name: &str, payload: Option<&[u8]>) {
        let Some(error_sender) = self.error_sender.take() else {
            return;
        };

        if let Some(payload) = payload {
            if let Err(err) = self.socket.write_all(payload).await {
                if let Err(err) = error_sender.send(format!("The connection is broken: {err}")) {
                    trace!("[{name}] Failed to send shutdown payload: {err}.");
                }
                return;
            }
        }

        if let Err(err) = self.socket.shutdown().await {
            if let Err(err) = error_sender.send(format!("Failed to shutdown the socket: {err}")) {
                error!("[{name}] {err}.");
            }
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

pub fn post_configure_hiddenlink_socket(connection: &TcpStream) -> EmptyResult {
    let socket = SockRef::from(connection);

    // BBR should be better for transcontinental connections with periodic packet drops
    if let Err(err) = socket.set_tcp_congestion("bbr".as_bytes()) {
        if err.kind() != ErrorKind::NotFound {
            return Err!("Failed to set TCP_CONGESTION socket option: {err}");
        }
        error!("Failed to enable BBR TCP congestion control algorithm for hiddenlink connection: tcp_bbr module is not loaded.");
    }

    // When hiddenlink handshake is completed, all subsequent writes to the socket will contain actual packets, so
    // disable Nagle algorithm to not delay them. But please note that small packets can actually reveal our tunnel
    // when we use simplex HTTPS masking.
    connection.set_nodelay(true).map_err(|e| format!(
        "Failed to set TCP_NODELAY socket option: {e}"))?;

    Ok(())
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

// The result secret must be a prefix of a first line of a valid HTTP request – in other case, an attacker will be able
// to brute-force the secret symbol-by-symbol if he knows how exactly the upstream server parses and validates HTTP
// requests.
pub fn encode_secret_for_http1(secret: &str) -> Vec<u8> {
    format!("POST /hiddenlink/v1/connect?secret={}", urlencoding::encode(secret)).into_bytes()
}

pub fn generate_random_payload<R, I>(range: R) -> (impl Iterator<Item=u8>, I)
    where R: SampleRange<I>, I: SampleUniform + PartialOrd + ToPrimitive,
{
    let mut random = rand::thread_rng();
    let size: I = random.gen_range(range);
    (Alphanumeric.sample_iter(random).take(size.to_usize().unwrap()), size)
}