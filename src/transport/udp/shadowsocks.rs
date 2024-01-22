// XXX(konishchev): HERE
use std::net::SocketAddr;
use shadowsocks::{
    config::{ServerConfig, ServerType},
    context::Context,
    crypto::CipherKind,
    relay::{socks5::Address, udprelay::{ProxySocket, proxy_socket::ProxySocketResult}},
};

use crate::core::GenericResult;

struct ShadowsocksConnection {
    socket: ProxySocket,
}

impl ShadowsocksConnection {
    // FIXME(konishchev): What the difference between server/client config?
    async fn new(server_address: SocketAddr, config_type: ServerType, secret: String) -> GenericResult<ShadowsocksConnection> {
        let context = Context::new_shared(config_type);
        let config = ServerConfig::new(server_address, secret, CipherKind::AEAD2022_BLAKE3_AES_256_GCM);

        let socket = match config_type {
            ServerType::Server => {
                ProxySocket::bind(context, &config).await.map_err(|e| format!(
                    "Failed to bind to {}: {}", server_address, e))?
            },
            ServerType::Local => {
                // XXX(konishchev): HERE
                ProxySocket::connect(context, &config).await.unwrap()
            },
        };

        Ok(ShadowsocksConnection {
            socket,
        })
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> ProxySocketResult<(usize, SocketAddr, Address, usize)> {
        self.socket.recv_from(buf).await
    }

    pub async fn send_to(&self, target: SocketAddr, addr: &Address, payload: &[u8]) -> ProxySocketResult<usize> {
        self.socket.send_to(target, addr, payload).await
    }

    pub async fn send(&self, addr: &Address, payload: &[u8]) -> ProxySocketResult<usize> {
        self.socket.send(addr, payload).await
    }
    pub async fn recv(&self, buf: &mut [u8]) -> ProxySocketResult<(usize, Address, usize)> {
        self.socket.recv(buf).await
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn connection() {
        let server_addr = "127.0.0.1:24001".parse::<SocketAddr>().unwrap(); // FIXME(konishchev): Make dynamic?
        let secret = "+ANaPOoJTCPbU2JxzpqVsxJIO6Pec6NBWJD7ouEL49U=";

        let mut buf = vec![0u8; 65536];
        let client_message: &[u8] = b"client message";
        let server_message: &[u8] = b"server message";

        let server = ShadowsocksConnection::new(server_addr, ServerType::Server, secret.to_owned()).await.unwrap();
        let client = ShadowsocksConnection::new(server_addr, ServerType::Local, secret.to_owned()).await.unwrap();

        client.send(&Address::SocketAddress(server_addr), client_message).await.unwrap();

        let (size, peer_addr, remote_addr, ..) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..size], client_message);
        server.send_to(peer_addr, &remote_addr, server_message).await.unwrap();

        let (size, ..) = client.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..size], server_message);
    }
}