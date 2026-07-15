# soundworm IPC protocol

Wire protocol between `sw` (CLI) / UI clients and `swd` (daemon).

Status: implemented, `proto = 2`. Pre-1.0, so a minor release may still
bump `proto`; the conformance tests in `crates/ipc/src/lib.rs` pin every
frame shape described here.

## 1. Transport

- Socket: `$SOUNDWORM_SOCK` if set, else
  `$XDG_RUNTIME_DIR/soundworm/swd.sock`, else `/tmp/soundworm/swd.sock`.
- Unix domain, `SOCK_STREAM`, daemon-owned, filesystem permissions are
  the only access control (localhost, single user).
- Framing: newline-delimited JSON. One UTF-8 message per `\n`-terminated
  line, max 1 MiB per line (`codec::MAX_LINE_BYTES`); a longer line is a
  `LineTooLong` error and the connection closes.
- Concurrency: many independent connections. Within one connection
  requests are answered in order; there is no multiplexing.

## 2. Message envelope

Every frame is a JSON object tagged by `type`: `Request`, `Response`, or
`Event`.

    { "type": "Request",  "id": 7, "op": "ListNodes" }
    { "type": "Response", "id": 7, "ok": true, "data": { "resp": "Nodes", "nodes": [ ... ] } }
    { "type": "Event",    "kind": "NodeAppeared", "node": { ... } }

- `id` is client-assigned, unique per connection, and echoed on the
  matching `Response`. `Event` frames are server-pushed and carry no `id`.
- A `Request` carries the op tag `op` plus that op's fields (flattened).
- A `Response` carries `ok` plus exactly one of `data` (on success) or
  `error` (on failure). `data` is an object tagged by `resp`.
- An `Event` carries the event tag `kind` plus that event's fields.

## 3. Operations

Client sends `{ "type": "Request", "id": N, "op": "<Op>", ...fields }`.
Response `data` is the object in the last column.

| op            | request fields              | success `data` (`resp`, fields)     |
|---------------|-----------------------------|-------------------------------------|
| `Hello`       | `client`, `version`         | `Hello`  `daemon_version`, `proto`  |
| `ListNodes`   | .                           | `Nodes`  `nodes: [NodeView]`        |
| `ListPorts`   | .                           | `Ports`  `ports: [Port]`            |
| `ListLinks`   | .                           | `Links`  `links: [Link]`            |
| `Link`        | `source: PortRef`, `sink: PortRef` | `Link`  `link_id`            |
| `Unlink`      | `link_id`                   | `Empty`                             |
| `Subscribe`   | `filter?: EventFilter`      | `Empty`, then an `Event` stream     |
| `Unsubscribe` | .                           | `Empty`                             |
| `LoadRules`   | `path`                      | `Rules`  `rule_count`               |
| `ReloadRules` | .                           | `Rules`  `rule_count`               |
| `LoadScript`  | `path`                      | `Script`  `path`                    |
| `ReloadScript`| .                           | `Script`  `path`                    |
| `GetMetrics`  | .                           | `Metrics`  `metrics: MetricsPayload`|
| `Snapshot`    | `name`                      | `Snapshot`  `path`                  |
| `Restore`     | `name`                      | `Restore`  `applied`, `skipped`     |
| `Shutdown`    | .                           | `Empty`, then the daemon exits      |

`resp` is tagged (not inferred from shape) precisely because several
payloads share a shape: `Script` and `Snapshot` are both `{ path }`,
`Rules` and `Restore` are both numeric. An untagged enum silently
mis-parsed those; the tag makes each unambiguous.

## 4. Events

After a successful `Subscribe`, the daemon pushes one `Event` per
relevant change. `filter.kinds` (array of kind strings) narrows the
stream; omit it for all events.

| kind            | fields                    |
|-----------------|---------------------------|
| `NodeAppeared`  | `node: Node`              |
| `NodeRemoved`   | `node_id`                 |
| `LinkAppeared`  | `link: Link`              |
| `LinkRemoved`   | `link_id`                 |
| `RulesApplied`  | `rule`, `link_id`         |
| `LinkRejected`  | `reason`                  |
| `EventsDropped` | `count`                   |
| `XrunObserved`  | `node_id`, `gap_ms`       |

Backpressure: the per-connection queue is bounded. On overflow the
daemon drops events and emits `EventsDropped { count }` so a client knows
its view may be stale and can re-fetch via `ListNodes` / `ListLinks`.

## 5. Handshake and versioning

- `Hello` must be the first op on a connection. Any other first op gets
  `error.code = "BadRequest"` ("Hello required first").
- The daemon replies `Hello { daemon_version, proto }`.
- The client compares `proto` to its own and disconnects on mismatch.
  The `Hello` request does not carry the client's proto, so the daemon
  does not enforce the version; detection is client-side.

## 6. Error model

Failure response: `{ "type": "Response", "id": N, "ok": false,
"error": { "code": "<ErrorCode>", "message": "..." } }`.

Codes are strings (grep-friendly): `UnknownOp`, `BadRequest`,
`NotFound`, `Conflict`, `BackendError`, `RulesError`, `UnsupportedProto`,
`Internal`. A newer daemon may add codes; clients decode an unrecognized
one to `Unknown` rather than failing the frame (`#[serde(other)]`).

## 7. Payload types

Serialized from `soundworm-core`. Id newtypes (`NodeId`, `PortId`,
`LinkId`) are bare JSON numbers (u64).

    Node   { id, name, kind, app_name, media_class,
             sample_rate, channels, latency_ms, properties }
    Port   { id, node_id, name, direction, channels }
    Link   { id, source_port, sink_port, latency_compensation_ms }

- `kind`: `"Source" | "Sink" | "Filter" | "Virtual"`.
- `direction`: `"Input" | "Output"`.
- `app_name`: string or `null`. `properties`: string to string map.

`NodeView` (the `ListNodes` element) inlines each node's ports so the UI
draws the graph in one round-trip:

    NodeView { node: Node, ports: [Port] }

`ports` defaults to `[]` when absent.

`PortRef` (a `Link` endpoint) is either a bare `PortId` number or a name
pair `{ node, port }` that the daemon resolves against the live graph:

    "source": 64
    "source": { "node": "Firefox", "port": "output_FL" }

`MetricsPayload` (from `GetMetrics`):

    MetricsPayload {
      nodes: [ { node_id, count, min_ms, p50_ms, p95_ms, p99_ms, max_ms } ],
      xrun_total,
      xrun_by_node: [ [node_id, count], ... ]
    }

## 8. Design decisions

- No `Watch` op: `ListNodes`/`ListLinks` for the snapshot plus
  `Subscribe` for the delta stream cover it without per-client cursors.
- Snapshot/Restore are mediated by the daemon (it owns the live graph and
  backend), but the persistence logic stays in `soundworm-snapshots`.
- `proto` is the only negotiation knob; no capability flags yet. Add a
  `caps` array when a client actually needs to feature-detect.
