use crate::{link::Link, node::Node, port::Port, link::LinkId, node::NodeId, port::PortId};

#[derive(Debug, Clone)]
pub enum BackendEvent {
    NodeAppeared(Node),
    NodeRemoved(NodeId),
    PortAppeared(Port),
    PortRemoved(PortId),
    LinkAppeared(Link),
    LinkRemoved(LinkId),
}
