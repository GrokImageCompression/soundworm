//! Shared daemon state: the live `AudioGraph` plus the backend handle.
//! Cloned (via `Arc`) into every IPC connection task.

use anyhow::Result;
use soundworm_core::{
    backend::AudioBackend,
    event::BackendEvent,
    link::{Link, LinkId},
    port::{Direction, PortId},
};
use soundworm_graph::AudioGraph;
use soundworm_ipc::{ErrorCode, Event as IpcEvent, PortRef, ProtoError};
use soundworm_observability::{Metrics, XrunLog};
use soundworm_policy::{rules::RulesEngine, session::SessionSnapshot};
use soundworm_rhai::{Decision, ScriptEngine};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Notify};

pub const EVENT_CHANNEL_CAP: usize = 1024;

pub struct DaemonState {
    pub graph: Mutex<AudioGraph>,
    pub backend: Arc<dyn AudioBackend>,
    pub events: broadcast::Sender<IpcEvent>,
    pub rules: Mutex<RulesEngine>,
    /// Last successfully loaded rules path; `ReloadRules` re-reads from here.
    pub rules_path: Mutex<Option<PathBuf>>,
    /// Optional Rhai script: consulted when TOML rules don't match.
    pub script: Mutex<Option<ScriptEngine>>,
    pub metrics: Mutex<Metrics>,
    pub xruns: Mutex<XrunLog>,
    pub shutdown: Notify,
}

impl DaemonState {
    pub fn new(backend: Arc<dyn AudioBackend>) -> Self {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        Self {
            graph: Mutex::new(AudioGraph::new()),
            backend,
            events: tx,
            rules: Mutex::new(RulesEngine::default()),
            rules_path: Mutex::new(None),
            script: Mutex::new(None),
            metrics: Mutex::new(Metrics::default()),
            xruns: Mutex::new(XrunLog::default()),
            shutdown: Notify::new(),
        }
    }

    /// Replace the rules engine from a TOML file on disk. Records the path
    /// so a later `ReloadRules` can re-read it.
    pub fn load_rules_from(&self, path: PathBuf) -> Result<usize> {
        let content = std::fs::read_to_string(&path)?;
        let mut engine = RulesEngine::default();
        engine.load_toml(&content)?;
        let count = engine.rule_count();
        *self.rules.lock().unwrap() = engine;
        *self.rules_path.lock().unwrap() = Some(path);
        Ok(count)
    }

    /// Compile a Rhai script from disk and install it atomically.
    /// On parse error the previously loaded script (if any) is retained.
    pub fn load_script_from(&self, path: PathBuf) -> Result<()> {
        let engine = ScriptEngine::load_from_path(path)?;
        *self.script.lock().unwrap() = Some(engine);
        Ok(())
    }

    /// Re-read the script from its previously loaded path. Returns
    /// `Ok(false)` if no script was loaded; `Ok(true)` on success.
    pub fn reload_script(&self) -> Result<bool> {
        let path = {
            let guard = self.script.lock().unwrap();
            match guard.as_ref().and_then(|s| s.source_path()) {
                Some(p) => p.to_path_buf(),
                None => return Ok(false),
            }
        };
        let fresh = ScriptEngine::load_from_path(path)?;
        *self.script.lock().unwrap() = Some(fresh);
        Ok(true)
    }

    /// Re-read rules from the previously loaded path.
    pub fn reload_rules(&self) -> Result<usize> {
        let path = self
            .rules_path
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no rules file loaded yet"))?;
        self.load_rules_from(path)
    }

    /// Build a snapshot of the current graph's links under `name`.
    pub fn build_snapshot(&self, name: &str) -> SessionSnapshot {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let links: Vec<(u64, u64)> = self
            .graph
            .lock()
            .unwrap()
            .links()
            .map(|l| (l.source_port.0, l.sink_port.0))
            .collect();
        SessionSnapshot {
            name: name.to_owned(),
            timestamp,
            links,
            volumes: std::collections::HashMap::new(),
        }
    }

    /// Apply each link in the snapshot via the backend. Returns
    /// `(applied, skipped)`.
    pub async fn apply_snapshot(&self, snap: &SessionSnapshot) -> (usize, usize) {
        let mut applied = 0usize;
        let mut skipped = 0usize;
        for (src, sink) in &snap.links {
            let link = Link {
                id: LinkId(0),
                source_port: PortId(*src),
                sink_port: PortId(*sink),
                latency_compensation_ms: 0.0,
            };
            if self.backend.create_link(&link).await.is_ok() {
                applied += 1;
            } else {
                skipped += 1;
            }
        }
        (applied, skipped)
    }

    /// Spawn the std::mpsc drain on a blocking thread; events flow into
    /// the shared graph AND fan out to IPC subscribers via broadcast.
    /// After applying each event the policy chain runs against the live
    /// graph, queuing pending routes for nodes whose ports haven't shown
    /// up yet.
    pub fn start_event_pump(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let rx = this.backend.subscribe();
        // Capture the tokio handle so the (non-tokio) pump thread can
        // spawn async `backend.create_link` calls without blocking.
        let handle = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("swd-event-pump".into())
            .spawn(move || {
                let mut pending: std::collections::HashMap<
                    soundworm_core::node::NodeId,
                    PendingRoute,
                > = std::collections::HashMap::new();

                while let Ok(event) = rx.recv() {
                    let ipc_event = to_ipc_event(&event);
                    let evaluated = matches!(
                        event,
                        BackendEvent::NodeAppeared(_) | BackendEvent::PortAppeared(_)
                    );

                    // Observability side-effects: record into shared
                    // stores *before* the graph mutation so a panic in
                    // graph code doesn't lose the measurement.
                    match &event {
                        BackendEvent::Xrun { node_id, gap_ms } => {
                            if let Ok(mut x) = this.xruns.lock() {
                                x.record(node_id.clone(), *gap_ms);
                            }
                        }
                        BackendEvent::LatencySample { node_id, latency_ms } => {
                            if let Ok(mut m) = this.metrics.lock() {
                                m.record_latency_ms(node_id.clone(), *latency_ms);
                            }
                        }
                        _ => {}
                    }

                    if let BackendEvent::NodeAppeared(ref node) = event {
                        if let Some(action) = evaluate_node(&this, node) {
                            handle_action(&this, node.id.clone(), action, &mut pending);
                        } else if let Some((rule_name, action)) = evaluate_script(&this, node) {
                            handle_action(&this, node.id.clone(), (rule_name, action), &mut pending);
                        }
                    }

                    if let Ok(mut g) = this.graph.lock() {
                        g.apply_event(event);
                    }
                    if let Some(ev) = ipc_event {
                        let _ = this.events.send(ev);
                    }
                    if evaluated {
                        try_fire_pending(&this, &handle, &mut pending);
                    }
                }
            })
            .expect("spawn event pump thread");
    }
}

#[derive(Clone)]
struct PendingRoute {
    rule_name: String,
    target_name: String,
}

/// Snapshot the TOML rule outcome for `node`. Returns an owned `Action`
/// so the graph lock can be taken without holding the rules lock.
fn evaluate_node(state: &DaemonState, node: &soundworm_core::node::Node) -> Option<(String, soundworm_policy::rules::Action)> {
    let rules = state.rules.lock().ok()?;
    let r = rules.evaluate_node(node)?;
    Some((r.name.clone(), r.action.clone()))
}

/// Consult the Rhai script. Returns an owned action with a synthetic
/// rule name (`"rhai"`) so downstream handling is unchanged.
fn evaluate_script(state: &DaemonState, node: &soundworm_core::node::Node) -> Option<(String, soundworm_policy::rules::Action)> {
    use soundworm_policy::rules::Action;
    let sinks = {
        let g = state.graph.lock().ok()?;
        g.nodes()
            .filter(|n| matches!(n.kind, soundworm_core::node::NodeKind::Sink))
            .map(|n| n.name.clone())
            .collect::<Vec<_>>()
    };
    let decision = {
        let guard = state.script.lock().ok()?;
        guard.as_ref()?.evaluate(node, &sinks)
    };
    match decision {
        Decision::Route(target) => Some(("rhai".into(), Action::Route { target })),
        Decision::Deny => Some(("rhai".into(), Action::Deny)),
        Decision::Allow | Decision::None => None,
    }
}

fn handle_action(
    state: &DaemonState,
    node_id: soundworm_core::node::NodeId,
    (rule_name, action): (String, soundworm_policy::rules::Action),
    pending: &mut std::collections::HashMap<soundworm_core::node::NodeId, PendingRoute>,
) {
    use soundworm_policy::rules::Action;
    match action {
        Action::Route { target } => {
            tracing::info!(
                "rule '{}' matched node {:?} → queue route to '{}'",
                rule_name, node_id, target
            );
            pending.insert(node_id, PendingRoute { rule_name, target_name: target });
        }
        Action::Deny => {
            let _ = state.events.send(IpcEvent::LinkRejected {
                reason: format!("rule '{rule_name}' denied node"),
            });
        }
        Action::SetVolume { volume } => {
            tracing::info!("rule '{}' set_volume {} (not yet implemented)", rule_name, volume);
        }
        Action::Notify { message } => {
            tracing::info!("rule '{}' notify: {}", rule_name, message);
        }
    }
}

fn try_fire_pending(
    state: &Arc<DaemonState>,
    handle: &tokio::runtime::Handle,
    pending: &mut std::collections::HashMap<soundworm_core::node::NodeId, PendingRoute>,
) {
    let mut fired: Vec<soundworm_core::node::NodeId> = Vec::new();
    let resolved: Vec<(soundworm_core::node::NodeId, PendingRoute, PortId, PortId)> = {
        let Ok(g) = state.graph.lock() else { return };
        pending
            .iter()
            .filter_map(|(src_id, route)| {
                let src_out = g.output_ports_of(src_id).into_iter().next()?.id.clone();
                let target = g.find_node_by_name(&route.target_name)?;
                let tgt_in = g.input_ports_of(&target.id).into_iter().next()?.id.clone();
                Some((src_id.clone(), route.clone(), src_out, tgt_in))
            })
            .collect()
    };

    for (src_id, route, src_port, sink_port) in resolved {
        fired.push(src_id);
        let state = Arc::clone(state);
        let route_clone = route.clone();
        handle.spawn(async move {
            let link = Link {
                id: LinkId(0),
                source_port: src_port,
                sink_port,
                latency_compensation_ms: 0.0,
            };
            match state.backend.create_link(&link).await {
                Ok(_) => {
                    let _ = state.events.send(IpcEvent::RulesApplied {
                        rule: route_clone.rule_name,
                        link_id: LinkId(0),
                    });
                }
                Err(e) => {
                    let _ = state.events.send(IpcEvent::LinkRejected {
                        reason: format!(
                            "rule '{}' link to '{}' failed: {e}",
                            route_clone.rule_name, route_clone.target_name
                        ),
                    });
                }
            }
        });
    }

    for id in fired {
        pending.remove(&id);
    }
}

fn to_ipc_event(e: &BackendEvent) -> Option<IpcEvent> {
    match e {
        BackendEvent::NodeAppeared(n) => Some(IpcEvent::NodeAppeared { node: n.clone() }),
        BackendEvent::NodeRemoved(id) => Some(IpcEvent::NodeRemoved { node_id: id.clone() }),
        BackendEvent::LinkAppeared(l) => Some(IpcEvent::LinkAppeared { link: l.clone() }),
        BackendEvent::LinkRemoved(id) => Some(IpcEvent::LinkRemoved { link_id: id.clone() }),
        BackendEvent::Xrun { node_id, gap_ms } => Some(IpcEvent::XrunObserved {
            node_id: node_id.clone(),
            gap_ms: *gap_ms,
        }),
        // Port-level + latency-sample events update internal state but
        // aren't surfaced on the IPC stream — clients re-fetch via
        // ListNodes / GetMetrics when needed.
        BackendEvent::PortAppeared(_)
        | BackendEvent::PortRemoved(_)
        | BackendEvent::LatencySample { .. } => None,
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
        IpcEvent::XrunObserved { .. } => "XrunObserved",
    }
}

/// Resolve a `PortRef` to a concrete `PortId` against the live graph.
/// `dir` is the direction we expect — for `Link.source` that's Output,
/// for `Link.sink` that's Input. Named refs pick the first matching port.
///
/// `require_kind` constrains which media kind the owning node must
/// have (Audio/Midi/Video/Other). PipeWire's link factory silently
/// accepts then destroys cross-kind links, so we'd rather refuse them
/// up front than fail invisibly later.
pub fn resolve_port(
    graph: &AudioGraph,
    r: &PortRef,
    dir: Direction,
    require_kind: Option<soundworm_core::node::MediaKind>,
) -> Result<soundworm_core::port::PortId, ProtoError> {
    match r {
        PortRef::Id(id) => {
            let p = graph
                .get_port(id)
                .ok_or_else(|| err(ErrorCode::NotFound, &format!("port {} unknown", id.0)))?;
            if let Some(want) = require_kind {
                let owner = graph.get_node(&p.node_id).ok_or_else(|| {
                    err(ErrorCode::NotFound, &format!("owner node for port {} unknown", id.0))
                })?;
                if owner.media_kind() != want {
                    return Err(err(
                        ErrorCode::BadRequest,
                        &format!(
                            "port {} is {:?}, sink expects {:?}",
                            id.0, owner.media_kind(), want,
                        ),
                    ));
                }
            }
            Ok(id.clone())
        }
        PortRef::Named { node, port: _ } => {
            let n = graph
                .find_node_by_name(node)
                .ok_or_else(|| err(ErrorCode::NotFound, &format!("node '{node}' not found")))?;
            if let Some(want) = require_kind {
                if n.media_kind() != want {
                    return Err(err(
                        ErrorCode::BadRequest,
                        &format!(
                            "node '{node}' is {:?}, peer expects {:?}",
                            n.media_kind(), want,
                        ),
                    ));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use soundworm_core::node::{NodeId, NodeKind};
    use soundworm_core::port::{Direction, Port};
    use soundworm_graph::mock::MockBackend;

    const RULE_SPOTIFY_TO_SPEAKERS: &str = r#"
[[rules]]
name     = "spotify-to-speakers"
priority = 10
[rules.matches]
node_name = "spotify"
[rules.action]
Route = { target = "speakers" }
"#;

    fn node(id: u64, name: &str) -> soundworm_core::node::Node {
        soundworm_core::node::Node {
            id: NodeId(id),
            name: name.into(),
            kind: NodeKind::Source,
            app_name: None,
            media_class: String::new(),
            sample_rate: 48000,
            channels: 2,
            latency_ms: 0.0,
            properties: Default::default(),
        }
    }

    fn port(id: u64, node_id: u64, dir: Direction) -> Port {
        Port {
            id: PortId(id),
            node_id: NodeId(node_id),
            name: format!("port_{id}"),
            direction: dir,
            channels: 2,
        }
    }

    #[tokio::test]
    async fn auto_route_fires_after_target_ports_appear() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(DaemonState::new(mock.clone() as Arc<dyn AudioBackend>));
        {
            let mut e = RulesEngine::default();
            e.load_toml(RULE_SPOTIFY_TO_SPEAKERS).unwrap();
            *state.rules.lock().unwrap() = e;
        }
        state.start_event_pump();

        // spotify (source) appears first, then its port — target not yet there.
        mock.emit(BackendEvent::NodeAppeared(node(1, "spotify")));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert!(calls.lock().unwrap().is_empty(), "must not fire before target exists");

        // Target appears with input port — pump fires the link.
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers")));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "expected exactly one create_link call, got {calls:?}");
        assert_eq!(calls[0], "create 100→200");
    }

    #[tokio::test]
    async fn rhai_script_routes_when_toml_misses() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(DaemonState::new(mock.clone() as Arc<dyn AudioBackend>));
        // No TOML rules — only a Rhai script.
        let script = soundworm_rhai::ScriptEngine::load_str(
            r#"if node.name == "spotify" { route("speakers") } else { deny() }"#,
        )
        .unwrap();
        *state.script.lock().unwrap() = Some(script);
        state.start_event_pump();

        mock.emit(BackendEvent::NodeAppeared(node(1, "spotify")));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers")));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "rhai should have routed spotify → speakers, got {calls:?}");
        assert_eq!(calls[0], "create 100→200");
    }

    #[tokio::test]
    async fn xrun_event_is_recorded_and_broadcast() {
        let mock = Arc::new(MockBackend::new());
        let state = Arc::new(DaemonState::new(mock.clone() as Arc<dyn AudioBackend>));
        let mut rx = state.events.subscribe();
        state.start_event_pump();

        mock.emit(BackendEvent::Xrun { node_id: NodeId(7), gap_ms: 3.5 });
        mock.emit(BackendEvent::LatencySample { node_id: NodeId(7), latency_ms: 12.0 });

        let ev = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("xrun event")
            .expect("recv");
        assert!(matches!(ev, IpcEvent::XrunObserved { gap_ms, .. } if (gap_ms - 3.5).abs() < 0.01));

        // Allow the second event to drain into metrics.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(state.xruns.lock().unwrap().total(), 1);
        let snap = state.metrics.lock().unwrap().snapshot();
        assert_eq!(snap.nodes.len(), 1);
        assert!(snap.nodes[0].count >= 1);
    }

    #[tokio::test]
    async fn no_rule_no_route() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(DaemonState::new(mock.clone() as Arc<dyn AudioBackend>));
        state.start_event_pump();

        mock.emit(BackendEvent::NodeAppeared(node(1, "vlc")));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers")));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(calls.lock().unwrap().is_empty());
    }
}
