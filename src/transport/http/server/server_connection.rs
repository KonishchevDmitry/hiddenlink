use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use itertools::Itertools;
use log::{trace, info, warn, error};
use rustls::ClientConfig;
use rustls::server::Acceptor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::error::Elapsed;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;

use crate::core::{GenericResult, ResultTools};
use crate::transport::http::common::{self, ConnectionFlags, configure_socket_timeout, generate_random_payload,
    pre_configure_hiddenlink_socket, post_configure_hiddenlink_socket};
use crate::transport::http::server::proxied_connection::{ProxiedConnection, ProxyProtocolHeader};
use crate::transport::http::tls::TlsDomains;

pub struct ServerConnection {
    name: String,

    peer_addr: SocketAddr,
    local_addr: SocketAddr,

    secret: Arc<String>,
    domains: Arc<TlsDomains>,

    upstream_addr: SocketAddr,
    upstream_client_config: Arc<ClientConfig>,
    proxy_protocol: bool,
}

impl ServerConnection {
    pub fn new(
        peer_addr: SocketAddr, local_addr: SocketAddr, secret: Arc<String>, domains: Arc<TlsDomains>,
        upstream_addr: SocketAddr, upstream_client_config: Arc<ClientConfig>, proxy_protocol: bool,
    ) -> ServerConnection {
        ServerConnection {
            name: format!("HTTP connection from {}", peer_addr),

            peer_addr,
            local_addr,

            secret,
            domains,

            upstream_addr,
            upstream_client_config,
            proxy_protocol,
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
                info!("[{}] The client has passed hiddenlink handshake as {:?}.", self.name, request.name);
                request
            },
            RoutingDecision::Proxy {connection, upstream_domain, negotiated_protocol, preread_data} => {
                trace!("[{}] The client hasn't passed hiddenlink handshake. Proxying the request to {}...",
                    self.name, self.upstream_addr);
                self.proxy_request(connection, &upstream_domain, negotiated_protocol, preread_data).await;
                return None;
            },
        };

        Some(hiddenlink_connection_request)
    }

    async fn process_handshake(&self, tcp_connection: TcpStream) -> Option<RoutingDecision> {
        let (tls_connection, upstream_domain, negotiated_protocol) = self.process_tls_handshake(tcp_connection).await?;
        self.process_routing_decision(tls_connection, upstream_domain, negotiated_protocol).await.ok_or_handle_error(|err| {
            trace!("[{}] Connection is broken: {err}.", self.name);
        })
    }

    async fn process_tls_handshake(&self, tcp_connection: TcpStream) -> Option<(TlsStream<TcpStream>, &str, Option<Vec<u8>>)> {
        let tls_acceptor = LazyConfigAcceptor::new(Acceptor::default(), tcp_connection);
        tokio::pin!(tls_acceptor);

        let tls_handshake = match tls_acceptor.as_mut().await {
            Ok(handshake) => handshake,
            Err(err) => {
                trace!("[{}] TLS handshake failed: {err}.", self.name);
                if let Some(tcp_connection) = tls_acceptor.take_io() {
                    self.handle_tls_handshake_error(tcp_connection).await;
                }
                return None;
            }
        };

        let client_hello = tls_handshake.client_hello();
        let requested_domain = client_hello.server_name();

        let (upstream_domain, mut tls_config) = self.domains.select(requested_domain).get_config(requested_domain);
        trace!("[{}] SNI: {:?} -> {}.", self.name, requested_domain, upstream_domain);

        // We negotiate only HTTP/2 and HTTP/1.1 protocols explicitly. For all other protocols fallback to no ALPN
        // scheme which is effectively HTTP < 2. It's the safest way in terms of compatibility with upstream server.
        //
        // Testing:
        // openssl s_client -connect server.lan:4000 -alpn h2,http/1.1
        // curl -v --no-alpn https://server.lan:4000/
        // curl -v --http1.1 https://server.lan:4000/
        // curl -v --http2 https://server.lan:4000/
        if let Some(mut alpn) = client_hello.alpn() {
            trace!("[{}] ALPN: {}.", self.name, client_hello.alpn().unwrap().map(String::from_utf8_lossy).join(", "));

            if let Some(protocol) = alpn.find(|&protocol| {
                protocol == common::ALPN_HTTP2 || protocol == common::ALPN_HTTP1
            }) {
                let mut new_config = Arc::unwrap_or_clone(tls_config);
                new_config.alpn_protocols.push(protocol.to_vec());
                tls_config = Arc::new(new_config);
            }
        };

        let mut negotiated_protocol = None;
        let tls_connection = match tls_handshake.into_stream_with(tls_config, |conn| {
            negotiated_protocol = conn.alpn_protocol().map(|protocol| protocol.to_vec());
        }).into_fallible().await {
            Ok(tls_connection) => tls_connection,
            Err((err, tcp_connection)) => {
                trace!("[{}] TLS handshake failed: {err}.", self.name);
                self.handle_tls_handshake_error(tcp_connection).await;
                return None;
            },
        };

        trace!("[{}] TLS handshake completed. Negotiated ALPN: {:?}.",
            self.name, negotiated_protocol.as_ref().map(|value| String::from_utf8_lossy(&value)));

        Some((tls_connection, upstream_domain, negotiated_protocol))
    }

    async fn handle_tls_handshake_error(&self, mut tcp_connection: TcpStream) {
        // nginx detects invalid HTTPS requests and responses with a plain text/HTTP error in such cases. We could use
        // this method to send some invalid request (or capture the original one) to upstream to get authentic bad
        // request error from it – and proxy it back over plain TCP connection, but it doesn't work as expected (and as
        // it's stated in crate docs), because rustls has already sent TLS alert to the socket, so we can't cleanly
        // mimic the upstream behaviour here without manual interception of this alert.
        //
        // But, this behaviour with plain text responses to invalid TLS handshake varies from server to server, so our
        // behaviour won't look odd – just like we have some TLS balancer before the upstream server.

        if let Err(err) = tcp_connection.shutdown().await {
            if err.kind() != ErrorKind::NotConnected {
                error!("[{}] Failed to shutdown client connection: {}.", self.name, err);
            }
        }
    }

    async fn process_routing_decision(
        &self, mut connection: TlsStream<TcpStream>, upstream_domain: &str, negotiated_protocol: Option<Vec<u8>>
    ) -> GenericResult<RoutingDecision> {
        // At this time we don't support hiddenlink over HTTP/2, which is not good in terms of TLS fingerprint of our
        // hiddenlink connections, but we don't bother about this now.
        if let Some(protocol) = negotiated_protocol.as_ref() {
            if protocol.as_slice() != common::ALPN_HTTP1 {
                return Ok(RoutingDecision::Proxy {
                    connection,
                    upstream_domain: upstream_domain.to_owned(),
                    negotiated_protocol,
                    preread_data: Bytes::new(),
                })
            }
        }

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
            connection,
            upstream_domain: upstream_domain.to_owned(),
            negotiated_protocol,
            preread_data: buf.freeze(),
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

        // Send random payload to mimic HTTP response (headers only)
        let (fake_http_response, fake_http_response_size) = generate_random_payload(70..=350);

        let mut response = BytesMut::new();
        response.put_u16(fake_http_response_size);
        response.extend(fake_http_response);
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

    async fn proxy_request<C: AsyncRead + AsyncWrite>(
        &self, connection: C, upstream_domain: &str, negotiated_protocol: Option<Vec<u8>>, preread_data: Bytes,
    ) {
        let mut tls_config = self.upstream_client_config.clone();
        if let Some(protocol) = negotiated_protocol {
            let mut new_config = Arc::unwrap_or_clone(tls_config);
            new_config.alpn_protocols.push(protocol);
            tls_config = Arc::new(new_config);
        }

        let proxy_protocol = self.proxy_protocol.then_some(ProxyProtocolHeader {
            peer_addr: self.peer_addr,
            local_addr: self.local_addr,
        });

        match ProxiedConnection::new(
            &self.name, connection, preread_data, false,
            tls_config, self.upstream_addr, upstream_domain, proxy_protocol,
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
        connection: TlsStream<TcpStream>,
        upstream_domain: String,
        negotiated_protocol: Option<Vec<u8>>,
        preread_data: Bytes,
    },
}

pub struct HiddenlinkConnectionRequest {
    pub name: String,
    pub flags: ConnectionFlags,
    pub preread_data: Bytes,
    pub connection: TlsStream<TcpStream>,
}