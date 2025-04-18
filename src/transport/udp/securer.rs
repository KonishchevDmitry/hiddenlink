use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use aes::Block;
use aes::cipher::{BlockDecrypt, BlockEncrypt, BlockSizeUser, KeyInit};
use base64::Engine;
use bytes::{BufMut, BytesMut};
use rand::Rng;
use serde_derive::{Serialize, Deserialize};
use shadowsocks_crypto::CipherKind;
use validator::Validate;

use crate::core::GenericResult;

const METHOD: CipherKind = CipherKind::AEAD2022_BLAKE3_AES_128_GCM;
use shadowsocks_crypto::v2::udp::AesGcmCipher as AeadCipher;
use aes::Aes128 as BlockCipher;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UdpConnectionSecurerConfig {
    secret: String, // Generation: `openssl rand -base64 16`
}

// Inspired by Shadowsocks.
// See https://github.com/Shadowsocks-NET/shadowsocks-specs/blob/main/2022-1-shadowsocks-2022-edition.md for details.
pub struct UdpConnectionSecurer {
    method: CipherKind,
    key: Vec<u8>,
    session: Session,
    header_cipher: BlockCipher,
    peer_session: Mutex<Option<PeerSession>>,
}

impl UdpConnectionSecurer {
    pub fn new(config: &UdpConnectionSecurerConfig) -> GenericResult<UdpConnectionSecurer> {
        let method = METHOD;
        let key = decode_secret(method, &config.secret)?;
        let session = Session::new(method, &key);
        let header_cipher = BlockCipher::new_from_slice(&key).unwrap();

        Ok(UdpConnectionSecurer {
            method,
            key,
            session,
            header_cipher,
            peer_session: Mutex::default(),
        })
    }

    pub fn encrypt(&self, buf: &mut BytesMut, payload: &[u8]) {
        let session_id = self.session.session_id;
        let timestamp = now_timestamp();
        let packet_id = self.session.packet_id.fetch_add(1, Ordering::Relaxed);

        let header_size =
            std::mem::size_of_val(&session_id) +
            std::mem::size_of_val(&timestamp) +
            std::mem::size_of_val(&packet_id);

        let tag_size = self.method.tag_len();

        buf.truncate(0);
        buf.reserve(header_size + payload.len() + tag_size);

        buf.put_u64(session_id);
        buf.put_u32(timestamp);
        buf.put_u32(packet_id);
        buf.put_slice(payload);

        unsafe {
            buf.advance_mut(tag_size);
        }

        // The header is encrypted with AES-ECB and payload with AES-GCM

        let (header, payload_and_tag) = buf.split_at_mut(header_size);

        let nonce = &header[4..16];
        debug_assert_eq!(nonce.len(), self.method.nonce_len());
        self.session.cipher.encrypt_packet(nonce, payload_and_tag);

        debug_assert_eq!(header.len(), BlockCipher::block_size());
        self.header_cipher.encrypt_block(Block::from_mut_slice(header));
    }

    pub fn decrypt<'a>(&self, payload: &'a mut [u8]) -> Result<&'a mut [u8], DecryptError> {
        let header_size = 16;
        let tag_size = self.method.tag_len();

        if payload.len() <= header_size + tag_size {
            return Err(DecryptError::TooSmallPacket(payload.len()));
        }

        let (header, payload_and_tag) = payload.split_at_mut(header_size);

        debug_assert_eq!(header.len(), BlockCipher::block_size());
        self.header_cipher.decrypt_block(Block::from_mut_slice(header));

        let mut last_peer_session = self.peer_session.lock().unwrap();
        let session_id = u64::from_be_bytes(header[0..8].try_into().unwrap());
        let timestamp = u32::from_be_bytes(header[8..12].try_into().unwrap());
        let nonce = &header[4..16];

        match last_peer_session.as_ref() {
            Some(session) if session.session_id == session_id => {
                if !session.cipher.decrypt_packet(nonce, payload_and_tag) {
                    return Err(DecryptError::InvalidPayload);
                }
            },
            _ => {
                let session = PeerSession::new(self.method, &self.key, session_id);
                if !session.cipher.decrypt_packet(nonce, payload_and_tag) {
                    return Err(DecryptError::InvalidPayload);
                }
                last_peer_session.replace(session);
            },
        }

        let time_diff: i64 = i64::from(timestamp) - i64::from(now_timestamp());
        if !(-20..=10).contains(&time_diff) {
            return Err(DecryptError::InvalidTimestamp(time_diff));
        }

        let payload_size = payload.len();
        Ok(&mut payload[header_size..payload_size - tag_size])
    }
}

struct Session {
    session_id: u64,
    packet_id: AtomicU32,
    cipher: AeadCipher,
}

impl Session {
    fn new(method: CipherKind, key: &[u8]) -> Session {
        let mut rng = rand::rng();
        let session_id = rng.random();

        Session {
            session_id,
            packet_id: AtomicU32::new(rng.random()),
            cipher: AeadCipher::new(method, key, session_id)
        }
    }
}

struct PeerSession {
    session_id: u64,
    cipher: AeadCipher,
}

impl PeerSession {
    fn new(method: CipherKind, key: &[u8], session_id: u64) -> PeerSession {
        PeerSession {
            session_id,
            cipher: AeadCipher::new(method, key, session_id)
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DecryptError {
    #[error("the packet is too small ({0} bytes)")]
    TooSmallPacket(usize),

    #[error("invalid payload")]
    InvalidPayload,

    #[error("invalid timestamp: {0} seconds from now")]
    InvalidTimestamp(i64),
}

fn decode_secret(method: CipherKind, secret: &str) -> GenericResult<Vec<u8>> {
    let key = base64::engine::general_purpose::STANDARD.decode(secret).map_err(|e| format!(
        "Invalid secret: must be base64-encoded string: {e}"))?;

    if key.len() != method.key_len() {
        return Err!("Invalid secret: got {} bytes key when {} is expected", key.len(), method.key_len());
    }

    Ok(key)
}

fn now_timestamp() -> u32 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).ok()
        .and_then(|duration| duration.as_secs().try_into().ok())
        .expect("Invalid system time")
}

#[cfg(test)]
mod test {
    use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr};

    use matches::assert_matches;
    use shadowsocks::{
        config::{ServerConfig, ServerType},
        context::Context,
        relay::socks5::Address,
        relay::udprelay::proxy_socket::ProxySocket,
    };

    use crate::constants;
    use super::*;

    #[test]
    fn securer() {
        let config = UdpConnectionSecurerConfig {
            secret: "2RUSCKTOeOhb9QSuTWbijw==".to_owned()
        };

        let message = "some secret message".as_bytes();
        let expected_size = message.len() + 32;

        let mut client = UdpConnectionSecurer::new(&config).unwrap();
        let server = UdpConnectionSecurer::new(&config).unwrap();

        let mut encrypted = None;
        let mut buf = BytesMut::new();

        for pass in 1..=4 {
            if pass == 3 {
                client = UdpConnectionSecurer::new(&config).unwrap();
            }

            client.encrypt(&mut buf, message);
            assert_eq!(buf.len(), expected_size);

            if encrypted.is_none() {
                encrypted.replace(buf.clone());
            }

            let result = server.decrypt(&mut buf).unwrap();
            assert_eq!(result, message);
        }

        let encrypted = encrypted.unwrap();

        for byte_pos in 0..encrypted.len() {
            for bit in 0..8 {
                let mask = 1 << bit;

                let mut encrypted = encrypted.clone();
                encrypted[byte_pos] ^= mask;

                assert_matches!(server.decrypt(&mut buf), Err(DecryptError::InvalidPayload));
            }
        }
    }

    #[tokio::test]
    async fn shadowsocks() {
        let method = CipherKind::AEAD2022_BLAKE3_AES_128_GCM;
        let secret = "2RUSCKTOeOhb9QSuTWbijw==";
        let mut server_address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

        let server_config = ServerConfig::new(server_address, secret, method).unwrap();
        let server = ProxySocket::bind(Context::new_shared(ServerType::Server), &server_config).await.unwrap();
        server_address = server.local_addr().unwrap();

        let client_config = ServerConfig::new(server_address, secret, method).unwrap();
        let client = ProxySocket::connect(Context::new_shared(ServerType::Local), &client_config).await.unwrap();

        let fake_target_address = Address::SocketAddress(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1234)));
        let mut buf = vec![0u8; constants::MTU - constants::IPV4_HEADER_SIZE - constants::UDP_HEADER_SIZE];

        for pass in 1..=2 {
            let client_message = format!("client message {pass}");
            client.send(&fake_target_address, client_message.as_bytes()).await.unwrap();

            let (size, peer_addr, remote_addr, _) = server.recv_from(&mut buf).await.unwrap();
            assert_eq!(peer_addr, client.local_addr().unwrap());
            assert_eq!(remote_addr, fake_target_address);
            assert_eq!(&buf[..size], client_message.as_bytes());

            let server_message = format!("server message {pass}");
            server.send_to(peer_addr, &remote_addr, server_message.as_bytes()).await.unwrap();

            let (size, remote_addr, _) = client.recv(&mut buf).await.unwrap();
            assert_eq!(remote_addr, fake_target_address);
            assert_eq!(&buf[..size], server_message.as_bytes());
        }
    }
}