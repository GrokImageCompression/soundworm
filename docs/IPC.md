# soundworm — Daemon IPC Protocol (v0.2 draft)

Status: draft, pre-implementation. Owner: v0.2 milestone.

## 1. Goals

- Let `sw` (CLI) and future tools talk to `swd` (daemon) without sharing
  process state.
- Stream live `BackendEvent`s to subscribers (TUI, dashboards, tests).
- Stay simple enough to implement in a weekend; no protobuf, no gRPC.

## 2. Non-Goals (v0.2)

- Remote/network access — Unix socket only.
- Multi-user auth — rely on filesystem permissions.
- Backwards compatibility — pre-1.0, breaking changes allowed per minor.

## 3. Transport

- **Socket path:** `$XDG_RUNTIME_DIR/soundworm/swd.sock`
  (fallback `/run/user/$UID/soundworm/swd.sock`).
- **Socket type:** `SOCK_STREAM` (Unix domain).
- **Permissions:** `0600`, owned by the daemon user.
- **Framing:** newline-delimited JSON (NDJSON). One message per line, UTF-8,
  `\n` terminator. Max line length 1 MiB (reject + close on overflow).
- **Concurrency:** daemon accepts many clients; each connection is
  independent. No request multiplexing within a single connection — requests
  are processed in order.

Rationale: NDJSON keeps `socat`/`nc` debugging trivial and matches the
event-stream shape. Length-prefixed framing buys nothing here.

## 4. Message Shape

Every message is a JSON object with a `type` discriminator and a numeric
`id` for request/response correlation.

    { "type": "Request",  "id": 7, "op": "ListNodes" }
    { "type": "Response", "id": 7, "ok": true, "data": { ... } }
    { "type": "Event",    "kind": "NodeAppeared", "node": { ... } }

- `id` is client-assigned, unique per connection, monotonic recommended.
- `Event` messages have no `id` (server-pushed).
- Errors: `{ "type": "Response", "id": N, "ok": false, "error": { "code": "...", "message": "..." } }`.

## 5. Operations (v0.2)

| op              | request fields            | response data                |
|-----------------|---------------------------|------------------------------|
| `Hello`         | `{ client, version }`     | `{ daemon_version, proto: 1 }` |
| `ListNodes`     | —                         | `{ nodes: [Node, ...] }`     |
| `ListPorts`     | —                         | `{ ports: [Port, ...] }`     |
| `ListLinks`     | —                         | `{ links: [Link, ...] }`     |
| `Link`          | `{ source, sink }`        | `{ link_id }`                |
| `Unlink`        | `{ link_id }`             | `{}`                         |
| `Subscribe`     | `{ filter?: EventFilter }`| `{}` then stream of `Event`  |
| `Unsubscribe`   | —                         | `{}`                         |
| `LoadRules`     | `{ path }`                | `{ rule_count }`             |
| `ReloadRules`   | —                         | `{ rule_count }`             |
| `LoadScript`    | `{ path }`                | `{ path }`                   |
| `ReloadScript`  | —                         | `{ path }`                   |
| `GetMetrics`    | —                         | `{ metrics: MetricsPayload }`|
| `Snapshot`      | `{ name }`                | `{ path }`                   |
| `Restore`       | `{ name }`                | `{ applied, skipped }`       |
| `Shutdown`      | —                         | `{}` then close              |

Node/Link payloads serialize from the existing `soundworm-core` types
(already `Serialize`/`Deserialize`).

`source`/`sink` in `Link` accept either a `PortId` or a
`{ node, port_name }` tuple — the daemon resolves the latter via
`AudioGraph::ports_of`.

## 6. Event Stream

After a successful `Subscribe`, the daemon pushes one `Event` per
`BackendEvent` plus synthetic `RulesApplied`/`LinkRejected` events from the
policy layer. Backpressure: bounded per-connection queue (suggest 1024); if
full, drop oldest non-critical events and emit `EventsDropped { count }`.

## 7. Error Codes

`UnknownOp`, `BadRequest`, `NotFound`, `Conflict`, `BackendError`,
`RulesError`, `Internal`. Strings, not numbers — easier to grep.

## 8. Versioning

- Wire protocol carries a single integer `proto` (currently `1`).
- `Hello` is mandatory first message; daemon closes the connection on
  mismatch with `error.code = "UnsupportedProto"`.

## 9. Resolved Design Decisions

- **No `Watch` op.** `ListNodes` + `Subscribe` already gives a client the
  current snapshot plus the live delta stream. A single-shot diff op would
  duplicate that path and force the daemon to track per-client cursors.
  Revisit only if a real consumer needs it.
- **Snapshot/Restore stay daemon-side at the IPC boundary; logic stays in
  `soundworm-snapshots`.** The daemon is the only process with the live
  `AudioGraph` and backend handle, so it must mediate. The policy crate is
  about rules evaluation, not persistence — keeping them separate avoids
  pulling filesystem concerns into rule eval.
- **No capability flags in `Hello` for v0.2.** `proto: 1` is the only
  negotiation knob. Add a `caps` array in v0.3 when Rhai scripting lands
  and clients actually need to feature-detect.

## 10. Implementation Order

1. Socket bind + accept loop in `swd` (tokio `UnixListener`).
2. NDJSON codec (`tokio_util::codec::LinesCodec`).
3. `Request`/`Response` types in a new `soundworm-ipc` crate (shared by
   `sw` and `swd`).
4. Wire `ListNodes`/`ListLinks`/`Link`/`Unlink` against existing
   `AudioGraph` + backend.
5. `Subscribe` — forward `BackendEvent` mpsc into per-client channel.
6. Migrate `sw` subcommands to talk to the socket; keep `--in-process`
   flag as an escape hatch for tests.
7. `LoadRules`/`ReloadRules` last — depends on TOML rules engine work.
