use std::fs::File;
use std::io::{BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::net::SocketAddr;

use async_trait::async_trait;
use log::{trace, info, error};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_derive::{Serialize, Deserialize};
use tokio::io::{AsyncWriteExt, copy, sink};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use validator::Validate;

use crate::core::{GenericResult, EmptyResult};
use crate::transport::Transport;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpsServerTransportConfig {
    bind_address: SocketAddr,
    cert: PathBuf,
    key: PathBuf,
}

pub struct HttpsServerTransport {
    name: String,
    cert_path: PathBuf,
    key_path: PathBuf,
    acceptor: TlsAcceptor,
}

impl HttpsServerTransport {
    pub async fn new(config: &HttpsServerTransportConfig) -> GenericResult<Arc<dyn Transport>> {
        let acceptor = load_certs(&config.cert, &config.key)?;

        let listener = TcpListener::bind(config.bind_address).await.map_err(|e| format!(
            "Failed to bind to {}: {}", config.bind_address, e))?;

        let transport = Arc::new(HttpsServerTransport{
            name: format!("HTTPS server on {}", config.bind_address),
            cert_path: config.cert.clone(),
            key_path: config.key.clone(),
            acceptor,
        });

        info!("[{}] Listening on {}.", transport.name, config.bind_address);

        {
            let transport = transport.clone();
            tokio::spawn(async move {
                transport.handle(listener).await;
            });
        }

        Ok(transport)
    }

    async fn handle(&self, listener: TcpListener) {
        loop {
            // FIXME(konishchev): Add semaphore
            // FIXME(konishchev): Timeouts?

            let (connection, peer_addr) = match listener.accept().await {
                Ok(result) => result,
                Err(err) => {
                    if err.kind() == ErrorKind::ConnectionAborted {
                        trace!("[{}] Unable to accept a connection: it has been aborted.", self.name);
                    } else {
                        error!("[{}] Failed to accept a connection: {}.", self.name, err)
                    }
                    continue;
                },
            };

            // FIXME(konishchev): Rewrite all below
            trace!("[{} <- {}] Connection accepted.", self.name, peer_addr);
            let acceptor = self.get_tls_acceptor().await;

            tokio::spawn(async move {
                if let Err(err) = HttpsServerTransport::handle_connection(peer_addr, connection, acceptor).await {
                    error!("[{}] {}.", peer_addr, err);
                }
            });
        }
    }

    // FIXME(konishchev): Rewrite
    async fn handle_connection(peer_addr: SocketAddr, tcp_connection: TcpStream, acceptor: TlsAcceptor) -> EmptyResult {
        let tls_connection = acceptor.accept(tcp_connection).await?;
        let upstream_connection = TcpStream::connect("localhost:8080").await?;

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


        // let mut output = sink();
        // tls_connection.write_all(
        //     &b"HTTP/1.0 200 ok\r\n\
        //     Connection: close\r\n\
        //     Content-length: 12\r\n\
        //     \r\n\
        //     Hello world!"[..]).await?;
        // stream.shutdown().await?;
        // copy(&mut stream, &mut output).await?;
        // println!("Hello: {}", peer_addr);

        Ok(())
    }

    async fn get_tls_acceptor(&self) -> TlsAcceptor {
        // FIXME(konishchev): Cache it + async
        load_certs(&self.cert_path, &self.key_path).unwrap_or_else(|e| {
            error!("[{}] {}.", self.name(), e);
            self.acceptor.clone()
        })
    }
}

#[async_trait]
impl Transport for HttpsServerTransport {
    fn name(&self) -> &str {
        &self.name
    }

    // FIXME(konishchev): Implement
    fn is_ready(&self) -> bool {
        false
    }

    // FIXME(konishchev): Implement
    async fn send(&self, _: &[u8]) -> EmptyResult {
        todo!()
    }
}

fn load_certs(cert_path: &Path, key_path: &Path) -> GenericResult<TlsAcceptor> {
    let certs = load_cert(cert_path).map_err(|e| format!(
        "Failed to load certificate {:?}: {}", cert_path, e))?;

    let key = load_key(key_path).map_err(|e| format!(
        "Failed to load private key {:?}: {}", key_path, e))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("Failed to load private key: {}", e))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn load_cert(path: &Path) -> GenericResult<Vec<CertificateDer<'static>>> {
    let mut file = BufReader::new(File::open(path)?);

    let certs = rustls_pemfile::certs(&mut file).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err!("the file doesn't contain any certificate");
    }

    Ok(certs)
}

fn load_key(path: &Path) -> GenericResult<PrivateKeyDer<'static>> {
    let mut file = BufReader::new(File::open(path)?);

    let mut key = Option::None;
    for item in rustls_pemfile::pkcs8_private_keys(&mut file) {
        if key.replace(item?).is_some() {
            return Err!("the file contains more than one private key")
        }
    }

    Ok(key.ok_or("the file doesn't contain any private key")?.into())
}