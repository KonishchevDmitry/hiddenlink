// See https://trojan-gfw.github.io/trojan/protocol.html for details

use std::net::SocketAddr;

use serde::{Serialize, Deserialize};
use sha2::{Sha224, Digest};
use validator::Validate;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct TrojanConfig {
    address: SocketAddr,
    #[validate(length(min = 1))]
    passwords: Vec<String>,
}

pub fn get_header(password: &str) -> String {
    hex::encode(Sha224::digest(password))
}