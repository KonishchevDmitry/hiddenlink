use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use log::{trace, warn};
use rand::{Rng, distributions::{Alphanumeric, Distribution}};
use rustls::ClientConfig;
use rustls::server::Acceptor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::error::Elapsed;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;

use crate::core::{GenericResult, ResultTools};
use crate::transport::http::common::{self, ConnectionFlags, configure_socket_timeout, pre_configure_hiddenlink_socket,
    post_configure_hiddenlink_socket};
use crate::transport::http::server::proxied_connection::ProxiedConnection;
use crate::transport::http::tls::TlsDomains;

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

    pub async fn handle(self, tcp_connection: TcpStream) -> Option<HiddenlinkConnectionRequest> {
        let process_handshake = tokio::time::timeout(
            common::CONNECTION_TIMEOUT,
            self.process_handshake(tcp_connection));

        let decision = process_handshake.await.ok_or_handle_error(|_err: Elapsed| {
            trace!("[{}] The connection has timed out.", self.name);
        })?;

        let hiddenlink_connection_request = match decision? {
            RoutingDecision::Hiddenlink(request) => {
                trace!("[{}] The client has passed hiddenlink handshake as {:?}.", self.name, request.name);
                request
            },
            RoutingDecision::Proxy {preread_data, connection, upstream_domain} => {
                self.process_request_proxying(preread_data, connection, &upstream_domain).await;
                return None;
            },
        };

        Some(hiddenlink_connection_request)
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
        let mut buf = BytesMut::with_capacity(secret.len() + Self::HIDDENLINK_STATIC_HEADER_SIZE);

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

        configure_socket_timeout(connection.get_ref().0, common::CONNECTION_TIMEOUT, None)?;

        Ok(RoutingDecision::Proxy {
            preread_data: buf.freeze(),
            connection,
            upstream_domain: upstream_domain.to_owned(),
        })
    }

    const HIDDENLINK_STATIC_HEADER_SIZE: usize = 1 + 1 + 2; // flags + name length + HTTP request size

    async fn process_hiddenlink_handshake(&self, mut connection: TlsStream<TcpStream>, mut buf: BytesMut) -> GenericResult<RoutingDecision> {
        let mut index = self.secret.len();

        while buf.len() < index + Self::HIDDENLINK_STATIC_HEADER_SIZE {
            if connection.read_buf(&mut buf).await? == 0 {
                return Err!("Got an unexpected EOF");
            }
        }

        let raw_flags = buf[index];
        index += 1;

        let name_len: usize = buf[index].into();
        index += 1;

        let fake_http_request_size: usize = u16::from_be_bytes(buf[index..index + 2].try_into().unwrap()).into();
        index += 2;

        let handshake_size = index + name_len + fake_http_request_size + common::HEADER_SUFFIX.len();

        while buf.len() < handshake_size {
            buf.reserve(handshake_size - buf.len());

            if connection.read_buf(&mut buf).await? == 0 {
                return Err!("Got an unexpected EOF");
            }
        }

        let raw_name = &buf[index..index + name_len];
        index += name_len;
        index += fake_http_request_size;

        if &buf[index..index + common::HEADER_SUFFIX.len()] != common::HEADER_SUFFIX {
            return Err!("Protocol violation error");
        }
        index += common::HEADER_SUFFIX.len();

        let flags = ConnectionFlags::from_bits(raw_flags).ok_or_else(|| format!(
            "Invalid connection flags: {raw_flags:08b}"))?;

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

        // Send random payload to mimic HTTP response
        let fake_http_response_size: u16 = rand::thread_rng().gen_range(70..=350);

        let mut response = BytesMut::new();
        response.put_u16(fake_http_response_size);
        response.extend(Alphanumeric.sample_iter(rand::thread_rng()).take(fake_http_response_size.into()));
        response.put_slice(common::HEADER_SUFFIX);
        connection.write_all(&response).await?;

        let tcp_connection = connection.get_ref().0;
        pre_configure_hiddenlink_socket(tcp_connection)
            .and_then(|()| post_configure_hiddenlink_socket(tcp_connection))
            .map_err(|e| format!("Failed to configure hiddenlink connection: {e}"))?;

        Ok(RoutingDecision::Hiddenlink(HiddenlinkConnectionRequest {
            name,
            flags,
            preread_data: buf.freeze().split_off(index),
            connection,
        }))
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
    Hiddenlink(HiddenlinkConnectionRequest),
    Proxy {
        preread_data: Bytes,
        connection: TlsStream<TcpStream>,
        upstream_domain: String,
    },
}

pub struct HiddenlinkConnectionRequest {
    pub name: String,
    pub flags: ConnectionFlags,
    pub preread_data: Bytes,
    pub connection: TlsStream<TcpStream>,
}