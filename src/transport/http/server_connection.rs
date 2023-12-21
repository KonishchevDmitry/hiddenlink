use std::sync::Arc;
use std::net::SocketAddr;

use log::error;
use rustls::server::Acceptor;
use tokio::io::copy;
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
        // let tls_acceptor = LazyConfigAcceptor::new(Acceptor::default(), tcp_connection);
        // // tls_acceptor.await?;
        // tokio::pin!(acceptor);

        // match acceptor.as_mut().await {
        //     Ok(start) => {
        //         let clientHello = start.client_hello();
        //         let config = choose_server_config(clientHello);
        //         let stream = start.into_stream(config).await.unwrap();
        //         // Proceed with handling the ServerConnection...
        //     }
        //     Err(err) => {
        //         if let Some(mut stream) = acceptor.take_io() {
        //             stream
        //                 .write_all(
        //                     format!("HTTP/1.1 400 Invalid Input\r\n\r\n\r\n{:?}\n", err)
        //                         .as_bytes()
        //                 )
        //                 .await
        //                 .unwrap();
        //         }
        //     }
        // }


        let tls_acceptor = self.domains.get_acceptor("localhost").await;
        let tls_connection = tls_acceptor.accept(tcp_connection).await?;
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