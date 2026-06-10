use async_trait::async_trait;
use soundworm_core::{backend::AudioBackend, error::Result, link::Link, node::Node};

pub struct WasapiBackend;

#[async_trait]
impl AudioBackend for WasapiBackend {
    fn name(&self) -> &str { "wasapi" }
    async fn enumerate_nodes(&self) -> Result<Vec<Node>>       { Ok(vec![]) }
    async fn create_link(&self, _l: &Link) -> Result<()>       { Ok(()) }
    async fn destroy_link(&self, _l: &Link) -> Result<()>      { Ok(()) }
    async fn set_volume(&self, _n: u64, _v: f32) -> Result<()> { Ok(()) }
}
