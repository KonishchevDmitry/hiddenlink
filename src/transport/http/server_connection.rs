use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use log::{trace, warn};
use rustls::ClientConfig;
use rustls::server::Acceptor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;

use crate::core::GenericResult;
use crate::transport::http::proxy_connection::ProxiedConnection;
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

    pub async fn handle(&self, tcp_connection: TcpStream) {
        let (mut tls_connection, upstream_domain) = match self.process_tls_handshake(tcp_connection).await {
            Some(tls_connection) => tls_connection,
            None => return,
        };

        match self.process_hiddenlink_handshake(&mut tls_connection).await {
            Ok(ConnectionType::Hiddenlink(preread_data)) => {
                self.process_hiddenlink_connection(preread_data, tls_connection).await;
            },
            Ok(ConnectionType::Proxied(preread_data)) => {
                self.process_request_proxying(preread_data, tls_connection, upstream_domain).await;
            },
            Err(err) => {
                trace!("[{}] {}.", self.name, err);
            },
        }
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

            if buf.len() >= secret.len() {
                return Ok(if &buf[..secret.len()] == secret {
                    ConnectionType::Hiddenlink(buf.freeze().split_off(secret.len()))
                } else {
                    ConnectionType::Proxied(buf.freeze())
                });
            } else if size == 0 || buf != &secret[..buf.len()] {
                return Ok(ConnectionType::Proxied(buf.freeze()));
            }
        }
    }

    async fn process_hiddenlink_connection(&self, _preread_data: Bytes, _connection: TlsStream<TcpStream>) {
        trace!("[{}] The client has passed hiddenlink handshake.", self.name);
        // FIXME(konishchev): Implement
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
    Hiddenlink(Bytes),
    Proxied(Bytes),
}