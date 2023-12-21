use std::sync::Arc;
use std::net::SocketAddr;

use log::{trace, error};
use rustls::internal::msgs::handshake;
use rustls::server::Acceptor;
use tokio::io::{copy, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::LazyConfigAcceptor;

use crate::core::EmptyResult;
use crate::transport::http::proxy_connection::ProxyConnection;
use crate::transport::http::tls::TlsDomains;

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
        let tls_acceptor = LazyConfigAcceptor::new(Acceptor::default(), tcp_connection);
        tokio::pin!(tls_acceptor);

        let tls_handshake = match tls_acceptor.as_mut().await {
            Ok(handshake) => handshake,
            Err(err) => {
                if let Some(mut stream) = tls_acceptor.take_io() {
                    stream
                        .write_all(
                            format!("HTTP/1.1 400 Invalid Input\r\n\r\n\r\n{:?}\n", err)
                                .as_bytes()
                        )
                        .await
                        .unwrap();
                }
                return Ok(());
            }
        };

        let client_hello = tls_handshake.client_hello();
        // FIXME(konishchev): ALPN support
        let requested_server_name = client_hello.server_name();

        let (sni_name, config) = self.domains.select(requested_server_name).get_config(requested_server_name);
        trace!(">>> {:?} -> {}", requested_server_name, sni_name);


        let tls_connection = tls_handshake.into_stream(config).await?;


        // let tls_acceptor = self.domains.select("localhost").await;
        // let tls_connection = tls_acceptor.accept(tcp_connection).await?;
        let upstream_connection = TcpStream::connect("localhost:80").await?;

        // let proxy_connection = ProxyConnection::new(config, addr, domain)?;

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