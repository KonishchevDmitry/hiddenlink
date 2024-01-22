use std::{io, net::SocketAddr, sync::Arc};

use shadowsocks::{
    config::{ServerConfig, ServerType},
    context::Context,
    crypto::CipherKind,
    relay::{socks5::Address, udprelay::ProxySocket},
};

async fn udp_tunnel_echo(
    server_addr: SocketAddr,
    local_addr: SocketAddr,
    password: &str,
    method: CipherKind,
) -> io::Result<()> {
    let svr_cfg_server = ServerConfig::new(server_addr, password, method);
    let svr_cfg_local = svr_cfg_server.clone();

    let ctx_server = Context::new_shared(ServerType::Server);
    let ctx_local = Context::new_shared(ServerType::Local);

    let svr_cfg_server = Arc::new(svr_cfg_server);
    let context = ctx_server;

    let mut server_socket = ProxySocket::bind(context.clone(), &svr_cfg_server).await.unwrap();
    tokio::spawn(async move {
        let mut recv_buf = vec![0u8; 65536];
        loop {
            let (n, peer_addr, remote_addr, ..) = server_socket.recv_from(&mut recv_buf).await.unwrap();
            let _ = server_socket.send_to(peer_addr, &remote_addr, &recv_buf[..n]).await;
        }
    });

    static SEND_PAYLOAD: &[u8] = b"HELLO WORLD. \012345";
    let svr_cfg = Arc::new(svr_cfg_local);
    let context = ctx_local;
    let server_socket = ProxySocket::connect(context, &svr_cfg).await?;
    server_socket.send(&Address::SocketAddress(server_addr), SEND_PAYLOAD).await?;

    let mut recv_buf = [0u8; 65536];
    let (n, ..) = server_socket.recv(&mut recv_buf).await?;
    assert_eq!(&recv_buf[..n], SEND_PAYLOAD);

    Ok(())
}

#[tokio::test]
async fn udp_tunnel_aead_2022_aes() {
    let server_addr = "127.0.0.1:24001".parse::<SocketAddr>().unwrap();
    let local_addr = "127.0.0.1:24101".parse::<SocketAddr>().unwrap();

    udp_tunnel_echo(
        server_addr,
        local_addr,
        "+ANaPOoJTCPbU2JxzpqVsxJIO6Pec6NBWJD7ouEL49U=",
        CipherKind::AEAD2022_BLAKE3_AES_256_GCM,
    )
    .await
    .unwrap();
}