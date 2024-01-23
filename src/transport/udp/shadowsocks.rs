#[cfg(test)]
mod test {
    use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr};

    use shadowsocks::{
        config::{ServerConfig, ServerType},
        context::Context,
        crypto::CipherKind,
        relay::socks5::Address,
        relay::udprelay::proxy_socket::ProxySocket,
    };

    use crate::constants;

    #[tokio::test]
    async fn connection() {
        let method = CipherKind::AEAD2022_BLAKE3_AES_256_GCM;
        let secret = "+ANaPOoJTCPbU2JxzpqVsxJIO6Pec6NBWJD7ouEL49U=";
        let mut server_address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

        let server_config = ServerConfig::new(server_address, secret, method);
        let server = ProxySocket::bind(Context::new_shared(ServerType::Server), &server_config).await.unwrap();
        server_address = server.local_addr().unwrap();

        let client_config = ServerConfig::new(server_address, secret, method);
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