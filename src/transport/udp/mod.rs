mod securer;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::BytesMut;
use log::{trace, info, error};
use prometheus_client::encoding::DescriptorEncoder;
use serde_derive::{Serialize, Deserialize};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio::time::{self, Duration, Instant};
use validator::Validate;

use crate::constants;
use crate::core::{GenericResult, EmptyResult};
use crate::metrics::{self, TransportLabels};
use crate::transport::{Transport, TransportDirection, MeteredTransport};
use crate::transport::udp::securer::UdpConnectionSecurer;
use crate::transport::stat::TransportConnectionStat;
use crate::tunnel::Tunnel;
use crate::util;

pub use self::securer::UdpConnectionSecurerConfig;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct InsecureUdpTransportConfig {
    bind_address: SocketAddr,
    peer_address: SocketAddr,
}

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct SecureUdpTransportConfig {
    #[validate]
    #[serde(flatten)]
    pub udp: InsecureUdpTransportConfig,

    #[validate]
    #[serde(flatten)]
    pub securer: UdpConnectionSecurerConfig,
}

pub struct UdpTransport {
    name: String,
    labels: TransportLabels,
    socket: UdpSocket,
    securer: Option<UdpConnectionSecurer>,
    encrypt_buffer: AsyncMutex<BytesMut>,
    peer_address: SocketAddr,
    state: Mutex<State>,
    ping_needed: Notify,
    stat: TransportConnectionStat,
}

impl UdpTransport {
    pub async fn new(
        name: &str, config: &InsecureUdpTransportConfig, securer_config: Option<&UdpConnectionSecurerConfig>, tunnel: Arc<Tunnel>,
    ) -> GenericResult<Arc<dyn MeteredTransport>> {
        let securer = securer_config.map(UdpConnectionSecurer::new).transpose()?;

        let socket = UdpSocket::bind(&config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let labels = metrics::transport_labels(name);
        let stat = TransportConnectionStat::new(name, name);

        let transport = Arc::new(UdpTransport {
            name: name.to_owned(),
            labels,
            socket,
            securer,
            encrypt_buffer: AsyncMutex::default(),
            peer_address: config.peer_address,
            state: Mutex::new(State::new()),
            ping_needed: Notify::new(),
            stat,
        });

        info!("[{}] Listening on {}.", transport.name, config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.receiver(tunnel).await
            });
        }

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.pinger().await
            });
        }

        Ok(transport)
    }

    async fn receiver(&self, tunnel: Arc<Tunnel>) {
        let max_size = constants::MTU - constants::IPV4_HEADER_SIZE - constants::UDP_HEADER_SIZE;
        let mut buf = BytesMut::zeroed(max_size + 1);

        loop {
            let size = match self.socket.recv_from(&mut buf).await {
                Ok((size, peer_address)) => {
                    if self.securer.is_some() && peer_address != self.peer_address {
                        trace!("[{}] Drop message from unknown peer ({peer_address}).", self.name);
                        continue;
                    }

                    if size == 0 {
                        error!("[{}] Got an empty message from the peer.", self.name);
                        continue;
                    } else if size > max_size {
                        // tokio doesn't support MSG_TRUNC yet, so handle it manually
                        error!("[{}] Got a too big message from the peer. Drop it.", self.name);
                        continue;
                    }

                    size
                },
                Err(err) => {
                    error!("[{}] Failed to receive a message from the peer: {}.", self.name, err);
                    break;
                }
            };

            let packet = match self.securer {
                Some(ref securer) => match securer.decrypt(&mut buf[..size]) {
                    Ok(packet) => packet,
                    Err(err) => {
                        error!("[{}] Drop packet: {err}.", self.name);
                        continue;
                    }
                },
                None => &mut buf[..size],
            };

            let egress_works = packet[0] & INGRESS_STATE_MARKER != 0;
            packet[0] &= !INGRESS_STATE_MARKER;
            self.on_ingress_packet(egress_works);

            if packet.len() == 1 {
                trace!("[{}] Got a keep alive packet (egress_works={egress_works}).", self.name);
                continue;
            }

            util::trace_packet(&self.name, packet);
            self.stat.on_packet_received(packet);

            if let Err(err) = tunnel.send(packet).await {
                error!("[{}] {}.", self.name, err);
            }
        }
    }

    async fn pinger(&self) {
        let mut encrypted = BytesMut::new();
        let mut next_ping_time = Instant::now();

        loop {
            tokio::select! {
                _ = time::sleep_until(next_ping_time) => {},
                _ = self.ping_needed.notified() => {},
            };

            let now = Instant::now();
            let ingress_works = {
                let mut state = self.state.lock().unwrap();

                if state.needs_connection_acknowledge {
                    state.needs_connection_acknowledge = false;
                } else if let Some(last_packet_time) = state.last_egress_packet_time {
                    next_ping_time = last_packet_time + KEEP_ALIVE_INTERVAL;
                    if next_ping_time > now {
                        continue;
                    }
                };

                match state.last_ingress_packet {
                    Some(last_packet) => !last_packet.is_timed_out(now),
                    None => false,
                }
            };

            trace!("[{}] Sending keep alive message (ingress_works={ingress_works})...", self.name);

            let message: [u8; 1] = [if ingress_works {
                INGRESS_STATE_MARKER
            } else {
                0
            }];

            let result = match self.securer.as_ref() {
                Some(securer) => {
                    securer.encrypt(&mut encrypted, &message);
                    self.socket.send_to(&encrypted, self.peer_address).await
                },
                None => {
                    self.socket.send_to(&message, self.peer_address).await
                },
            };

            let send_time = self.on_egress_packet();
            if let Err(err) = result {
                trace!("[{}] Failed to send keep alive message: {err}.", self.name);
            }

            next_ping_time = send_time + KEEP_ALIVE_INTERVAL;
        }
    }

    fn on_egress_packet(&self) -> Instant {
        let packet_time = Instant::now();
        self.state.lock().unwrap().last_egress_packet_time = Some(packet_time);
        packet_time
    }

    fn on_ingress_packet(&self, egress_works: bool) {
        let now = Instant::now();

        let mut is_first_packet = false;
        let mut was_connected = false;

        {
            let mut state = self.state.lock().unwrap();

            match state.last_ingress_packet {
                Some(last_packet) if !last_packet.is_timed_out(now) => {
                    was_connected = last_packet.egress_works;
                },
                _ => {
                    is_first_packet = true;
                    state.needs_connection_acknowledge = true;
                },
            };

            state.last_ingress_packet = Some(IngressPacket {
                time: now,
                egress_works,
            });
        }

        if is_first_packet {
            trace!("[{}] Got a first ingress packet (egress_works={egress_works}).", self.name);
            self.ping_needed.notify_one();
        }

        if egress_works && !was_connected {
            trace!("[{}] Connected.", self.name);
        }
    }
}

#[async_trait]
impl Transport for UdpTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn direction(&self) -> TransportDirection {
        TransportDirection::all()
    }

    fn connected(&self) -> bool {
        let state = *self.state.lock().unwrap();
        state.connected()
    }

    fn ready_for_sending(&self) -> bool {
        self.connected()
    }

    fn collect(&self, encoder: &mut DescriptorEncoder) -> std::fmt::Result {
        self.stat.collect(encoder)?;
        self.stat.collect_udp_socket(encoder, &self.socket)
    }

    async fn send(&self, packet: &mut [u8]) -> EmptyResult {
        if let Some(last_ingress_packet) = self.state.lock().unwrap().last_ingress_packet {
            if !last_ingress_packet.is_timed_out(Instant::now()) {
                packet[0] |= INGRESS_STATE_MARKER;
            }
        }

        let result = match self.securer.as_ref() {
            Some(securer) => {
                let mut encrypted = self.encrypt_buffer.lock().await;
                securer.encrypt(&mut encrypted, packet);
                self.socket.send_to(&encrypted, self.peer_address).await
            },
            None => {
                self.socket.send_to(packet, self.peer_address).await
            },
        };

        self.on_egress_packet();
        if let Err(err) = result {
            self.stat.on_packet_dropped();
            return Err(err.into());
        }

        self.stat.on_packet_sent(packet);
        Ok(())
    }
}

impl MeteredTransport for UdpTransport {
    fn labels(&self) -> &TransportLabels {
        &self.labels
    }
}

// This bit is always zero in IPv4/IPv6 packets, so we use it to store current ingress state
const INGRESS_STATE_MARKER: u8 = 1 << 4;

const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(3);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
struct State {
    last_ingress_packet: Option<IngressPacket>,
    last_egress_packet_time: Option<Instant>,
    needs_connection_acknowledge: bool,
}

impl State {
    fn new() -> State {
        State {
            last_ingress_packet: None,
            last_egress_packet_time: None,
            needs_connection_acknowledge: false,
        }
    }

    fn connected(&self) -> bool {
        match self.last_ingress_packet {
            Some(last_packet) => last_packet.egress_works && !last_packet.is_timed_out(Instant::now()),
            None => false,
        }
    }
}

#[derive(Clone, Copy)]
struct IngressPacket {
    time: Instant,
    egress_works: bool,
}

impl IngressPacket {
    fn is_timed_out(&self, now: Instant) -> bool {
        now.duration_since(self.time) >= KEEP_ALIVE_TIMEOUT
    }
}