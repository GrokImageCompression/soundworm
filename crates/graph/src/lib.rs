pub mod mock;

use std::collections::HashMap;
use soundworm_core::{
    error::{Result, SoundwormError},
    event::BackendEvent,
    link::{Link, LinkId},
    node::{Node, NodeId},
    port::{Direction, Port, PortId},
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
        // also drop ports that belong to this node
        self.ports.retain(|_, p| &p.node_id != id);
        self.nodes.remove(id)
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn find_node_by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.values().find(|n| n.name == name)
    }

    pub fn ports_of(&self, node_id: &NodeId) -> Vec<&Port> {
        self.ports.values().filter(|p| &p.node_id == node_id).collect()
    }

    pub fn output_ports_of(&self, node_id: &NodeId) -> Vec<&Port> {
        self.ports.values()
            .filter(|p| &p.node_id == node_id && p.direction == Direction::Output)
            .collect()
    }

    pub fn input_ports_of(&self, node_id: &NodeId) -> Vec<&Port> {
        self.ports.values()
            .filter(|p| &p.node_id == node_id && p.direction == Direction::Input)
            .collect()
    }

    pub fn add_port(&mut self, port: Port) {
        self.ports.insert(port.id.clone(), port);
    }

    pub fn remove_port(&mut self, id: &PortId) -> Option<Port> {
        self.ports.remove(id)
    }

    pub fn get_port(&self, id: &PortId) -> Option<&Port> {
        self.ports.get(id)
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

    pub fn apply_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::NodeAppeared(node) => {
                self.nodes.entry(node.id.clone()).or_insert(node);
            }
            BackendEvent::NodeRemoved(id) => { self.remove_node(&id); }
            BackendEvent::PortAppeared(port) => {
                self.ports.entry(port.id.clone()).or_insert(port);
            }
            BackendEvent::PortRemoved(id) => { self.remove_port(&id); }
            BackendEvent::LinkAppeared(link) => {
                self.links.entry(link.id.clone()).or_insert(link);
            }
            BackendEvent::LinkRemoved(id) => { self.remove_link(&id); }
        }
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
            app_name: None, media_class: String::new(),
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
    fn test_find_node_by_name() {
        let mut g = AudioGraph::new();
        g.add_node(make_node(1, "spotify"));
        assert!(g.find_node_by_name("spotify").is_some());
        assert!(g.find_node_by_name("vlc").is_none());
    }

    #[test]
    fn test_ports_of() {
        let mut g = AudioGraph::new();
        g.add_port(make_port(1, 10, Direction::Output));
        g.add_port(make_port(2, 10, Direction::Input));
        g.add_port(make_port(3, 20, Direction::Output));
        assert_eq!(g.ports_of(&NodeId(10)).len(), 2);
        assert_eq!(g.output_ports_of(&NodeId(10)).len(), 1);
        assert_eq!(g.input_ports_of(&NodeId(10)).len(), 1);
    }

    #[test]
    fn test_remove_node_clears_ports() {
        let mut g = AudioGraph::new();
        g.add_node(make_node(1, "foo"));
        g.add_port(make_port(10, 1, Direction::Output));
        g.add_port(make_port(11, 1, Direction::Input));
        g.remove_node(&NodeId(1));
        assert_eq!(g.ports_of(&NodeId(1)).len(), 0);
    }

    #[test]
    fn test_apply_event_idempotent() {
        let mut g = AudioGraph::new();
        let node = make_node(1, "spotify");
        g.apply_event(BackendEvent::NodeAppeared(node.clone()));
        g.apply_event(BackendEvent::NodeAppeared(node));
        assert_eq!(g.nodes().count(), 1);
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
