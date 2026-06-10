use serde::{Deserialize, Serialize};
use crate::node::NodeId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PortId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Direction { Input, Output }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Port {
    pub id:        PortId,
    pub node_id:   NodeId,
    pub name:      String,
    pub direction: Direction,
    pub channels:  u8,
}
