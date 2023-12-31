use std::fs::File;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde_derive::{Serialize, Deserialize};
use validator::{Validate, ValidationErrors};

use crate::core::GenericResult;

use crate::transport::default_transport_weight;
pub use crate::transport::http::{HttpClientTransportConfig, HttpServerTransportConfig};
pub use crate::transport::udp::UdpTransportConfig;

#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(skip)]
    pub path: PathBuf,

    #[validate(length(min = 1))]
    pub name: String,

    #[validate]
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

    #[validate(range(min = 1))]
    #[serde(default="default_transport_weight")]
    pub weight: u16,

    #[validate]
    #[serde(flatten)]
    pub transport: TransportConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all="kebab-case")]
pub enum TransportConfig {
    HttpClient(HttpClientTransportConfig),
    HttpServer(HttpServerTransportConfig),
    Udp(UdpTransportConfig),
}

impl Validate for TransportConfig {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            TransportConfig::HttpClient(t) => t.validate(),
            TransportConfig::HttpServer(t) => t.validate(),
            TransportConfig::Udp(t) => t.validate(),
        }
    }
}