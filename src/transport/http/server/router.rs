use bytes::BytesMut;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;

use crate::core::GenericResult;

#[derive(Clone, Copy)]
pub enum TunnelProtocol {
    Hiddenlink,
    Trojan,
}

pub struct TunnelProtocolSpec<'a> {
    pub protocol: TunnelProtocol,
    pub header: &'a [u8],
    pub extra_capacity: Option<usize>,
}

pub struct Router<'a> {
    buf: BytesMut,
    specs: Vec<TunnelProtocolSpec<'a>>,
}

impl<'a> Router<'a> {
    fn new(specs: Vec<TunnelProtocolSpec<'a>>) -> Router<'a> {
        let buf_size = specs.iter().map(|spec| {
            spec.header.len() + spec.extra_capacity.unwrap_or(0)
        }).max().unwrap_or(0);

        return Router {
            buf: BytesMut::with_capacity(buf_size),
            specs,
        }
    }

    async fn route(mut self, connection: &mut TlsStream<TcpStream>) -> GenericResult<(Option<TunnelProtocol>, BytesMut)> {
        loop {
            if connection.read_buf(&mut self.buf).await? == 0 {
                return Ok((None, self.buf));
            }

            let mut partial_match = false;

            for spec in &self.specs {
                let size = std::cmp::min(self.buf.len(), spec.header.len());

                if &self.buf[..size] == &spec.header[..size] {
                    if size == spec.header.len() {
                        return Ok((Some(spec.protocol), self.buf));
                    }
                    partial_match = true;
                }
            }

            if !partial_match {
                return Ok((None, self.buf));
            }
        }
    }
}