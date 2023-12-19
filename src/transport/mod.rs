use async_trait::async_trait;

use crate::core::EmptyResult;

pub mod https;
pub mod udp;

#[async_trait]
pub trait Transport {
    fn name(&self) -> &str;
    fn is_ready(&self) -> bool;
    async fn send(&self, buf: &[u8]) -> EmptyResult;
}