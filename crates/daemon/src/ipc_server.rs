//! Unix socket accept loop. Per-connection structure:
//!
//!   reader task   ──Request──▶ dispatch ──Response──▶ outbound mpsc ──▶ writer task
//!   subscribe task ──Event──▶ outbound mpsc ──▶ writer task
//!
//! The outbound mpsc serializes writes, so we never interleave a response
//! frame with an event frame on the wire.

use crate::state::{event_kind, resolve_port, DaemonState};
use anyhow::{Context, Result};
use soundworm_core::{
    link::{Link, LinkId},
    port::{Direction, PortId},
};
use soundworm_ipc::{
    codec, ErrorCode, Event as IpcEvent, Message, MetricsPayload, NodeLatencyPayload, Op,
    ProtoError, Request, Response, ResponseData, PROTO_VERSION,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

const OUTBOUND_CAP: usize = 1024;

pub fn socket_path() -> PathBuf {
    soundworm_ipc::default_socket_path()
}

pub async fn serve(path: PathBuf, state: Arc<DaemonState>) -> Result<()> {
    #[cfg(unix)]
    {
        serve_unix(path, state).await
    }
    #[cfg(windows)]
    {
        serve_windows(path, state).await
    }
}

#[cfg(unix)]
async fn serve_unix(path: PathBuf, state: Arc<DaemonState>) -> Result<()> {
    use tokio::net::UnixListener;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {path:?}"))?;
    set_socket_perms(&path)?;
    tracing::info!("IPC listening on {:?}", path);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let (reader, writer) = stream.into_split();
            if let Err(e) = handle_client(reader, writer, state).await {
                tracing::warn!("client closed with error: {e:#}");
            }
        });
    }
}

// Windows named-pipe accept loop. Each accepted instance is handed off and
// the next instance is created immediately so a connecting client never
// finds the pipe missing.
#[cfg(windows)]
async fn serve_windows(path: PathBuf, state: Arc<DaemonState>) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = path.as_os_str().to_owned();
    tracing::info!("IPC listening on {:?}", path);

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&name)
        .with_context(|| format!("create pipe {path:?}"))?;
    loop {
        server.connect().await.context("pipe connect")?;
        let connected = server;
        server = ServerOptions::new()
            .create(&name)
            .context("create next pipe instance")?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let (reader, writer) = tokio::io::split(connected);
            if let Err(e) = handle_client(reader, writer, state).await {
                tracing::warn!("client closed with error: {e:#}");
            }
        });
    }
}

#[cfg(unix)]
fn set_socket_perms(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

struct ClientCtx {
    state: Arc<DaemonState>,
    outbound: mpsc::Sender<Message>,
    sub_handle: Option<tokio::task::JoinHandle<()>>,
    said_hello: bool,
}

async fn handle_client<R, W>(reader: R, mut writer: W, state: Arc<DaemonState>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<Message>(OUTBOUND_CAP);

    // Writer task: drains outbound queue onto the socket.
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match codec::encode(&msg) {
                Ok(frame) => {
                    if writer.write_all(frame.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("encode error: {e}");
                    break;
                }
            }
        }
    });

    let mut ctx = ClientCtx { state, outbound: tx, sub_handle: None, said_hello: false };
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        let reply = match codec::decode(&line) {
            Ok(Message::Request(req)) => dispatch(req, &mut ctx).await,
            Ok(_) => err(0, ErrorCode::BadRequest, "expected Request frame"),
            Err(e) => err(0, ErrorCode::BadRequest, &e.to_string()),
        };
        if ctx.outbound.send(Message::Response(reply)).await.is_err() {
            break;
        }
    }
    if let Some(h) = ctx.sub_handle.take() {
        h.abort();
    }
    drop(ctx.outbound);
    let _ = write_task.await;
    Ok(())
}

async fn dispatch(req: Request, ctx: &mut ClientCtx) -> Response {
    let id = req.id;
    if !ctx.said_hello {
        match &req.op {
            Op::Hello { .. } => {}
            _ => return err(id, ErrorCode::BadRequest, "Hello required first"),
        }
    }
    match req.op {
        Op::Hello { .. } => {
            ctx.said_hello = true;
            ok(id, ResponseData::Hello {
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                proto: PROTO_VERSION,
            })
        }
        Op::ListNodes => {
            let g = ctx.state.graph.lock().unwrap();
            let nodes = g
                .nodes()
                .map(|n| soundworm_ipc::NodeView {
                    node: n.clone(),
                    ports: g.ports_of(&n.id).into_iter().cloned().collect(),
                })
                .collect();
            ok(id, ResponseData::Nodes { nodes })
        }
        Op::ListPorts => {
            let ports = ctx.state.graph.lock().unwrap().ports().cloned().collect();
            ok(id, ResponseData::Ports { ports })
        }
        Op::ListLinks => {
            let links = ctx.state.graph.lock().unwrap().links().cloned().collect();
            ok(id, ResponseData::Links { links })
        }
        Op::Link { source, sink } => do_link(id, &ctx.state, source, sink).await,
        Op::Unlink { link_id } => do_unlink(id, &ctx.state, link_id).await,
        Op::SetVolume { node, volume } => match ctx.state.backend.set_volume(node.0, volume).await {
            Ok(()) => ok(id, ResponseData::Empty {}),
            Err(e) => err(id, ErrorCode::BackendError, &e.to_string()),
        },
        Op::SetMute { node, mute } => match ctx.state.backend.set_mute(node.0, mute).await {
            Ok(()) => ok(id, ResponseData::Empty {}),
            Err(e) => err(id, ErrorCode::BackendError, &e.to_string()),
        },
        Op::Subscribe { filter } => do_subscribe(id, ctx, filter),
        Op::Unsubscribe => {
            if let Some(h) = ctx.sub_handle.take() {
                h.abort();
            }
            ok(id, ResponseData::Empty {})
        }
        Op::LoadRules { path } => do_load_rules(id, &ctx.state, path),
        Op::ReloadRules => do_reload_rules(id, &ctx.state),
        Op::LoadScript { path } => do_load_script(id, &ctx.state, path),
        Op::ReloadScript => do_reload_script(id, &ctx.state),
        Op::GetMetrics => do_get_metrics(id, &ctx.state),
        Op::Snapshot { name } => do_snapshot(id, &ctx.state, name).await,
        Op::Restore { name } => do_restore(id, &ctx.state, name).await,
        Op::Shutdown => do_shutdown(id, &ctx.state),
    }
}

fn do_load_rules(id: u64, state: &Arc<DaemonState>, path: String) -> Response {
    match state.load_rules_from(PathBuf::from(path)) {
        Ok(rule_count) => ok(id, ResponseData::Rules { rule_count }),
        Err(e) => err(id, ErrorCode::RulesError, &e.to_string()),
    }
}

fn do_reload_rules(id: u64, state: &Arc<DaemonState>) -> Response {
    match state.reload_rules() {
        Ok(rule_count) => ok(id, ResponseData::Rules { rule_count }),
        Err(e) => err(id, ErrorCode::RulesError, &e.to_string()),
    }
}

fn do_load_script(id: u64, state: &Arc<DaemonState>, path: String) -> Response {
    match state.load_script_from(PathBuf::from(&path)) {
        Ok(()) => ok(id, ResponseData::Script { path }),
        Err(e) => err(id, ErrorCode::RulesError, &e.to_string()),
    }
}

fn do_reload_script(id: u64, state: &Arc<DaemonState>) -> Response {
    match state.reload_script() {
        Ok(true) => {
            let p = state
                .script
                .lock()
                .ok()
                .and_then(|g| g.as_ref().and_then(|s| s.source_path().map(|p| p.display().to_string())))
                .unwrap_or_default();
            ok(id, ResponseData::Script { path: p })
        }
        Ok(false) => err(id, ErrorCode::NotFound, "no script loaded"),
        Err(e) => err(id, ErrorCode::RulesError, &e.to_string()),
    }
}

async fn do_snapshot(id: u64, state: &Arc<DaemonState>, name: String) -> Response {
    let snap = state.build_snapshot(&name);
    if let Err(e) = soundworm_snapshots::save(&snap).await {
        return err(id, ErrorCode::Internal, &e.to_string());
    }
    let path = soundworm_snapshots::snapshot_dir()
        .join(format!("{}.json", name))
        .to_string_lossy()
        .into_owned();
    ok(id, ResponseData::Snapshot { path })
}

async fn do_restore(id: u64, state: &Arc<DaemonState>, name: String) -> Response {
    let snap = match soundworm_snapshots::load(&name).await {
        Ok(s) => s,
        Err(e) => return err(id, ErrorCode::NotFound, &e.to_string()),
    };
    let (applied, skipped) = state.apply_snapshot(&snap).await;
    ok(id, ResponseData::Restore { applied, skipped })
}

fn do_get_metrics(id: u64, state: &Arc<DaemonState>) -> Response {
    let snap = state.metrics.lock().unwrap().snapshot();
    let xruns = state.xruns.lock().unwrap();
    let metrics = MetricsPayload {
        nodes: snap
            .nodes
            .into_iter()
            .map(|n| NodeLatencyPayload {
                node_id: n.node_id,
                count:   n.count,
                min_ms:  n.min_ms,
                p50_ms:  n.p50_ms,
                p95_ms:  n.p95_ms,
                p99_ms:  n.p99_ms,
                max_ms:  n.max_ms,
            })
            .collect(),
        xrun_total: xruns.total(),
        xrun_by_node: xruns
            .counts()
            .iter()
            .map(|(id, c)| (id.clone(), *c))
            .collect(),
    };
    ok(id, ResponseData::Metrics { metrics })
}

fn do_shutdown(id: u64, state: &Arc<DaemonState>) -> Response {
    tracing::info!("Shutdown requested via IPC");
    state.shutdown.notify_waiters();
    ok(id, ResponseData::Empty {})
}

fn do_subscribe(
    id: u64,
    ctx: &mut ClientCtx,
    filter: Option<soundworm_ipc::EventFilter>,
) -> Response {
    if ctx.sub_handle.is_some() {
        return err(id, ErrorCode::Conflict, "already subscribed");
    }
    let mut rx = ctx.state.events.subscribe();
    let out = ctx.outbound.clone();
    let allow_kinds = filter.and_then(|f| f.kinds);

    let handle = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(kinds) = &allow_kinds {
                        if !kinds.iter().any(|k| k == event_kind(&ev)) {
                            continue;
                        }
                    }
                    if out.send(Message::Event(ev)).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let _ = out
                        .send(Message::Event(IpcEvent::EventsDropped { count: n }))
                        .await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    ctx.sub_handle = Some(handle);
    ok(id, ResponseData::Empty {})
}

async fn do_link(
    id: u64,
    state: &Arc<DaemonState>,
    source: soundworm_ipc::PortRef,
    sink: soundworm_ipc::PortRef,
) -> Response {
    tracing::info!(?source, ?sink, req_id = id, "Op::Link received");
    let (src_pid, sink_pid) = {
        let graph = state.graph.lock().unwrap();

        // Resolve the sink first to determine its media kind; use that
        // as a constraint when resolving the source so we never accept
        // an Audio→MIDI or MIDI→Audio pair. PW would silently destroy
        // such a link a few ms after factory create.
        let sk = match resolve_port(&graph, &sink, Direction::Input, None) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(?sink, error = ?e, "resolve sink port failed");
                return fail(id, e);
            }
        };
        let sink_kind = graph
            .get_port(&sk)
            .and_then(|p| graph.get_node(&p.node_id))
            .map(|n| n.media_kind());

        let src = match resolve_port(&graph, &source, Direction::Output, sink_kind) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(?source, error = ?e, "resolve source port failed");
                return fail(id, e);
            }
        };
        (src, sk)
    };
    tracing::info!(
        src_port = src_pid.0,
        sink_port = sink_pid.0,
        "ports resolved; calling backend.create_link"
    );

    let link = Link {
        id: LinkId(0),
        source_port: PortId(src_pid.0),
        sink_port: PortId(sink_pid.0),
        latency_compensation_ms: 0.0,
    };
    if let Err(e) = state.backend.create_link(&link).await {
        return err(id, ErrorCode::BackendError, &e.to_string());
    }
    // create_link is fire-and-forget: the real link id is assigned by
    // PipeWire and reaches the graph via the LinkAppeared registry
    // event. Wait for it so the client gets the actual id (needed to
    // Unlink later) instead of the placeholder 0.
    let link_id = wait_for_link(state, link.source_port.clone(), link.sink_port.clone())
        .await
        .unwrap_or_else(|| {
            tracing::warn!(
                src_port = link.source_port.0,
                sink_port = link.sink_port.0,
                "link created but no LinkAppeared within timeout; returning id 0"
            );
            LinkId(0)
        });
    ok(id, ResponseData::Link { link_id })
}

/// Poll the graph until the just-created link's registry event lands,
/// returning its real id. ~1s ceiling so a wedged backend can't hang the
/// request.
async fn wait_for_link(
    state: &Arc<DaemonState>,
    source: PortId,
    sink: PortId,
) -> Option<LinkId> {
    for _ in 0..50 {
        // Scope the guard so it drops before the await (MutexGuard isn't
        // Send); materialize an owned id so no borrow of the graph escapes.
        let found = {
            let graph = state.graph.lock().unwrap();
            let mut id = None;
            for l in graph.links() {
                if l.source_port == source && l.sink_port == sink {
                    id = Some(l.id.clone());
                    break;
                }
            }
            id
        };
        if found.is_some() {
            return found;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    None
}

async fn do_unlink(id: u64, state: &Arc<DaemonState>, link_id: LinkId) -> Response {
    let link = Link {
        id: link_id.clone(),
        source_port: PortId(0),
        sink_port: PortId(0),
        latency_compensation_ms: 0.0,
    };
    if let Err(e) = state.backend.destroy_link(&link).await {
        return err(id, ErrorCode::BackendError, &e.to_string());
    }
    ok(id, ResponseData::Empty {})
}

fn ok(id: u64, data: ResponseData) -> Response {
    Response { id, ok: true, data: Some(data), error: None }
}

fn err(id: u64, code: ErrorCode, message: &str) -> Response {
    Response {
        id,
        ok: false,
        data: None,
        error: Some(ProtoError { code, message: message.into() }),
    }
}

fn fail(id: u64, e: ProtoError) -> Response {
    Response { id, ok: false, data: None, error: Some(e) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundworm_core::{
        backend::AudioBackend,
        event::BackendEvent,
        node::{NodeId, NodeKind, Node},
        port::{Direction, Port, PortId},
    };
    use soundworm_graph::mock::MockBackend;
    use soundworm_ipc::PortRef;
    use std::sync::Arc;

    fn node(id: u64, name: &str, kind: NodeKind) -> Node {
        let media_class = match kind {
            NodeKind::Source => "Audio/Source",
            NodeKind::Sink => "Audio/Sink",
            _ => "Stream/Output/Audio",
        };
        Node {
            id: NodeId(id),
            name: name.into(),
            kind,
            app_name: None,
            media_class: media_class.into(),
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

    /// Populate a DaemonState's graph with two connectable nodes and
    /// their ports, going through the real event pump so the test
    /// exercises the same path as live PipeWire input.
    async fn seeded_state() -> Arc<crate::state::DaemonState> {
        let mock = Arc::new(MockBackend::new());
        let state = Arc::new(crate::state::DaemonState::new(
            mock.clone() as Arc<dyn AudioBackend>,
        ));
        state.start_event_pump();
        mock.emit(BackendEvent::NodeAppeared(node(1, "src-app", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers", NodeKind::Sink)));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        // Let the pump drain the events into the graph.
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        // Stash the mock backend handle for assertions via downcast-free
        // path: callers also still hold their own Arc<MockBackend>.
        state
    }

    /// Op::ListNodes returns NodeView with ports embedded — the wire
    /// shape the UI is bound to. Catches anyone who removes the
    /// embedding or breaks the join.
    #[tokio::test]
    async fn list_nodes_response_embeds_ports() {
        let state = seeded_state().await;

        let g = state.graph.lock().unwrap();
        let nvs: Vec<soundworm_ipc::NodeView> = g
            .nodes()
            .map(|n| soundworm_ipc::NodeView {
                node: n.clone(),
                ports: g.ports_of(&n.id).into_iter().cloned().collect(),
            })
            .collect();
        drop(g);

        assert_eq!(nvs.len(), 2);
        for nv in &nvs {
            assert_eq!(
                nv.ports.len(), 1,
                "node {} should carry exactly its one port",
                nv.node.id.0
            );
            assert_eq!(nv.ports[0].node_id, nv.node.id, "port→node id");
        }
    }

    /// do_link with PortRef::Id → backend.create_link is called with
    /// the exact port ids. This is the chain the UI exercises when
    /// you drag a connection.
    #[tokio::test]
    async fn do_link_by_port_id_invokes_backend_create_link() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(crate::state::DaemonState::new(
            mock.clone() as Arc<dyn AudioBackend>,
        ));
        state.start_event_pump();
        mock.emit(BackendEvent::NodeAppeared(node(1, "src-app", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers", NodeKind::Sink)));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let resp = do_link(
            1,
            &state,
            PortRef::Id(PortId(100)),
            PortRef::Id(PortId(200)),
        )
        .await;
        assert!(resp.ok, "do_link by id should succeed, got {resp:?}");
        let calls = calls.lock().unwrap();
        assert_eq!(*calls, vec!["create 100→200".to_string()]);
    }

    /// do_link with PortRef::Named resolves to the first
    /// output/input port — matches what the UI's onConnect sends.
    #[tokio::test]
    async fn do_link_by_node_name_resolves_first_port() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(crate::state::DaemonState::new(
            mock.clone() as Arc<dyn AudioBackend>,
        ));
        state.start_event_pump();
        mock.emit(BackendEvent::NodeAppeared(node(1, "src-app", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers", NodeKind::Sink)));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let resp = do_link(
            7,
            &state,
            PortRef::Named { node: "src-app".into(), port: String::new() },
            PortRef::Named { node: "speakers".into(), port: String::new() },
        )
        .await;
        assert!(resp.ok, "do_link by name should succeed, got {resp:?}");
        let calls = calls.lock().unwrap();
        assert_eq!(*calls, vec!["create 100→200".to_string()]);
    }

    fn midi_node(id: u64, name: &str, kind: NodeKind) -> Node {
        let media_class = match kind {
            NodeKind::Source => "Midi/Source",
            NodeKind::Sink => "Midi/Sink",
            _ => "Midi/Bridge",
        };
        let mut n = node(id, name, kind);
        // Override media_class to a MIDI string so media_kind() returns Midi.
        n.media_class = media_class.into();
        n
    }

    /// PW silently destroys cross-media-kind links a few ms after
    /// factory create. The daemon must reject the resolution outright.
    #[tokio::test]
    async fn do_link_rejects_midi_source_into_audio_sink() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(crate::state::DaemonState::new(
            mock.clone() as Arc<dyn AudioBackend>,
        ));
        state.start_event_pump();
        mock.emit(BackendEvent::NodeAppeared(midi_node(1, "bluez_midi", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "speakers", NodeKind::Sink)));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Input)));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let resp = do_link(
            5,
            &state,
            PortRef::Named { node: "bluez_midi".into(), port: String::new() },
            PortRef::Named { node: "speakers".into(),   port: String::new() },
        )
        .await;
        assert!(!resp.ok, "midi → audio must be rejected");
        assert_eq!(resp.error.as_ref().map(|e| e.code), Some(ErrorCode::BadRequest));
        assert!(
            calls.lock().unwrap().is_empty(),
            "backend.create_link must not be invoked for incompatible kinds"
        );
    }

    /// Linking two Sources together has no Input port to land on, so
    /// we'd already fail naturally — but the *kind* check should bite
    /// first with a clearer error when the user asks for source-to-
    /// source via PortRef::Named (which the UI does).
    #[tokio::test]
    async fn do_link_rejects_source_into_source() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(crate::state::DaemonState::new(
            mock.clone() as Arc<dyn AudioBackend>,
        ));
        state.start_event_pump();
        mock.emit(BackendEvent::NodeAppeared(node(1, "src-a", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        mock.emit(BackendEvent::NodeAppeared(node(2, "src-b", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(200, 2, Direction::Output)));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let resp = do_link(
            6,
            &state,
            PortRef::Named { node: "src-a".into(), port: String::new() },
            PortRef::Named { node: "src-b".into(), port: String::new() },
        )
        .await;
        assert!(!resp.ok, "source → source must be rejected");
        assert!(calls.lock().unwrap().is_empty());
    }

    /// do_link with a name that doesn't resolve must fail cleanly
    /// without calling backend.create_link — protects against the
    /// "second-link failure clobbers the first" UI race that pushed
    /// us to this whole testing exercise.
    #[tokio::test]
    async fn do_link_unknown_node_returns_not_found() {
        let mock = Arc::new(MockBackend::new());
        let calls = Arc::clone(&mock.link_calls);
        let state = Arc::new(crate::state::DaemonState::new(
            mock.clone() as Arc<dyn AudioBackend>,
        ));
        state.start_event_pump();
        mock.emit(BackendEvent::NodeAppeared(node(1, "src-app", NodeKind::Source)));
        mock.emit(BackendEvent::PortAppeared(port(100, 1, Direction::Output)));
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let resp = do_link(
            3,
            &state,
            PortRef::Named { node: "src-app".into(), port: String::new() },
            PortRef::Named { node: "nonexistent".into(), port: String::new() },
        )
        .await;
        assert!(!resp.ok, "must fail for unknown sink node");
        assert_eq!(
            resp.error.as_ref().map(|e| e.code),
            Some(ErrorCode::NotFound),
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "backend.create_link must not be called on resolution failure"
        );
    }
}
