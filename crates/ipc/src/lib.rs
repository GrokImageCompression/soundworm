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

pub const PROTO_VERSION: u32 = 2;

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

/// Internally tagged on `resp`. Tagging (not untagged) is deliberate:
/// several variants share a shape (Script/Snapshot are both `{path}`,
/// Rules/Restore are numeric), and under `#[serde(untagged)]` serde
/// picks the first structurally-matching variant, so those silently
/// mis-parsed. The tag makes every variant unambiguous by construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "resp")]
pub enum ResponseData {
    Hello { daemon_version: String, proto: u32 },
    Nodes { nodes: Vec<NodeView> },
    Ports { ports: Vec<Port> },
    Links { links: Vec<Link> },
    Link { link_id: LinkId },
    Rules { rule_count: usize },
    Script { path: String },
    Snapshot { path: String },
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
    // Nested rather than #[serde(flatten)]: flatten interacts badly with
    // serde's Content buffering (it needs direct deserializer access),
    // so keeping node as its own field sidesteps that regardless of how
    // the enclosing ResponseData is tagged.
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
#[non_exhaustive]
pub enum ErrorCode {
    UnknownOp,
    BadRequest,
    NotFound,
    Conflict,
    BackendError,
    RulesError,
    UnsupportedProto,
    Internal,
    /// A code this client predates. `#[serde(other)]` catches any codes a
    /// newer daemon introduces so the response still deserializes instead
    /// of failing the whole frame.
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
#[non_exhaustive]
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

    // Regression: ListNodes must carry embedded ports through the wire.
    // Originally broke because NodeView used #[serde(flatten)] inside an
    // untagged ResponseData, which silently mis-parsed. NodeView is now
    // nested and ResponseData is tagged; this pins both.
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

    // Regression: Script and Snapshot both carry a single `path`. Under
    // the old untagged ResponseData the first matching variant (Script)
    // always won, so Snapshot mis-parsed and `sw snapshot save` / the UI
    // reported "unexpected response". The `resp` tag disambiguates them.
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

    // --- protocol conformance -------------------------------------------
    // Every wire enum must survive a JSON round-trip landing on the SAME
    // variant. std::mem::discriminant compares variant identity without
    // needing field equality, so these catch the whole mis-parse class
    // (a colliding untagged variant, a dropped tag, a renamed field)
    // regardless of which enum it happens in.

    use std::mem::discriminant;

    fn sample_node() -> Node {
        use soundworm_core::node::{NodeId, NodeKind};
        Node {
            id: NodeId(1),
            name: "n".into(),
            kind: NodeKind::Sink,
            app_name: None,
            media_class: "Audio/Sink".into(),
            sample_rate: 48000,
            channels: 2,
            latency_ms: 0.0,
            properties: std::collections::HashMap::new(),
        }
    }
    fn sample_port() -> Port {
        use soundworm_core::{node::NodeId, port::Direction};
        Port { id: PortId(2), node_id: NodeId(1), name: "p".into(), direction: Direction::Input, channels: 1 }
    }
    fn sample_link() -> Link {
        Link { id: LinkId(3), source_port: PortId(2), sink_port: PortId(4), latency_compensation_ms: 0.0 }
    }

    fn roundtrips<T>(v: &T) -> bool
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(v).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        discriminant(v) == discriminant(&back)
    }

    #[test]
    fn every_response_variant_round_trips_to_same_variant() {
        let metrics = MetricsPayload { nodes: vec![], xrun_total: 0, xrun_by_node: vec![] };
        let all = vec![
            ResponseData::Hello { daemon_version: "0".into(), proto: PROTO_VERSION },
            ResponseData::Nodes { nodes: vec![NodeView { node: sample_node(), ports: vec![sample_port()] }] },
            ResponseData::Ports { ports: vec![sample_port()] },
            ResponseData::Links { links: vec![sample_link()] },
            ResponseData::Link { link_id: LinkId(3) },
            ResponseData::Rules { rule_count: 2 },
            ResponseData::Script { path: "/s.rhai".into() },
            ResponseData::Snapshot { path: "/s.json".into() },
            ResponseData::Restore { applied: 1, skipped: 2 },
            ResponseData::Metrics { metrics },
            ResponseData::Empty {},
        ];
        // Guard against silently dropping a variant from coverage.
        assert_eq!(all.len(), 11, "add new ResponseData variants here");
        for v in &all {
            assert!(roundtrips(v), "ResponseData variant mis-parsed: {v:?}");
        }
    }

    #[test]
    fn every_op_variant_round_trips_to_same_variant() {
        let all = vec![
            Op::Hello { client: "c".into(), version: "1".into() },
            Op::ListNodes,
            Op::ListPorts,
            Op::ListLinks,
            Op::Link { source: PortRef::Id(PortId(1)), sink: PortRef::Id(PortId(2)) },
            Op::Unlink { link_id: LinkId(3) },
            Op::Subscribe { filter: Some(EventFilter { kinds: Some(vec!["XrunObserved".into()]) }) },
            Op::Unsubscribe,
            Op::LoadRules { path: "/r.toml".into() },
            Op::ReloadRules,
            Op::LoadScript { path: "/r.rhai".into() },
            Op::ReloadScript,
            Op::GetMetrics,
            Op::Snapshot { name: "s".into() },
            Op::Restore { name: "s".into() },
            Op::Shutdown,
        ];
        assert_eq!(all.len(), 16, "add new Op variants here");
        for op in &all {
            // Round-trip through the framed Request so the flattened tag
            // is exercised the way the wire actually carries it.
            let msg = Message::Request(Request { id: 1, op: op.clone() });
            let json = serde_json::to_string(&msg).expect("serialize");
            let back: Message = serde_json::from_str(&json).expect("deserialize");
            match back {
                Message::Request(r) => assert!(
                    discriminant(op) == discriminant(&r.op),
                    "Op variant mis-parsed: {op:?}"
                ),
                _ => panic!("frame variant changed for {op:?}"),
            }
        }
    }

    #[test]
    fn every_event_variant_round_trips_and_has_a_kind() {
        use soundworm_core::node::NodeId;
        let all = vec![
            Event::NodeAppeared { node: sample_node() },
            Event::NodeRemoved { node_id: NodeId(1) },
            Event::LinkAppeared { link: sample_link() },
            Event::LinkRemoved { link_id: LinkId(3) },
            Event::RulesApplied { rule: "r".into(), link_id: LinkId(3) },
            Event::LinkRejected { reason: "x".into() },
            Event::EventsDropped { count: 5 },
            Event::XrunObserved { node_id: NodeId(1), gap_ms: 1.5 },
        ];
        assert_eq!(all.len(), 8, "add new Event variants here");
        for ev in &all {
            assert!(roundtrips(ev), "Event variant mis-parsed: {ev:?}");
            // The "kind" tag must be present for subscribers to filter on.
            let json = serde_json::to_string(ev).unwrap();
            assert!(json.contains("\"kind\""), "event missing kind tag: {json}");
        }
    }

    #[test]
    fn error_code_unknown_is_forward_compatible() {
        // Known codes round-trip as their string name.
        let j = serde_json::to_string(&ErrorCode::NotFound).unwrap();
        assert_eq!(j, "\"NotFound\"");
        assert_eq!(serde_json::from_str::<ErrorCode>(&j).unwrap(), ErrorCode::NotFound);
        // A code a newer daemon adds degrades to Unknown instead of failing.
        let future: ErrorCode =
            serde_json::from_str("\"SomeFutureCode\"").expect("unknown code must parse");
        assert_eq!(future, ErrorCode::Unknown);
    }
}
