use async_trait::async_trait;
use std::sync::mpsc;
use soundworm_core::{backend::AudioBackend, error::Result, event::BackendEvent, link::Link, node::Node};

pub struct CoreAudioBackend;

#[async_trait]
impl AudioBackend for CoreAudioBackend {
    fn name(&self) -> &str { "coreaudio" }
    fn subscribe(&self) -> mpsc::Receiver<BackendEvent> { mpsc::channel().1 }
    async fn enumerate_nodes(&self) -> Result<Vec<Node>>       { Ok(vec![]) }
    async fn create_link(&self, _l: &Link) -> Result<()>       { Ok(()) }
    async fn destroy_link(&self, _l: &Link) -> Result<()>      { Ok(()) }
    async fn set_volume(&self, _n: u64, _v: f32) -> Result<()> { Ok(()) }
}
