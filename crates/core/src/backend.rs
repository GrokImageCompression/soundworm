use async_trait::async_trait;
use crate::{error::Result, link::Link, node::Node};

#[async_trait]
pub trait AudioBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn enumerate_nodes(&self) -> Result<Vec<Node>>;
    async fn create_link(&self, link: &Link) -> Result<()>;
    async fn destroy_link(&self, link: &Link) -> Result<()>;
    async fn set_volume(&self, node_id: u64, volume: f32) -> Result<()>;
}
