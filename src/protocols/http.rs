use std::net::SocketAddr;
use std::time::Duration;

use serde::{Serialize, Deserialize};
use validator::Validate;

// Available protocols:
// https://www.iana.org/assignments/tls-extensiontype-values/tls-extensiontype-values.xhtml#alpn-protocol-ids
pub const ALPN_HTTP1: &[u8] = "http/1.1".as_bytes();
pub const ALPN_HTTP2: &[u8] = "h2".as_bytes();

pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60); // Standard nginx timeout

#[derive(Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpUpstreamConfig {
    pub address: SocketAddr,
    #[serde(default)]
    pub proxy_protocol: bool,
}