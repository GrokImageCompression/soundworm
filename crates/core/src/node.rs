use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind { Source, Sink, Filter, Virtual }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id:          NodeId,
    pub name:        String,
    pub kind:        NodeKind,
    pub app_name:    Option<String>,
    pub media_class: String,
    pub sample_rate: u32,
    pub channels:    u8,
    pub latency_ms:  f32,
    pub properties:  HashMap<String, String>,
}

impl Node {
    pub fn new(id: u64, name: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id: NodeId(id),
            name: name.into(),
            kind,
            app_name: None,
            media_class: String::new(),
            sample_rate: 48000,
            channels: 2,
            latency_ms: 0.0,
            properties: HashMap::new(),
        }
    }
}
