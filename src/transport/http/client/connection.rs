use std::os::fd::AsRawFd;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use log::{trace, info, warn, error};
use prometheus_client::encoding::DescriptorEncoder;
use rand::Rng;
use rustls::ClientConfig;
use rustls::pki_types::{ServerName, DnsName};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::oneshot::{self, Receiver};
use tokio::task::JoinHandle;
use tokio::time::{self, Instant, Duration};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use crate::core::{EmptyResult, GenericResult};
use crate::transport::{Transport, TransportDirection};
use crate::transport::http::common::{self, ConnectionFlags, PacketReader, PacketWriter, generate_random_payload,
    pre_configure_hiddenlink_socket, post_configure_hiddenlink_socket};
use crate::transport::stat::TransportConnectionStat;
use crate::tunnel::Tunnel;
use crate::util;

pub struct ConnectionConfig {
    pub endpoint: String,
    pub domain: DnsName<'static>,
    pub client_config: Arc<ClientConfig>,
    pub flags: ConnectionFlags,
    pub secret: Vec<u8>,
    pub min_ttl: Duration,
    pub max_ttl: Option<Duration>,
}

pub struct Connection {
    name: String,
    config: Arc<ConnectionConfig>,
    writer: PacketWriter<WriteHalf<ClientTlsStream<TcpStream>>>,
    stat: Arc<TransportConnectionStat>,
}

impl Connection {
    pub fn new(name: String, config: Arc<ConnectionConfig>, stat: Arc<TransportConnectionStat>) -> Connection {
        Connection {
            name: name.clone(),
            config,
            writer: PacketWriter::new(name, stat.clone()),
            stat,
        }
    }

    pub async fn handle(self: Arc<Connection>, tunnel: Arc<Tunnel>) {
        let mut current_connection_id: usize = 0;
        let mut previous_connection: Option<(String, JoinHandle<()>)> = None;
        let mut next_reconnect = Instant::now();

        loop {
            current_connection_id += 1;
            next_reconnect += Duration::from_secs(3);

            let current_connection_name = format!("{}|#{}", self.name, current_connection_id);
            let result = tokio::time::timeout(
                common::CONNECTION_TIMEOUT, self.process_connect(&current_connection_name)
            ).await.unwrap_or_else(|_| Err!("The connection has timed out"));

            match result {
                Ok((connection, ttl)) => {
                    let fd = connection.as_raw_fd();
                    let (reader, writer) = tokio::io::split(connection);
                    let (error_sender, error_receiver) = oneshot::channel();
                    let (writer_handle, previous_connection_shutdown) = self.writer.replace(
                        &current_connection_name, fd, writer, error_sender);

                    let mut connection_task = {
                        let name = current_connection_name.clone();
                        let client = self.clone();
                        let tunnel = tunnel.clone();

                        tokio::spawn(async move {
                            let graceful_shutdown = client.process_connection(&name, reader, error_receiver, tunnel).await;
                            client.writer.close_matching(writer_handle, graceful_shutdown).await
                        })
                    };

                    if let Some(previous_connection_shutdown) = previous_connection_shutdown {
                        previous_connection_shutdown.await.unwrap();
                    }

                    let expired = match ttl {
                        Some(ttl) => tokio::time::timeout(ttl, &mut connection_task).await.ok(),
                        None => Some((&mut connection_task).await),
                    }.is_none();

                    if let Some((name, task)) = previous_connection.take() {
                        self.close_previous_connection(&name, task).await;
                    }

                    if expired {
                        trace!("[{}] Connection TTL has expired. Spawning new connection...", current_connection_name);
                        previous_connection.replace((current_connection_name, connection_task));
                    }
                },
                Err(err) => {
                    warn!("[{}] Failed to establish hiddenlink connection: {err}.", current_connection_name);
                },
            }

            let nearest_reconnect_at = Instant::now() + Duration::from_secs(1);
            if next_reconnect < nearest_reconnect_at {
                next_reconnect = nearest_reconnect_at;
            }

            time::sleep_until(next_reconnect).await;
        }
    }

    async fn process_connect(&self, name: &str) -> GenericResult<(ClientTlsStream<TcpStream>, Option<Duration>)> {
        let endpoint = &self.config.endpoint;
        info!("[{name}] Establishing new connection to {endpoint}...");

        let tcp_connection = TcpStream::connect(endpoint).await.map_err(|e| format!(
            "Unable to connect: {e}"))?;
        pre_configure_hiddenlink_socket(&tcp_connection)?;

        let domain = ServerName::DnsName(self.config.domain.clone());
        let tls_connector = TlsConnector::from(self.config.client_config.clone());

        let mut tls_connection = tls_connector.connect(domain, tcp_connection).await.map_err(|e| format!(
            "TLS handshake failed: {e}"))?;

        self.process_handshake(&mut tls_connection).await.map_err(|e| format!(
            "Hiddenlink handshake failed: {e}"))?;

        let ttl = self.config.max_ttl.map(|max_ttl| {
            let ttl = rand::rng().random_range(self.config.min_ttl..=max_ttl);
            info!("[{name}] Connected (TTL={ttl:.0?}).");
            ttl
        }).or_else(|| {
            info!("[{name}] Connected.");
            None
        });

        Ok((tls_connection, ttl))
    }

    async fn process_handshake(&self, connection: &mut ClientTlsStream<TcpStream>) -> EmptyResult {
        let name_len: u8 = self.name.len().try_into().map_err(|_| "Transport name is too long")?;

        // Send random payload to mimic HTTP request
        let (fake_http_request, fake_http_request_size) = generate_random_payload(100..=2000);

        let mut request = BytesMut::new();
        request.put_slice(&self.config.secret);
        request.put_u8(self.config.flags.bits());
        request.put_u8(name_len);
        request.put_u16(fake_http_request_size);
        request.put_slice(self.name.as_bytes());
        request.extend(fake_http_request);
        request.put_slice(common::HEADER_SUFFIX);
        connection.write_all(&request).await?;

        let mut fake_http_response_size: [u8; 2] = [0; 2];
        connection.read_exact(&mut fake_http_response_size).await?;
        let response_size: usize = usize::from(u16::from_be_bytes(fake_http_response_size)) + common::HEADER_SUFFIX.len();

        let mut response = BytesMut::with_capacity(response_size);
        unsafe { response.advance_mut(response_size); }
        connection.read_exact(&mut response).await?;

        if !response.ends_with(common::HEADER_SUFFIX) {
            return Err!("Protocol violation error");
        }

        post_configure_hiddenlink_socket(connection.get_ref().0)?;
        Ok(())
    }

    async fn process_connection(
        &self, name: &str, connection: ReadHalf<ClientTlsStream<TcpStream>>, error_receiver: Receiver<String>,
        tunnel: Arc<Tunnel>,
    ) -> bool {
        let mut packet_reader = PacketReader::new(Bytes::new(), connection, error_receiver, self.stat.clone());

        loop {
            let packet = match packet_reader.read().await {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    info!("[{}]: Server has closed the connection.", name);
                    break;
                },
                Err(err) => {
                    warn!("[{}]: {err}.", name);
                    return false;
                }
            };

            util::trace_packet(name, packet);
            if !self.config.flags.contains(ConnectionFlags::INGRESS) {
                error!("[{}] Got a packet from non-ingress connection.", name);
                continue;
            }

            if let Err(err) = tunnel.send(packet).await {
                error!("[{}] {err}.", self.name);
            }
        }

        true
    }

    async fn close_previous_connection(&self, name: &str, task: JoinHandle<()>) {
        if task.is_finished() {
            return
        }

        trace!("[{}] The connection handler is not terminated yet. Killing it...", name);
        task.abort();

        match task.await {
            Ok(_) => {
                trace!("[{}] The connection handler has finished its execution.", name);
            },
            Err(err) => {
                if let Ok(reason) = err.try_into_panic() {
                    std::panic::resume_unwind(reason);
                }
                trace!("[{}] The connection handler has been killed.", name);
            },
        }
    }
}

#[async_trait]
impl Transport for Connection {
    fn name(&self) -> &str {
        &self.name
    }

    fn direction(&self) -> TransportDirection {
        let mut direction = TransportDirection::empty();

        if self.config.flags.contains(ConnectionFlags::INGRESS) {
            direction |= TransportDirection::INGRESS;
        }

        if self.config.flags.contains(ConnectionFlags::EGRESS) {
            direction |= TransportDirection::EGRESS;
        }

        direction
    }

    fn connected(&self) -> bool {
        self.writer.connected()
    }

    fn ready_for_sending(&self) -> bool {
        self.config.flags.contains(ConnectionFlags::EGRESS) && self.writer.connected()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        self.stat.collect(encoder)?;
        self.writer.collect(encoder)
    }

    async fn send(&self, packet: &mut [u8]) -> EmptyResult {
        self.writer.send(packet).await
    }
}