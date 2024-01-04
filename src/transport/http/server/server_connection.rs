use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use log::{trace, warn};
use rustls::ClientConfig;
use rustls::server::Acceptor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::error::Elapsed;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;

use crate::core::{GenericResult, ResultTools};
use crate::transport::http::common::{ConnectionFlags, configure_socket_timeout, pre_configure_hiddenlink_socket,
    post_configure_hiddenlink_socket};
use crate::transport::http::server::proxied_connection::ProxiedConnection;
use crate::transport::http::server::hiddenlink_connection::HiddenlinkConnection;
use crate::transport::http::tls::TlsDomains;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);  // Standard nginx timeout

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

    pub async fn handle(self, tcp_connection: TcpStream) -> Option<HiddenlinkConnection> {
        let process_handshake = tokio::time::timeout(
            CONNECTION_TIMEOUT,
            self.process_handshake(tcp_connection));

        let decision = process_handshake.await.ok_or_handle_error(|_err: Elapsed| {
            trace!("[{}] The connection has timed out.", self.name);
        })?;

        let hiddenlink_connection = match decision? {
            RoutingDecision::Hiddenlink {name, flags, preread_data, connection} => {
                trace!("[{}] The client has passed hiddenlink handshake as {name:?}.", self.name);
                HiddenlinkConnection::new(name, flags, preread_data, connection)
            },
            RoutingDecision::Proxy {preread_data, connection, upstream_domain} => {
                self.process_request_proxying(preread_data, connection, &upstream_domain).await;
                return None;
            },
        };

        Some(hiddenlink_connection)
    }

    async fn process_handshake(&self, tcp_connection: TcpStream) -> Option<RoutingDecision> {
        let (tls_connection, upstream_domain) = self.process_tls_handshake(tcp_connection).await?;
        self.process_routing_decision(tls_connection, upstream_domain).await.ok_or_handle_error(|err| {
            trace!("[{}] Connection is broken: {err}.", self.name);
        })
    }

    async fn process_tls_handshake(&self, tcp_connection: TcpStream) -> Option<(TlsStream<TcpStream>, &str)> {
        let tls_acceptor = LazyConfigAcceptor::new(Acceptor::default(), tcp_connection);
        tokio::pin!(tls_acceptor);

        let tls_handshake = match tls_acceptor.as_mut().await {
            Ok(handshake) => handshake,
            Err(err) => {
                // FIXME(konishchev): Check for ECONNRESET?
                trace!("[{}] TLS handshake failed: {err}.", self.name);
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
                trace!("[{}] TLS handshake failed: {err}.", self.name);
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

    async fn process_routing_decision(&self, mut connection: TlsStream<TcpStream>, upstream_domain: &str) -> GenericResult<RoutingDecision> {
        let secret = self.secret.as_bytes();

        let mut buf = BytesMut::with_capacity(
            secret.len()
            + Self::HIDDENLINK_STATIC_HEADER_SIZE
            + usize::from(u8::MAX) // name
        );

        loop {
            let size = connection.read_buf(&mut buf).await?;

            if buf.len() >= secret.len() {
                if &buf[..secret.len()] != secret {
                    break;
                }
                return self.process_hiddenlink_handshake(connection, buf).await.map_err(|e| format!(
                    "Hiddenlink handshake failed: {e}").into());
            } else if size == 0 || buf != secret[..buf.len()] {
                break;
            }
        }

        configure_socket_timeout(connection.get_ref().0, CONNECTION_TIMEOUT, None)?;

        Ok(RoutingDecision::Proxy {
            preread_data: buf.freeze(),
            connection,
            upstream_domain: upstream_domain.to_owned(),
        })
    }

    const HIDDENLINK_STATIC_HEADER_SIZE: usize = 2; // flags + name len

    async fn process_hiddenlink_handshake(&self, mut connection: TlsStream<TcpStream>, mut buf: BytesMut) -> GenericResult<RoutingDecision> {
        let mut index = self.secret.len();

        while buf.len() < index + Self::HIDDENLINK_STATIC_HEADER_SIZE {
            if connection.read_buf(&mut buf).await? == 0 {
                return Err!("Got an unexpected EOF");
            }
        }

        let raw_flags = buf[index];
        index += 1;

        let flags = ConnectionFlags::from_bits(raw_flags).ok_or_else(|| format!(
            "Invalid connection flags: {raw_flags:08b}"))?;

        let name_len: usize = buf[index].into();
        index += 1;

        while buf.len() < index + name_len {
            if connection.read_buf(&mut buf).await? == 0 {
                return Err!("Got an unexpected EOF");
            }
        }

        let raw_name = &buf[index..index + name_len];
        index += name_len;

        let name = String::from_utf8(raw_name.into())
            .ok().and_then(|name| {
                if name.is_empty() || name.trim() != name || !name.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c.is_ascii_punctuation() || c == ' '
                }) {
                    None
                } else {
                    Some(name)
                }
            })
            .ok_or_else(|| format!("Got an invalid connection name: {:?}", String::from_utf8_lossy(raw_name)))?;

        let tcp_connection = connection.get_ref().0;
        pre_configure_hiddenlink_socket(tcp_connection)
            .and_then(|()| post_configure_hiddenlink_socket(tcp_connection))
            .map_err(|e| format!("Failed to configure hiddenlink connection: {e}"))?;

        Ok(RoutingDecision::Hiddenlink {
            name,
            flags,
            preread_data: buf.freeze().split_off(index),
            connection,
        })
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

enum RoutingDecision {
    Hiddenlink {
        name: String,
        flags: ConnectionFlags,
        preread_data: Bytes,
        connection: TlsStream<TcpStream>,
    },
    Proxy {
        preread_data: Bytes,
        connection: TlsStream<TcpStream>,
        upstream_domain: String,
    },
}