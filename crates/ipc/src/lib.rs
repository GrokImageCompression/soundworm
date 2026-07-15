//! Wire types for the soundworm daemon IPC protocol.
//!
//! See `docs/IPC.md` for the spec. NDJSON over a Unix socket; this crate
//! holds only the message structs, framing helpers, and the proto version.

// Client transport is unix-only (tokio's UnixStream isn't on Windows).
// A Windows named-pipe backend will land alongside the WASAPI work in
// v0.6; until then the IPC wire types are still usable cross-platform.
#[cfg(unix)]
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
    port::{Port, PortId},
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
    ListPorts,
    ListLinks,
    Link { source: PortRef, sink: PortRef },
    Unlink { link_id: LinkId },
    Subscribe { filter: Option<EventFilter> },
    Unsubscribe,
    LoadRules { path: String },
    ReloadRules,
    LoadScript { path: String },
    ReloadScript,
    GetMetrics,
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
    Nodes { nodes: Vec<NodeView> },
    Ports { ports: Vec<Port> },
    Links { links: Vec<Link> },
    Link { link_id: LinkId },
    Rules { rule_count: usize },
    Script { path: String },
    // Distinct JSON key from Script's `path`: in an untagged enum serde
    // picks the first structurally-matching variant, so an identical
    // `{path}` shape would always deserialize as Script and never reach
    // Snapshot (same footgun documented on NodeView).
    Snapshot {
        #[serde(rename = "snapshot_path")]
        path: String,
    },
    Restore { applied: usize, skipped: usize },
    Metrics { metrics: MetricsPayload },
    Empty {},
}

/// `ListNodes` wire payload. Inlines the node's ports so the UI only
/// needs one round-trip to draw the graph and so port→node mapping is
/// already established without a join. `ListPorts` is still available
/// for callers that just want the flat port list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    // Nested rather than flattened: #[serde(flatten)] inside an
    // #[serde(untagged)] enum variant (ResponseData::Nodes) is a known
    // serde footgun — the flatten path requires direct deserializer
    // access that untagged's Content buffer can't provide, so the
    // Nodes variant fails to match and Empty {} silently wins.
    pub node: Node,
    #[serde(default)]
    pub ports: Vec<Port>,
}

/// Wire shape mirrors `soundworm_observability::MetricsSnapshot` but
/// kept independent so the IPC crate doesn't depend on observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsPayload {
    pub nodes: Vec<NodeLatencyPayload>,
    pub xrun_total: u64,
    pub xrun_by_node: Vec<(NodeId, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLatencyPayload {
    pub node_id: NodeId,
    pub count:   u64,
    pub min_ms:  f32,
    pub p50_ms:  f32,
    pub p95_ms:  f32,
    pub p99_ms:  f32,
    pub max_ms:  f32,
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
    XrunObserved { node_id: NodeId, gap_ms: f32 },
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

    // Regression: `#[serde(flatten)]` inside a `#[serde(untagged)]`
    // variant doesn't work — serde's untagged code path uses a Content
    // buffer that the flatten path can't consume from, so the
    // ResponseData::Nodes arm silently fails to match and Empty {}
    // wins. We hit this in v0.5-ui and the UI showed "unexpected
    // response: Empty" for every ListNodes call. NodeView is now
    // nested, not flattened; this test pins that decision.
    #[test]
    fn list_nodes_response_round_trips_with_embedded_ports() {
        use soundworm_core::{
            node::{Node, NodeId, NodeKind},
            port::{Direction, Port, PortId},
        };
        use std::collections::HashMap;

        let n = Node {
            id: NodeId(7),
            name: "alsa_output.default".into(),
            kind: NodeKind::Sink,
            app_name: None,
            media_class: "Audio/Sink".into(),
            sample_rate: 48000,
            channels: 2,
            latency_ms: 0.0,
            properties: HashMap::new(),
        };
        let p = Port {
            id: PortId(42),
            node_id: NodeId(7),
            name: "playback_FL".into(),
            direction: Direction::Input,
            channels: 1,
        };
        let resp = Message::Response(Response {
            id: 9,
            ok: true,
            data: Some(ResponseData::Nodes {
                nodes: vec![NodeView { node: n.clone(), ports: vec![p.clone()] }],
            }),
            error: None,
        });

        // Serialize and deserialize end-to-end through the wire format.
        let line = serde_json::to_string(&resp).expect("serialize");
        let back: Message = serde_json::from_str(&line).expect("deserialize");

        let data = match back {
            Message::Response(r) => r.data.expect("data present"),
            _ => panic!("not a Response"),
        };
        match data {
            ResponseData::Nodes { nodes } => {
                assert_eq!(nodes.len(), 1, "node count");
                assert_eq!(nodes[0].node.id, NodeId(7));
                assert_eq!(nodes[0].ports.len(), 1, "ports embedded");
                assert_eq!(nodes[0].ports[0].id, PortId(42));
            }
            other => panic!("variant mismatch: {other:?} — flatten+untagged regression"),
        }
    }

    /// An older daemon (or a node with no ports yet) sends nodes
    /// without the `ports` field. New IPC clients must still parse
    /// these into NodeView with an empty ports vec.
    #[test]
    fn node_view_accepts_missing_ports_field() {
        let raw = r#"{
            "node": {
                "id": 1,
                "name": "test",
                "kind": "Source",
                "app_name": null,
                "media_class": "Audio/Source",
                "sample_rate": 48000,
                "channels": 2,
                "latency_ms": 0.0,
                "properties": {}
            }
        }"#;
        let nv: NodeView = serde_json::from_str(raw).expect("parse without ports");
        assert!(nv.ports.is_empty());
    }

    #[test]
    fn portref_accepts_id_or_named() {
        let by_id: PortRef = serde_json::from_str("42").unwrap();
        assert!(matches!(by_id, PortRef::Id(_)));
        let named: PortRef =
            serde_json::from_str(r#"{"node":"Firefox","port":"output_FL"}"#).unwrap();
        assert!(matches!(named, PortRef::Named { .. }));
    }

    // Regression: Script and Snapshot both carry a single path. In an
    // untagged enum, identical shapes make the first variant (Script)
    // always win, so a Snapshot response deserialized as Script and
    // `sw snapshot save` / the UI reported "unexpected response". The
    // snapshot_path rename keeps the two shapes distinct.
    #[test]
    fn snapshot_response_does_not_collide_with_script() {
        let snap = serde_json::to_string(&ResponseData::Snapshot { path: "/x/s.json".into() })
            .expect("serialize");
        assert!(matches!(
            serde_json::from_str::<ResponseData>(&snap).expect("deserialize"),
            ResponseData::Snapshot { .. }
        ));

        let script = serde_json::to_string(&ResponseData::Script { path: "/x/r.rhai".into() })
            .expect("serialize");
        assert!(matches!(
            serde_json::from_str::<ResponseData>(&script).expect("deserialize"),
            ResponseData::Script { .. }
        ));
    }
}
