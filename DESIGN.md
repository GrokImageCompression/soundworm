# soundworm — Design Document

## 1. Goals

- Route audio between applications and hardware on Linux (PipeWire),
  macOS (CoreAudio), and Windows (WASAPI)
- Sub-100ms reaction time to node appearance/disappearance
- Rules-driven automatic routing with Rhai scripting override
- Session save/restore
- Observable: xrun detection, per-node latency metrics
- Library-first: daemon and CLI are thin wrappers

## 2. Non-Goals (v0.x)

- Audio DSP or mixing
- GUI application
- Replacing PipeWire itself
- Network audio transport

## 3. Architecture

    CLI (sw) / Daemon (swd)
              |
              v
      soundworm-graph           (AudioGraph: nodes/ports/links, pure data)
              |
       +------+------+
       |             |
       v             v
     policy     observability   (rules, xrun log, metrics)
       |
       v
    backend trait
       |
       +---- pipewire-backend   (primary)
       +---- coreaudio-backend  (stub)
       +---- wasapi-backend     (stub)

## 4. Core Types (soundworm-core)

    pub struct NodeId(pub u64);
    pub struct PortId(pub u64);
    pub struct LinkId(pub u64);

    pub enum Direction { Input, Output }

    pub struct Node {
        pub id: NodeId,
        pub name: String,
        pub app_name: Option<String>,
        pub media_class: String,     // "Audio/Sink", "Stream/Output/Audio", ...
    }

    pub struct Port {
        pub id: PortId,
        pub node: NodeId,
        pub direction: Direction,
        pub channel: String,         // "FL", "FR", "MONO", ...
    }

    pub struct Link {
        pub id: LinkId,
        pub from: PortId,
        pub to: PortId,
    }

    pub trait AudioBackend: Send + Sync {
        fn enumerate(&self) -> Result<Vec<Node>>;
        fn subscribe(&self) -> Receiver<BackendEvent>;
        fn link(&self, from: PortId, to: PortId) -> Result<LinkId>;
        fn unlink(&self, id: LinkId) -> Result<()>;
    }

## 5. Graph (soundworm-graph)

In-memory authoritative model. Updated from backend events. Queryable by
policy engine. Never blocks on I/O.

## 6. Policy (soundworm-policy)

- Loads TOML rules from XDG config dir
- Matches NodeAppeared events against rule predicates
- Resolves conflicts by `priority` (higher wins)
- Emits routing actions to backend
- Optionally delegates decision to Rhai engine

## 7. Rhai Engine (soundworm-rhai-engine)

- Exposes node metadata to script
- Script returns allow()/deny() and optional target
- Hot-reloads on file change (notify crate)

## 8. Backends

### 8.1 PipeWire (primary)
- libpipewire via `pipewire` crate
- Single-threaded loop in dedicated thread
- Events forwarded via crossbeam channel to graph

### 8.2 CoreAudio (stub)
- Skeleton trait impl, returns empty enumeration

### 8.3 WASAPI (stub)
- Skeleton trait impl, returns empty enumeration

## 9. Observability

- tracing crate for structured logs
- Xrun counter per node
- Latency histogram (hdrhistogram) per link
- `sw metrics` dumps current snapshot as JSON

## 10. Snapshots

- Serialize Vec<Link> + rule set hash to JSON
- Restore: diff against current graph, apply minimal changes
- Stored under $XDG_DATA_HOME/soundworm/snapshots/

## 11. Daemon (swd)

- Loads config
- Starts backend
- Runs policy loop
- Exposes Unix socket for CLI (length-prefixed JSON RPC)

## 12. CLI (sw)

- Connects to swd socket
- Pretty-prints with `comfy-table`
- Falls back to direct backend if daemon not running (read-only ops)

## 13. Testing

- Unit tests per crate
- Integration: mock backend implementing AudioBackend, drives graph
- CI: GitHub Actions on Fedora container

## 14. Roadmap to 1.0

### Current state (2026-06)

Skeleton only. All crates compile as stubs (~650 LOC total). No backend
talks to a real audio system yet, no IPC protocol exists, no tests run
against a real graph. v0.1 is the first milestone with end-user value.

### Cross-cutting tracks (worked on continuously)

- **CI**: GitHub Actions matrix (Fedora container w/ PipeWire, macOS,
  Windows). `cargo build`, `cargo test --workspace`, `cargo clippy -D
  warnings`, `cargo fmt --check`. Required from v0.1.
- **Error model**: single `soundworm_core::Error` enum, `thiserror`-based,
  no `anyhow` outside binaries. Lock down before v0.5.
- **Logging**: every backend event, policy decision, and IPC call carries
  a tracing span with `node_id`/`link_id`. Established by v0.2.
- **Docs**: rustdoc on every public item in `core`; mdBook user guide
  started at v0.3.

### v0.1 — PipeWire MVP (foundation)

Goal: a developer on Fedora can enumerate the graph and manually wire
nodes from the CLI.

- `pipewire-backend`: real `enumerate()` via libpipewire registry walk
- `pipewire-backend`: `subscribe()` emitting `NodeAppeared`,
  `NodeRemoved`, `PortAppeared`, `LinkChanged`
- `pipewire-backend`: working `link()`/`unlink()` against the PW core
- `graph`: apply events idempotently; expose `find_node_by_name`,
  `ports_of(node)`
- `cli`: `sw list`, `sw link <src> <sink>`, `sw unlink <id>` running
  in-process (no daemon yet)
- `snapshots`: save/restore link set to JSON
- Tests: mock backend driving the graph through a scripted event
  sequence

Exit: on a stock Fedora 41 box, `sw list` matches `pw-cli ls Node`
within 100 ms of a node appearing.

### v0.2 — Daemon + rules

Goal: declarative auto-routing without writing code.

- `daemon`: tokio runtime, owns the backend, exposes Unix socket
- IPC: length-prefixed JSON-RPC, versioned (`"protocol": 1`); spec
  written down before code
- `policy::rules`: load TOML from `$XDG_CONFIG_HOME/soundworm/rules/`,
  evaluate on `NodeAppeared`
- `policy::conflict`: deterministic resolution by `priority` then rule
  name
- `cli`: switch to socket transport; fall back to direct backend for
  read-only ops when daemon is down
- `contrib/systemd`: working user unit, `Restart=on-failure`
- Tests: golden TOML files + scripted backend events → expected link
  set

Exit: spotify launches → routed to configured sink in <100 ms; daemon
survives PipeWire restart; `journalctl --user -u soundworm` is clean.

### v0.3 — Rhai scripting + hot reload

Goal: rules can express logic TOML can't.

- `rhai-engine`: registered API — `node.name`, `node.app`,
  `node.media_class`, `sinks()`, `route(target)`, `allow()`, `deny()`,
  `log_route(node, target)`
- Policy chain: TOML rules first, fall through to Rhai if no match;
  Rhai can override with `priority`
- `notify`-based file watcher for `routing.rhai`; reload is atomic
  (parse new script before swapping)
- Script execution timeout (default 50 ms) — abort + log on overrun
- Tests: script unit tests via `rhai`'s test harness

Exit: editing `routing.rhai` takes effect within 1 s without restart;
malformed script logs an error and keeps the previous one active.

### v0.4 — Observability

Goal: you can answer "why is audio glitching?" without `pw-top`.

- `observability::xrun`: subscribe to PipeWire xrun events, counter per
  node, last-N ring buffer
- `observability::metrics`: `hdrhistogram` of link latency, sampled
  from PW driver info
- IPC: `sw metrics` returns JSON snapshot; `sw metrics --watch` streams
- Optional Prometheus exporter behind a feature flag
- Tests: synthetic xrun injection via mock backend

Exit: induced xrun (e.g. CPU spike) shows up in `sw metrics` within
1 s; latency histograms match `pw-top` numbers ±10%.

### v0.5 — CoreAudio backend

Goal: feature parity on macOS for enumerate/link/unlink.

- Replace stub with real `AudioBackend` impl using `coreaudio-rs`
- Map CoreAudio device/stream/format model onto `Node`/`Port`
- HAL property listeners → `BackendEvent`s
- CI: macOS runner builds and runs mock-backend tests; integration
  tests gated on `cfg(target_os = "macos")`
- Document semantic gaps vs PipeWire (e.g. no arbitrary port-to-port
  linking; route to default device instead)

Exit: `sw list` and `sw link` work on a macOS dev box; rules engine
unchanged.

### v0.6 — WASAPI backend

Goal: same on Windows.

- `windows-rs` bindings for `IMMDeviceEnumerator`,
  `IAudioSessionManager2`
- `IMMNotificationClient` → device-change events
- Session-level routing via `IAudioSessionControl` where possible
- CI: Windows runner

Exit: same as v0.5 but on Windows 11.

### v0.7 — Hardening

Goal: nothing in `core` or the IPC protocol will need a breaking
change to reach 1.0.

- API review of every `pub` item in `core`, `graph`, `policy`;
  `#[non_exhaustive]` on all event/error enums
- IPC protocol freeze: write a numbered spec doc, add a conformance
  test suite
- Fuzz `policy::rules` TOML parsing and Rhai script evaluation
  (`cargo-fuzz`)
- Soak test: 24 h run with synthetic node churn, watch for leaks
  (`heaptrack`) and link-id exhaustion
- Document migration path for anyone on v0.x

Exit: zero `pub` items marked `#[doc(hidden)]` or `unstable`; fuzz
runs 1 h clean; soak test RSS flat after warmup.

### v0.8 — Beta

Goal: real users on Linux.

- Tag `0.8.0`, publish to crates.io
- COPR repo for Fedora; AUR `PKGBUILD`
- mdBook user guide complete: install, rules cookbook, scripting
  guide, troubleshooting
- Issue triage SLA, public Matrix/Discord channel
- Collect feedback for one release cycle

Exit: ≥10 external users reporting; no P0 bugs open for >2 weeks.

### v0.9 — Release candidate

- Address P0/P1 feedback from v0.8
- Final API sweep; semver-checks in CI (`cargo-semver-checks`)
- Performance budget verified: enumerate 200 nodes <50 ms, event
  latency p99 <100 ms

### v1.0 — Stable

- Semver guarantee on `soundworm-core`, `soundworm-graph`,
  `soundworm-policy`, and the IPC protocol
- Backends remain `0.x` (platform churn) but trait is frozen
- macOS and Windows backends at feature parity (enumerate/link/unlink/
  events); platform gaps documented
- Release announcement, changelog, upgrade guide from 0.x

### Risks & open questions

- **PipeWire crate maturity**: the `pipewire` crate's API has churned;
  may need a thin C-FFI wrapper if it stalls. Decide by v0.2.
- **CoreAudio routing model**: arbitrary port linking isn't a thing.
  Define what `link()` means on macOS before v0.5 — probably "set
  default device for app" via Audio HAL.
- **Rhai sandboxing**: default Rhai allows unbounded loops. Need
  `Engine::set_max_operations` plus the 50 ms wall-clock timeout from
  v0.3.
- **IPC auth**: Unix socket perms only, or token? Decide before v0.2
  freezes the protocol.
- **Snapshot portability**: node names aren't stable across reboots on
  PipeWire. Snapshots must match on `(app_name, media_class, channel)`
  tuples, not raw IDs. Spec this in v0.1.
