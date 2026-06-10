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
    codec, ErrorCode, Event as IpcEvent, Message, Op, ProtoError, Request, Response, ResponseData,
    PROTO_VERSION,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

const OUTBOUND_CAP: usize = 1024;

pub fn socket_path() -> PathBuf {
    soundworm_ipc::default_socket_path()
}

pub async fn serve(path: PathBuf, state: Arc<DaemonState>) -> Result<()> {
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
            if let Err(e) = handle_client(stream, state).await {
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

async fn handle_client(stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
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
            let nodes = ctx.state.graph.lock().unwrap().nodes().cloned().collect();
            ok(id, ResponseData::Nodes { nodes })
        }
        Op::ListLinks => {
            let links = ctx.state.graph.lock().unwrap().links().cloned().collect();
            ok(id, ResponseData::Links { links })
        }
        Op::Link { source, sink } => do_link(id, &ctx.state, source, sink).await,
        Op::Unlink { link_id } => do_unlink(id, &ctx.state, link_id).await,
        Op::Subscribe { filter } => do_subscribe(id, ctx, filter),
        Op::Unsubscribe => {
            if let Some(h) = ctx.sub_handle.take() {
                h.abort();
            }
            ok(id, ResponseData::Empty {})
        }
        Op::LoadRules { .. }
        | Op::ReloadRules
        | Op::Snapshot { .. }
        | Op::Restore { .. }
        | Op::Shutdown => err(id, ErrorCode::UnknownOp, "not implemented yet"),
    }
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
    let (src_pid, sink_pid) = {
        let graph = state.graph.lock().unwrap();
        let src = match resolve_port(&graph, &source, Direction::Output) {
            Ok(p) => p,
            Err(e) => return fail(id, e),
        };
        let sk = match resolve_port(&graph, &sink, Direction::Input) {
            Ok(p) => p,
            Err(e) => return fail(id, e),
        };
        (src, sk)
    };

    let link = Link {
        id: LinkId(0),
        source_port: PortId(src_pid.0),
        sink_port: PortId(sink_pid.0),
        latency_compensation_ms: 0.0,
    };
    if let Err(e) = state.backend.create_link(&link).await {
        return err(id, ErrorCode::BackendError, &e.to_string());
    }
    ok(id, ResponseData::Link { link_id: link.id })
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
