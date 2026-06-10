//! Shared daemon state: the live `AudioGraph` plus the backend handle.
//! Cloned (via `Arc`) into every IPC connection task.

use anyhow::Result;
use soundworm_core::{backend::AudioBackend, event::BackendEvent, port::Direction};
use soundworm_graph::AudioGraph;
use soundworm_ipc::{ErrorCode, Event as IpcEvent, PortRef, ProtoError};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub const EVENT_CHANNEL_CAP: usize = 1024;

pub struct DaemonState {
    pub graph: Mutex<AudioGraph>,
    pub backend: Arc<dyn AudioBackend>,
    pub events: broadcast::Sender<IpcEvent>,
}

impl DaemonState {
    pub fn new(backend: Arc<dyn AudioBackend>) -> Self {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        Self { graph: Mutex::new(AudioGraph::new()), backend, events: tx }
    }

    /// Spawn the std::mpsc drain on a blocking thread; events flow into
    /// the shared graph AND fan out to IPC subscribers via broadcast.
    pub fn start_event_pump(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let rx = this.backend.subscribe();
        std::thread::Builder::new()
            .name("swd-event-pump".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    let ipc_event = to_ipc_event(&event);
                    if let Ok(mut g) = this.graph.lock() {
                        g.apply_event(event);
                    }
                    if let Some(ev) = ipc_event {
                        // err only when no receivers — fine, ignore.
                        let _ = this.events.send(ev);
                    }
                }
            })
            .expect("spawn event pump thread");
    }
}

fn to_ipc_event(e: &BackendEvent) -> Option<IpcEvent> {
    match e {
        BackendEvent::NodeAppeared(n) => Some(IpcEvent::NodeAppeared { node: n.clone() }),
        BackendEvent::NodeRemoved(id) => Some(IpcEvent::NodeRemoved { node_id: id.clone() }),
        BackendEvent::LinkAppeared(l) => Some(IpcEvent::LinkAppeared { link: l.clone() }),
        BackendEvent::LinkRemoved(id) => Some(IpcEvent::LinkRemoved { link_id: id.clone() }),
        // Port-level events update the graph but aren't surfaced on the
        // IPC stream — clients re-fetch ports via ListNodes when needed.
        BackendEvent::PortAppeared(_) | BackendEvent::PortRemoved(_) => None,
    }
}

pub fn event_kind(e: &IpcEvent) -> &'static str {
    match e {
        IpcEvent::NodeAppeared { .. } => "NodeAppeared",
        IpcEvent::NodeRemoved { .. } => "NodeRemoved",
        IpcEvent::LinkAppeared { .. } => "LinkAppeared",
        IpcEvent::LinkRemoved { .. } => "LinkRemoved",
        IpcEvent::RulesApplied { .. } => "RulesApplied",
        IpcEvent::LinkRejected { .. } => "LinkRejected",
        IpcEvent::EventsDropped { .. } => "EventsDropped",
    }
}

/// Resolve a `PortRef` to a concrete `PortId` against the live graph.
/// `dir` is the direction we expect — for `Link.source` that's Output,
/// for `Link.sink` that's Input. Named refs pick the first matching port.
pub fn resolve_port(
    graph: &AudioGraph,
    r: &PortRef,
    dir: Direction,
) -> Result<soundworm_core::port::PortId, ProtoError> {
    match r {
        PortRef::Id(id) => {
            if graph.get_port(id).is_some() {
                Ok(id.clone())
            } else {
                Err(err(ErrorCode::NotFound, &format!("port {} unknown", id.0)))
            }
        }
        PortRef::Named { node, port: _ } => {
            let n = graph
                .find_node_by_name(node)
                .ok_or_else(|| err(ErrorCode::NotFound, &format!("node '{node}' not found")))?;
            let ports = match dir {
                Direction::Output => graph.output_ports_of(&n.id),
                Direction::Input => graph.input_ports_of(&n.id),
            };
            ports
                .into_iter()
                .next()
                .map(|p| p.id.clone())
                .ok_or_else(|| err(ErrorCode::NotFound, &format!("no {dir:?} port on '{node}'")))
        }
    }
}

fn err(code: ErrorCode, message: &str) -> ProtoError {
    ProtoError { code, message: message.into() }
}
