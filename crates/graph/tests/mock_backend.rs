use soundworm_core::{
    event::BackendEvent,
    node::{Node, NodeId, NodeKind},
    port::{Direction, Port, PortId},
    link::{Link, LinkId},
    backend::AudioBackend,
};
use soundworm_graph::{AudioGraph, mock::MockBackend};
use std::collections::HashMap;

fn node(id: u64, name: &str, kind: NodeKind) -> Node {
    Node {
        id: NodeId(id), name: name.to_string(), kind,
        app_name: None, media_class: String::new(),
        sample_rate: 48000, channels: 2, latency_ms: 0.0,
        properties: HashMap::new(),
    }
}

fn port(id: u64, node_id: u64, dir: Direction) -> Port {
    Port { id: PortId(id), node_id: NodeId(node_id), name: format!("p{}", id), direction: dir, channels: 2 }
}

fn link(id: u64, src: u64, dst: u64) -> Link {
    Link { id: LinkId(id), source_port: PortId(src), sink_port: PortId(dst), latency_compensation_ms: 0.0 }
}

#[test]
fn graph_follows_mock_events() {
    let backend = MockBackend::new();
    let rx = backend.subscribe();
    let mut graph = AudioGraph::new();

    // Emit a scripted event sequence.
    backend.emit(BackendEvent::NodeAppeared(node(1, "spotify", NodeKind::Source)));
    backend.emit(BackendEvent::NodeAppeared(node(2, "speakers", NodeKind::Sink)));
    backend.emit(BackendEvent::PortAppeared(port(10, 1, Direction::Output)));
    backend.emit(BackendEvent::PortAppeared(port(20, 2, Direction::Input)));
    backend.emit(BackendEvent::LinkAppeared(link(100, 10, 20)));

    // Drain into graph.
    while let Ok(evt) = rx.try_recv() { graph.apply_event(evt); }

    assert!(graph.find_node_by_name("spotify").is_some());
    assert!(graph.find_node_by_name("speakers").is_some());
    assert_eq!(graph.output_ports_of(&NodeId(1)).len(), 1);
    assert_eq!(graph.input_ports_of(&NodeId(2)).len(), 1);
    assert_eq!(graph.links().count(), 1);

    // Remove the link and source node.
    backend.emit(BackendEvent::LinkRemoved(LinkId(100)));
    backend.emit(BackendEvent::NodeRemoved(NodeId(1)));
    while let Ok(evt) = rx.try_recv() { graph.apply_event(evt); }

    assert!(graph.find_node_by_name("spotify").is_none());
    assert_eq!(graph.links().count(), 0);
    // Port 10 (node 1) should have been cleaned up with the node.
    assert_eq!(graph.output_ports_of(&NodeId(1)).len(), 0);
}

#[test]
fn apply_event_is_idempotent() {
    let backend = MockBackend::new();
    let rx = backend.subscribe();
    let mut graph = AudioGraph::new();

    let n = node(5, "vlc", NodeKind::Source);
    backend.emit(BackendEvent::NodeAppeared(n.clone()));
    backend.emit(BackendEvent::NodeAppeared(n));
    while let Ok(evt) = rx.try_recv() { graph.apply_event(evt); }

    assert_eq!(graph.nodes().count(), 1);
}

#[tokio::test]
async fn mock_records_link_create() {
    let backend = MockBackend::new();
    let l = link(0, 1, 2);
    backend.create_link(&l).await.unwrap();
    backend.create_link(&l).await.unwrap();
    let calls = backend.link_calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].contains("create"));
}
