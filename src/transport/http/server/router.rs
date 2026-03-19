use bytes::BytesMut;
use serde::{Serialize, Deserialize};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use validator::Validate;

use crate::core::GenericResult;
use crate::protocols::hiddenlink;
use crate::protocols::http::HttpUpstreamConfig;
use crate::protocols::trojan::{self, TrojanConfig};

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UpstreamsConfig {
    http: HttpUpstreamConfig,
    trojan: Option<TrojanConfig>,
    hiddenlink: HiddenlinkConfig,
}

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HiddenlinkConfig {
    #[validate(non_control_character)]
    #[validate(length(min = "hiddenlink::MIN_SECRET_LEN"))]
    secret: String,
}

impl UpstreamsConfig {
    pub fn as_routing_rules(&self) -> Vec<TunnelProtocolSpec> {
        let mut routing_rules = vec![TunnelProtocolSpec {
            protocol: TunnelProtocol::Hiddenlink,
            header: hiddenlink::encode_secret_for_http1(&self.hiddenlink.secret),
            extra_capacity: Some(hiddenlink::STATIC_HEADER_SIZE),
        }];

        if let Some(trojan) = self.trojan.as_ref() {
            routing_rules.extend(trojan.passwords.iter().map(|password| {
                TunnelProtocolSpec {
                    protocol: TunnelProtocol::Trojan,
                    header: trojan::get_header(password).into_bytes(),
                    extra_capacity: None,
                }
            }));
        }

        routing_rules
    }
}

#[derive(Clone, Copy)]
pub enum TunnelProtocol {
    Hiddenlink,
    Trojan,
}

pub struct TunnelProtocolSpec {
    protocol: TunnelProtocol,
    header: Vec<u8>,
    extra_capacity: Option<usize>,
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