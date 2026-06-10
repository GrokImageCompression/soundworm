use std::collections::HashMap;
use soundworm_core::{
    error::{Result, SoundwormError},
    link::{Link, LinkId},
    node::{Node, NodeId},
    port::{Port, PortId},
};

#[derive(Default)]
pub struct AudioGraph {
    nodes:        HashMap<NodeId, Node>,
    ports:        HashMap<PortId, Port>,
    links:        HashMap<LinkId, Link>,
    next_link_id: u64,
}

impl AudioGraph {
    pub fn new() -> Self { Self::default() }

    pub fn add_node(&mut self, node: Node) {
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn remove_node(&mut self, id: &NodeId) -> Option<Node> {
        self.nodes.remove(id)
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn add_port(&mut self, port: Port) {
        self.ports.insert(port.id.clone(), port);
    }

    pub fn add_link(&mut self, mut link: Link) -> Result<LinkId> {
        if !self.ports.contains_key(&link.source_port) {
            return Err(SoundwormError::PortNotFound(format!("{:?}", link.source_port)));
        }
        if !self.ports.contains_key(&link.sink_port) {
            return Err(SoundwormError::PortNotFound(format!("{:?}", link.sink_port)));
        }
        let id = LinkId(self.next_link_id);
        self.next_link_id += 1;
        link.id = id.clone();
        self.links.insert(id.clone(), link);
        Ok(id)
    }

    pub fn remove_link(&mut self, id: &LinkId) -> Option<Link> {
        self.links.remove(id)
    }

    pub fn links(&self) -> impl Iterator<Item = &Link> {
        self.links.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundworm_core::{
        node::{Node, NodeId, NodeKind},
        port::{Direction, Port, PortId},
        link::{Link, LinkId},
    };
    use std::collections::HashMap;

    fn make_node(id: u64, name: &str) -> Node {
        Node {
            id: NodeId(id), name: name.to_string(), kind: NodeKind::Source,
            sample_rate: 48000, channels: 2, latency_ms: 5.0,
            properties: HashMap::new(),
        }
    }

    fn make_port(id: u64, node_id: u64, dir: Direction) -> Port {
        Port {
            id: PortId(id), node_id: NodeId(node_id),
            name: format!("port_{}", id), direction: dir, channels: 2,
        }
    }

    #[test]
    fn test_add_remove_node() {
        let mut g = AudioGraph::new();
        g.add_node(make_node(1, "spotify"));
        assert!(g.get_node(&NodeId(1)).is_some());
        g.remove_node(&NodeId(1));
        assert!(g.get_node(&NodeId(1)).is_none());
    }

    #[test]
    fn test_link_requires_ports() {
        let mut g = AudioGraph::new();
        let link = Link { id: LinkId(0), source_port: PortId(99), sink_port: PortId(100), latency_compensation_ms: 0.0 };
        assert!(g.add_link(link).is_err());
    }

    #[test]
    fn test_link_succeeds_with_ports() {
        let mut g = AudioGraph::new();
        g.add_port(make_port(1, 10, Direction::Output));
        g.add_port(make_port(2, 20, Direction::Input));
        let link = Link { id: LinkId(0), source_port: PortId(1), sink_port: PortId(2), latency_compensation_ms: 0.0 };
        assert!(g.add_link(link).is_ok());
    }
}
