use std::net::SocketAddr;
use std::time::Duration;

use serde::{Serialize, Deserialize};
use validator::Validate;

pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60); // Standard nginx timeout

#[derive(Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct HttpUpstreamConfig {
    pub address: SocketAddr,
    #[serde(default)]
    pub proxy_protocol: bool,
}