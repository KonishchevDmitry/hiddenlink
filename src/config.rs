use std::fs::File;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Serialize, Deserialize};
use validator::{Validate, ValidationErrors};

use crate::core::GenericResult;

pub use crate::transport::http::{HttpClientTransportConfig, HttpServerTransportConfig};
pub use crate::transport::udp::{InsecureUdpTransportConfig, SecureUdpTransportConfig};

#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(skip)]
    pub path: PathBuf,

    #[validate(length(min = 1))]
    pub name: String,

    #[validate(nested)]
    #[validate(length(min = 1))]
    pub transports: Vec<TransportSpec>,

    pub metrics_bind_address: Option<SocketAddr>,
}

impl Config {
    pub fn load(path: &Path) -> GenericResult<Config> {
        let mut config: Config = serde_yaml::from_reader(File::open(path)?)?;
        config.path = path.to_owned();
        config.validate()?;
        Ok(config)
    }
}

#[derive(Serialize, Deserialize, Validate)]
// Don't use #[serde(deny_unknown_fields)] because of #[serde(flatten)]
pub struct TransportSpec {
    pub name: Option<String>,

    #[validate(nested)]
    #[serde(flatten)]
    pub transport: TransportConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all="kebab-case")]
pub enum TransportConfig {
    HttpClient(HttpClientTransportConfig),
    HttpServer(HttpServerTransportConfig),
    InsecureUdp(InsecureUdpTransportConfig),
    Udp(SecureUdpTransportConfig),
}

impl Validate for TransportConfig {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            TransportConfig::HttpClient(t) => t.validate(),
            TransportConfig::HttpServer(t) => t.validate(),
            TransportConfig::InsecureUdp(t) => t.validate(),
            TransportConfig::Udp(t) => t.validate(),
        }
    }
}