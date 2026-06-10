use async_trait::async_trait;
use soundworm_core::{backend::AudioBackend, error::Result, link::Link, node::Node};

pub struct PipeWireBackend;

impl PipeWireBackend {
    pub fn new() -> Self {
        tracing::info!("PipeWire backend initialised (stub)");
        Self
    }
}

impl Default for PipeWireBackend { fn default() -> Self { Self::new() } }

#[async_trait]
impl AudioBackend for PipeWireBackend {
    fn name(&self) -> &str { "pipewire" }
    async fn enumerate_nodes(&self) -> Result<Vec<Node>>       { Ok(vec![]) }
    async fn create_link(&self, link: &Link) -> Result<()>     {
        tracing::info!("PipeWire: create_link {:?}→{:?}", link.source_port, link.sink_port);
        Ok(())
    }
    async fn destroy_link(&self, link: &Link) -> Result<()>    {
        tracing::info!("PipeWire: destroy_link {:?}", link.id);
        Ok(())
    }
    async fn set_volume(&self, node_id: u64, volume: f32) -> Result<()> {
        tracing::info!("PipeWire: set_volume node={} vol={:.2}", node_id, volume);
        Ok(())
    }
}
