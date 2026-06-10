use std::collections::HashMap;
use soundworm_core::node::NodeId;

#[derive(Default)]
pub struct Metrics {
    latency_ms:  HashMap<NodeId, f32>,
    cpu_percent: HashMap<NodeId, f32>,
}

impl Metrics {
    pub fn update_latency(&mut self, id: NodeId, ms: f32)  { self.latency_ms.insert(id, ms); }
    pub fn update_cpu(&mut self, id: NodeId, cpu: f32)     { self.cpu_percent.insert(id, cpu); }
    pub fn report(&self) -> Vec<String> {
        self.latency_ms.iter().map(|(id, ms)| {
            format!("Node {:?}  latency={:.2}ms  cpu={:.1}%",
                id, ms, self.cpu_percent.get(id).copied().unwrap_or(0.0))
        }).collect()
    }
}
