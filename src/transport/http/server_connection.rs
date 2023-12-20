use std::sync::Arc;
use std::net::SocketAddr;

use log::error;
use tokio::io::copy;
use tokio::net::TcpStream;

use crate::core::EmptyResult;
use crate::transport::http::tls_domain::TlsDomains;

pub struct ServerConnection {
    peer_addr: SocketAddr,
    domains: Arc<TlsDomains>,
}

impl ServerConnection {
    pub fn new(peer_addr: SocketAddr, domains: Arc<TlsDomains>) -> ServerConnection {
        ServerConnection {peer_addr, domains}
    }

    // FIXME(konishchev): Rewrite
    pub async fn handle(&self, tcp_connection: TcpStream) -> EmptyResult {
        let tls_acceptor = self.domains.get_acceptor("localhost").await;
        let tls_connection = tls_acceptor.accept(tcp_connection).await?;
        let upstream_connection = TcpStream::connect("localhost:80").await?;

        let (mut tls_reader, mut tls_writer) = tokio::io::split(tls_connection);
        let (mut upstream_reader, mut upstream_writer) = tokio::io::split(upstream_connection);

        tokio::spawn(async move {
            if let Err(err) = copy(&mut tls_reader, &mut upstream_writer).await {
                error!(">>> {}", err);
            }
        });

        tokio::spawn(async move {
            if let Err(err) = copy(&mut upstream_reader, &mut tls_writer).await {
                error!(">>> {}", err);
            }
        });


        // stream.shutdown().await?;

        Ok(())
    }
}