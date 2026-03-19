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

pub struct TunnelProtocolSpec {
    pub protocol: TunnelProtocol,
    pub header: Vec<u8>,
    pub extra_capacity: Option<usize>,
}

pub struct Router<'a> {
    buf: BytesMut,
    specs: &'a Vec<TunnelProtocolSpec>,
}

impl<'a> Router<'a> {
    pub fn new(specs: &'a Vec<TunnelProtocolSpec>) -> Router<'a> {
        let buf_size = specs.iter().map(|spec| {
            spec.header.len() + spec.extra_capacity.unwrap_or(0)
        }).max().unwrap_or(0);

        return Router {
            buf: BytesMut::with_capacity(buf_size),
            specs,
        }
    }

    pub async fn route(mut self, connection: &mut TlsStream<TcpStream>) -> GenericResult<(Option<(TunnelProtocol, usize)>, BytesMut)> {
        loop {
            if connection.read_buf(&mut self.buf).await? == 0 {
                return Ok((None, self.buf));
            }

            let mut partial_match = false;

            for spec in self.specs {
                let header_size = spec.header.len();
                let size = std::cmp::min(self.buf.len(), header_size);

                if &self.buf[..size] == &spec.header[..size] {
                    if size == header_size {
                        return Ok((Some((spec.protocol, header_size)), self.buf));
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