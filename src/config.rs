use std::fs::File;
use std::path::{Path, PathBuf};

use serde_derive::{Serialize, Deserialize};
use validator::{Validate, ValidationErrors};

use crate::core::GenericResult;

pub use crate::transport::https::HttpsServerTransportConfig;
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
    pub transports: Vec<TransportConfig>,
}

impl Config {
    pub fn load(path: &Path) -> GenericResult<Config> {
        let mut config: Config = serde_yaml::from_reader(File::open(path)?)?;
        config.path = path.to_owned();
        config.validate()?;
        Ok(config)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all="kebab-case")]
pub enum TransportConfig {
    HttpsServer(HttpsServerTransportConfig),
    Udp(UdpTransportConfig),
}

impl Validate for TransportConfig {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            TransportConfig::HttpsServer(t) => t.validate(),
            TransportConfig::Udp(t) => t.validate(),
        }
    }
}