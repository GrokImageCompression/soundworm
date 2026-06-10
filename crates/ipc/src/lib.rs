//! Wire types for the soundworm daemon IPC protocol.
//!
//! See `docs/IPC.md` for the spec. NDJSON over a Unix socket; this crate
//! holds only the message structs, framing helpers, and the proto version.

pub mod client;
pub mod codec;

use std::path::PathBuf;

/// Default socket path used by both `swd` and `sw`. Override with
/// `SOUNDWORM_SOCK`. Falls back to `/tmp` if `XDG_RUNTIME_DIR` is unset.
pub fn default_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("SOUNDWORM_SOCK") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("soundworm").join("swd.sock")
}

use serde::{Deserialize, Serialize};
use soundworm_core::{
    link::{Link, LinkId},
    node::{Node, NodeId},
    port::PortId,
};

pub const PROTO_VERSION: u32 = 1;

/// Top-level frame. `type` is the discriminator on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Request(Request),
    Response(Response),
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    Hello { client: String, version: String },
    ListNodes,
    ListLinks,
    Link { source: PortRef, sink: PortRef },
    Unlink { link_id: LinkId },
    Subscribe { filter: Option<EventFilter> },
    Unsubscribe,
    LoadRules { path: String },
    ReloadRules,
    Snapshot { name: String },
    Restore { name: String },
    Shutdown,
}

/// A port can be addressed by raw id (fast path) or by node + port name
/// (the form `sw link` accepts on the CLI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PortRef {
    Id(PortId),
    Named { node: String, port: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub kinds: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ResponseData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtoError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseData {
    Hello { daemon_version: String, proto: u32 },
    Nodes { nodes: Vec<Node> },
    Links { links: Vec<Link> },
    Link { link_id: LinkId },
    Rules { rule_count: usize },
    Snapshot { path: String },
    Restore { applied: usize, skipped: usize },
    Empty {},
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ProtoError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    UnknownOp,
    BadRequest,
    NotFound,
    Conflict,
    BackendError,
    RulesError,
    UnsupportedProto,
    Internal,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    NodeAppeared { node: Node },
    NodeRemoved { node_id: NodeId },
    LinkAppeared { link: Link },
    LinkRemoved { link_id: LinkId },
    RulesApplied { rule: String, link_id: LinkId },
    LinkRejected { reason: String },
    EventsDropped { count: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_request() {
        let req = Message::Request(Request {
            id: 7,
            op: Op::ListNodes,
        });
        let line = serde_json::to_string(&req).unwrap();
        let back: Message = serde_json::from_str(&line).unwrap();
        match back {
            Message::Request(r) => {
                assert_eq!(r.id, 7);
                assert!(matches!(r.op, Op::ListNodes));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn portref_accepts_id_or_named() {
        let by_id: PortRef = serde_json::from_str("42").unwrap();
        assert!(matches!(by_id, PortRef::Id(_)));
        let named: PortRef =
            serde_json::from_str(r#"{"node":"Firefox","port":"output_FL"}"#).unwrap();
        assert!(matches!(named, PortRef::Named { .. }));
    }
}
