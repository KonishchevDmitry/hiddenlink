use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use log::{trace, info, warn, error};
use rustls::ClientConfig;
use rustls::server::Acceptor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_tun::Tun;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;
use crate::transport::http::common::{ConnectionFlags, PacketReader};
use crate::transport::http::proxy_connection::ProxiedConnection;
use crate::transport::http::tls::TlsDomains;
use crate::util;

pub struct ServerConnection {
    name: String,

    secret: Arc<String>,
    domains: Arc<TlsDomains>,

    upstream_addr: SocketAddr,
    upstream_client_config: Arc<ClientConfig>,
}

impl ServerConnection {
    pub fn new(
        peer_addr: SocketAddr, secret: Arc<String>, domains: Arc<TlsDomains>,
        upstream_addr: SocketAddr, upstream_client_config: Arc<ClientConfig>,
    ) -> ServerConnection {
        ServerConnection {
            name: format!("HTTP connection from {}", peer_addr),

            secret,
            domains,

            upstream_addr,
            upstream_client_config,
        }
    }

    pub async fn handle(self, tcp_connection: TcpStream) -> Option<ServerHiddenlinkConnection> {
        let (mut tls_connection, upstream_domain) = self.process_tls_handshake(tcp_connection).await?;

        let hiddenlink_connection = match self.process_hiddenlink_handshake(&mut tls_connection).await {
            Ok(ConnectionType::Hiddenlink(flags, preread_data)) => {
                trace!("[{}] The client has passed hiddenlink handshake.", self.name);
                ServerHiddenlinkConnection::new(self.name, flags, preread_data, tls_connection)
            },
            Ok(ConnectionType::Proxied(preread_data)) => {
                self.process_request_proxying(preread_data, tls_connection, upstream_domain).await;
                return None;
            },
            Err(err) => {
                trace!("[{}] {}.", self.name, err);
                return None;
            },
        };

        Some(hiddenlink_connection)
    }

    async fn process_tls_handshake(&self, tcp_connection: TcpStream) -> Option<(TlsStream<TcpStream>, &str)> {
        let tls_acceptor = LazyConfigAcceptor::new(Acceptor::default(), tcp_connection);
        tokio::pin!(tls_acceptor);

        let tls_handshake = match tls_acceptor.as_mut().await {
            Ok(handshake) => handshake,
            Err(err) => {
                // FIXME(konishchev): Check for ECONNRESET?
                trace!("[{}] TLS handshake failed: {}.", self.name, err);
                if let Some(tcp_connection) = tls_acceptor.take_io() {
                    self.handle_tls_handshake_error(tcp_connection).await;
                }
                return None;
            }
        };

        let client_hello = tls_handshake.client_hello();
        // FIXME(konishchev): ALPN and HTTP/2 support
        let requested_domain = client_hello.server_name();

        let (upstream_domain, config) = self.domains.select(requested_domain).get_config(requested_domain);
        trace!("[{}] SNI: {:?} -> {}.", self.name, requested_domain, upstream_domain);

        let tls_connection = match tls_handshake.into_stream(config).into_fallible().await {
            Ok(tls_connection) => tls_connection,
            Err((err, tcp_connection)) => {
                trace!("[{}] TLS handshake failed: {}.", self.name, err);
                self.handle_tls_handshake_error(tcp_connection).await;
                return None;
            },
        };

        trace!("[{}] TLS handshake completed.", self.name);
        Some((tls_connection, upstream_domain))
    }

    async fn handle_tls_handshake_error(&self, tcp_connection: TcpStream) {
        // Sending some invalid request to get authentic bad request error from upstream server
        let invalid_request = Bytes::from_static(b" ");
        let upstream_domain = self.domains.default_domain.primary_domain();
        self.proxy_request(invalid_request, tcp_connection, true, upstream_domain).await
    }

    async fn process_hiddenlink_handshake(&self, connection: &mut TlsStream<TcpStream>) -> GenericResult<ConnectionType> {
        let secret = self.secret.as_bytes();
        let mut buf = BytesMut::with_capacity(secret.len());

        loop {
            let size = connection.read_buf(&mut buf).await.map_err(|e| format!(
                "Connection is broken: {}", e))?;

            if buf.len() > secret.len() {
                let mut buf = buf.freeze();

                return Ok(if &buf[..secret.len()] == secret {
                    let raw_flags = buf[secret.len()];

                    let flags = ConnectionFlags::from_bits(raw_flags).ok_or_else(|| format!(
                        "Hiddenlink handshake failed: Invalid connection flags: {raw_flags:08b}"))?;

                    ConnectionType::Hiddenlink(flags, buf.split_off(secret.len() + 1))
                } else {
                    ConnectionType::Proxied(buf)
                });
            } else if size == 0 || buf != secret[..buf.len()] {
                return Ok(ConnectionType::Proxied(buf.freeze()));
            }
        }
    }

    async fn process_request_proxying(&self, preread_data: Bytes, connection: TlsStream<TcpStream>, upstream_domain: &str) {
        trace!("[{}] The client hasn't passed hiddenlink handshake. Proxying the request to {}...", self.name, self.upstream_addr);
        self.proxy_request(preread_data, connection, false, upstream_domain).await
    }

    async fn proxy_request<C: AsyncRead + AsyncWrite>(
        &self, preread_data: Bytes, connection: C, use_preread_data_only: bool, upstream_domain: &str,
    ) {
        match ProxiedConnection::new(
            &self.name, preread_data, connection, use_preread_data_only,
            self.upstream_client_config.clone(), self.upstream_addr, upstream_domain,
        ).await {
            Ok(proxied_connection) => {
                proxied_connection.handle().await;
            },
            Err(err) => {
                warn!("[{}] Failed to proxy client connection to upstream server {}: {}.",
                    self.name, self.upstream_addr, err);
            },
        };
    }
}

enum ConnectionType {
    Hiddenlink(ConnectionFlags, Bytes),
    Proxied(Bytes),
}

// XXX(konishchev): Move to separate module
pub struct ServerHiddenlinkConnection {
    name: String,
    flags: ConnectionFlags,
    writer: Mutex<WriteHalf<TlsStream<TcpStream>>>,
    packet_reader: Mutex<PacketReader<ReadHalf<TlsStream<TcpStream>>>>,
}

impl ServerHiddenlinkConnection {
    fn new(name: String, flags: ConnectionFlags, preread_data: Bytes, connection: TlsStream<TcpStream>) -> ServerHiddenlinkConnection {
        let (reader, writer) = tokio::io::split(connection);

        ServerHiddenlinkConnection {
            name,
            flags,
            writer: Mutex::new(writer),
            packet_reader:Mutex::new(PacketReader::new(preread_data, reader)),
        }
    }

    pub async fn handle(&self, tun: Arc<Tun>) {
        let mut packet_reader = self.packet_reader.lock().await;

        loop {
            let packet = match packet_reader.read().await {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    info!("[{}]: The client has closed the connection.", self.name);
                    break;
                },
                Err(err) => {
                    warn!("[{}]: {err}.", self.name);
                    return;
                }
            };

            if !self.flags.contains(ConnectionFlags::EGRESS) {
                error!("[{}] Got a packet from non-egress connection.", self.name);
                continue;
            }

            util::trace_packet(&self.name, packet);

            if let Err(err) = tun.send(packet).await {
                error!("[{}] Failed to send packet to tun device: {err}.", self.name);
            }
        }

        // FIXME(konishchev): Shutdown
    }
}

#[async_trait] // FIXME(konishchev): Deprecate it
impl Transport for ServerHiddenlinkConnection {
    fn name(&self) -> &str {
        &self.name
    }

    // XXX(konishchev): HERE
    // FIXME(konishchev): Implement
    // FIXME(konishchev): Check socket buffers?
    fn is_ready(&self) -> bool {
        self.flags.contains(ConnectionFlags::INGRESS)
    }

    // FIXME(konishchev): Implement
    async fn send(&self, packet: &[u8]) -> EmptyResult {
        let mut writer = self.writer.lock().await;
        Ok(writer.write_all(packet).await?)
    }
}