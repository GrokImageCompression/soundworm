use async_trait::async_trait;
use std::sync::{mpsc, Arc, Mutex};
use soundworm_core::{
    backend::AudioBackend,
    error::Result,
    event::BackendEvent,
    link::Link,
    node::Node,
};

/// Injects scripted events into subscribers; records link create/destroy calls.
pub struct MockBackend {
    sinks:          Arc<Mutex<Vec<mpsc::SyncSender<BackendEvent>>>>,
    pub link_calls: Arc<Mutex<Vec<String>>>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            sinks: Arc::new(Mutex::new(Vec::new())),
            link_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn emit(&self, event: BackendEvent) {
        let mut guard = self.sinks.lock().unwrap();
        guard.retain(|tx| tx.try_send(event.clone()).is_ok());
    }
}

impl Default for MockBackend { fn default() -> Self { Self::new() } }

#[async_trait]
impl AudioBackend for MockBackend {
    fn name(&self) -> &str { "mock" }

    fn subscribe(&self) -> mpsc::Receiver<BackendEvent> {
        let (tx, rx) = mpsc::sync_channel(256);
        self.sinks.lock().unwrap().push(tx);
        rx
    }

    async fn enumerate_nodes(&self) -> Result<Vec<Node>> { Ok(vec![]) }

    async fn create_link(&self, link: &Link) -> Result<()> {
        self.link_calls.lock().unwrap()
            .push(format!("create {}→{}", link.source_port.0, link.sink_port.0));
        Ok(())
    }

    async fn destroy_link(&self, link: &Link) -> Result<()> {
        self.link_calls.lock().unwrap()
            .push(format!("destroy {}", link.id.0));
        Ok(())
    }

    async fn set_volume(&self, node_id: u64, volume: f32) -> Result<()> {
        self.link_calls.lock().unwrap()
            .push(format!("vol {} {:.2}", node_id, volume));
        Ok(())
    }
}
