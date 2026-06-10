use serde::{Deserialize, Serialize};
use crate::port::PortId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct LinkId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub id:                      LinkId,
    pub source_port:             PortId,
    pub sink_port:               PortId,
    pub latency_compensation_ms: f32,
}
