use std::io::ErrorKind;
use std::time::Duration;

use log::error;
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

use crate::core::EmptyResult;

pub fn configure_socket_timeout(connection: &TcpStream, timeout: Duration, keep_alive: Option<TcpKeepalive>) -> EmptyResult {
    let socket = SockRef::from(connection);

    socket.set_tcp_user_timeout(Some(timeout)).map_err(|e| format!(
        "Failed to set TCP_USER_TIMEOUT socket option: {e}"))?;

    if let Some(keep_alive) = keep_alive {
        socket.set_tcp_keepalive(&keep_alive).map_err(|e| format!(
            "Failed to configure TCP keepalive: {e}"))?;
    }

    Ok(())
}

// BBR should be better for transcontinental and mobile connections with periodic packet drops
pub fn configure_congestion_control(connection: &TcpStream) -> EmptyResult {
    let socket = SockRef::from(connection);

    if let Err(err) = socket.set_tcp_congestion("bbr".as_bytes()) {
        if err.kind() != ErrorKind::NotFound {
            return Err!("Failed to set TCP_CONGESTION socket option: {err}");
        }
        error!("Failed to enable BBR TCP congestion control algorithm for a connection: tcp_bbr module is not loaded.");
    }

    Ok(())
}