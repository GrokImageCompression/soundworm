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
    pub sample_rate: u32,
    pub channels:    u8,
    pub latency_ms:  f32,
    pub properties:  HashMap<String, String>,
}
