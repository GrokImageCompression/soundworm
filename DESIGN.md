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
- GUI application *in the core workspace's release artifacts* (a Tauri UI
  is being scaffolded as a separate layer above the daemon — see §15)
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

### Current state (2026-06-14)

v0.5 skeleton landed 2026-06-14 (HAL hookup deferred to a macOS dev box). Daemon `Cargo.toml` now selects backends per-target (`PipeWireBackend` on linux, `CoreAudioBackend` on macos, `WasapiBackend` on windows) via `[target.'cfg(target_os = "…")'.dependencies]`; `main.rs` resolves the chosen impl through a `PlatformBackend` type alias. CLI's `--in-process` flag is now Linux-only and returns a clear error elsewhere. `soundworm-coreaudio` gains a `macos` module behind `#[cfg(target_os = "macos")]` with `Inner::{start, subscribe, enumerate_nodes, set_default_output, set_volume}` scaffolding, `device_id_to_node_id`/`node_id_to_device_id` conversions (tested), and `coreaudio-sys = "0.2"` as a target-only dep. The actual HAL calls are clearly marked `TODO(v0.5-mac)` and currently return empty / log-only — they're a focused session against a real Mac, not speculative code shipped from a Linux box. `.github/workflows/ci.yml` adds a 3-OS matrix: Fedora 41 container (real PW headers), macOS-latest (compiles coreaudio-backend with real bindings, excludes `soundworm-pipewire`), Windows-latest (stub builds only, excludes the unix backends). Semantic gaps vs PipeWire documented in `crates/coreaudio-backend/src/lib.rs`: HAL has no port-to-port linking, so `create_link` sets the system default output/input device; `destroy_link` is a no-op; ports are synthesized one-per-stream-direction; per-app routing requires an HAL plugin (out of scope).

v0.1 shipped 2026-06-09. v0.2 shipped 2026-06-14. v0.3 shipped 2026-06-14. v0.4 shipped 2026-06-14:

- `soundworm-observability` rewritten: `XrunLog` is a bounded ring (CAP=1024) with per-node counters; `Metrics` wraps per-node `hdrhistogram::Histogram` storing µs internally and exposing ms percentiles via `MetricsSnapshot` (min/p50/p95/p99/max).
- `BackendEvent::Xrun { node_id, gap_ms }` and `BackendEvent::LatencySample { node_id, latency_ms }` added. PipeWire backend emits `LatencySample` via per-node info listeners that parse `node.latency = "samples/rate"` into ms (debounced). Xrun emission landed as a partial: when info props include `xrun-count` (JACK clients and similar), the backend diffs against a per-node baseline and emits one `BackendEvent::Xrun` per delta. The first observation only records the baseline. The fuller Profiler-POD path (catches ALSA/PW-native nodes) still requires unwrapping `pw::profiler::Profiler` via raw FFI + SPA POD parsing; deferred to v0.5+.
- Daemon `start_event_pump` records xruns/latency into shared `state.xruns`/`state.metrics` and broadcasts `IpcEvent::XrunObserved` to subscribers.
- New IPC op `GetMetrics` → `{ metrics: MetricsPayload }` (`MetricsPayload`/`NodeLatencyPayload` keep the IPC crate observability-dep-free).
- CLI: `sw metrics` prints a comfy-table summary; `sw metrics --json` dumps the wire payload; `sw metrics --watch` subscribes filtered to `XrunObserved`.
- Prometheus exporter behind `observability/prometheus` feature: hand-rolled text format covering `soundworm_xrun_total`, `soundworm_xrun_count{node=…}`, `soundworm_latency_ms{quantile=…}` summary.
- 30 workspace tests passing (added: 2 obs xrun, 2 obs metrics, 1 daemon Xrun-broadcast).

v0.3 shipped:

- `soundworm-rhai` rewritten with a `Decision`-returning API: `route(target)`, `allow()`, `deny()` builtins; scripts see `node` (map with `name`/`app`/`media_class`/`kind`/`properties`/`id`) and `sinks` (Vec<String>). Runtime cap via `set_max_operations` (100k); on overrun returns `Decision::None` and logs. Compile is atomic — a malformed script never replaces a working one.
- Policy chain: `start_event_pump` runs TOML `evaluate_node` first; on `None`, falls through to the Rhai script. TOML rules now honor `node_kind` (`Source`/`Sink`/`Filter`/`Virtual`, case-insensitive) and `property = ["key","value"]` in addition to `node_name`. `Action::SetVolume`/`Notify` still log-only.
- File watcher: `notify`-based watcher on the script's parent directory, 150 ms debounce, atomic reload via `state.reload_script()`.
- New IPC ops `LoadScript { path }` / `ReloadScript` (`docs/IPC.md` updated). CLI: `sw script load <path>` / `sw script reload`.
- Daemon loads `$XDG_CONFIG_HOME/soundworm/routing.rhai` automatically at startup if present and watches it for changes.
- `rhai` workspace dep upgraded to use the `sync` feature (required for `Engine: Send`, so `DaemonState` stays tokio-spawnable).
- 26 workspace tests passing (added: 2 policy predicate tests, 1 daemon Rhai-fallthrough test, +3 from rewritten rhai-engine).

v0.2 carry-overs that landed in v0.3:
- `RulesEngine::evaluate_rule` (legacy) preserved; new `evaluate_node` is the predicate-aware entry point used by the daemon.

Original v0.2 plumbing recap:

- IPC protocol spec at `docs/IPC.md`; `soundworm-ipc` crate holds wire
  types + tokio client + NDJSON codec (1 MiB cap).
- Daemon `swd` exposes a Unix socket at
  `$XDG_RUNTIME_DIR/soundworm/swd.sock` (override `SOUNDWORM_SOCK`).
  All proto-v1 ops implemented end-to-end: `Hello`, `ListNodes`,
  `ListLinks`, `Link`, `Unlink`, `Subscribe`/`Unsubscribe`, `LoadRules`,
  `ReloadRules`, `Snapshot`, `Restore`, `Shutdown`.
- Auto-routing dispatch wired: `start_event_pump` matches each
  `NodeAppeared` against `RulesEngine`, stages a `PendingRoute`, and
  fires `backend.create_link` once both ends have ports — emitting
  `RulesApplied` or `LinkRejected` IPC events. Only the
  `matches.node_name` predicate is honored so far; `node_kind` and
  `property` are parsed but unused. `Action::Deny` works;
  `SetVolume`/`Notify` log only.
- CLI: `sw list/link/unlink/watch/snapshot/rules/shutdown` all go
  through the daemon. `sw snapshot save/load` is daemon-backed (only
  `sw snapshot list` reads disk directly). `--in-process` escape hatch
  remains for `list/link/unlink`.
- `contrib/systemd/soundworm.service` user unit with
  `Restart=on-failure`; install to `~/.config/systemd/user/`.
- Workspace test count: 20 (incl. 2 auto-route tests in
  `crates/daemon/src/state.rs` driven by `MockBackend`).
- Carry-over into v0.3: extend `RulesEngine::evaluate_rule` to honor
  `matches.node_kind` and `matches.property`; implement
  `Action::SetVolume`/`Notify` (currently log-only).

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

## 15. TODOs / Future tracks

### 15.1 Tauri desktop UI (`soundworm-ui`)

Goal: a Loopback-style node-graph desktop app sitting on top of `swd`,
not replacing it. The daemon stays the source of truth; the UI is a
view + editor that drives it over the existing IPC socket.

**Posture decision:** the UI is a *client*, not a host. It connects to
`swd` over the unix socket like `sw` does. This keeps the headless
daemon useful, lets CLI and GUI coexist, and matches Loopback's mental
model (background service + thin UI).

**Stack:**
- Tauri 2.x shell (Rust backend, system webview frontend)
- Frontend: vanilla HTML/JS for the scaffold; pick a graph library
  (Rete.js, React Flow, Svelte Flow) once interaction needs are real
- Rust side links `soundworm-ipc` directly to talk to `swd`
- Tauri commands proxy IPC calls; Tauri events fan out `Subscribe`
  stream to the webview

**What must land in soundworm-proper before the UI is useful:**
- **Richer IPC event coverage.** The current `Subscribe` op streams
  `BackendEvent`s plus `RulesApplied`/`LinkRejected`/`XrunObserved` —
  enough for v0.1 list rendering, but the UI needs the full set of
  granular deltas so it can update live instead of polling: `NodeAdded`,
  `NodeRemoved`, `NodeChanged` (props/name), `PortAdded`/`PortRemoved`,
  `LinkAdded`/`LinkRemoved`/`LinkChanged`, `XrunFired { node, gap_ms }`,
  `LatencySample { node, ms }`. Audit `start_event_pump` and the
  pipewire-backend emitter to confirm each of these has a corresponding
  wire event; fill any gaps before binding the UI to them.
- **Stable JSON schema for the graph state.** The wire payloads serialize
  from `soundworm-core` today, but the field names have never been
  treated as a public contract. Before the UI ships, freeze the schema:
  document every field of `Node`/`Port`/`Link`/`BackendEvent` on the
  wire, generate TS types from the Rust types (`ts-rs` or `specta`), and
  add a `proto: 2` bump in `Hello` that gates the new event set. Treat
  these payloads as the daemon's public API from that point on.
- **`swd subscribe`-style endpoint / websocket bridge.** The Tauri Rust
  side can speak the unix socket directly, so the in-app path doesn't
  need a websocket. But two cases push us toward one anyway: (a) future
  in-browser dashboards or remote tooling, and (b) frontend-only dev
  loops where running the Rust shell is overkill. Plan: add an opt-in
  `swd --listen-ws 127.0.0.1:PORT` flag that bridges the same NDJSON
  protocol over a websocket frame-per-line, loopback-only, off by
  default. Same wire format, same auth model (none, localhost-only).
  Defer implementation until ui-v0.2 actually needs it.
- No new daemon *ops* required for v0.1 of the UI — `ListNodes`,
  `ListLinks`, `Link`, `Unlink`, `Subscribe`, `GetMetrics`,
  `Snapshot`/`Restore` cover the Loopback feature surface. The work
  above is event-coverage and schema-freeze, not new endpoints.

**UI roadmap (rough, not committed):**
- **ui-v0.1 (done)** — connect to socket, list nodes/ports in a sidebar,
  list links. Live updates via `Subscribe`. Proved the wiring
  end-to-end.
- **ui-v0.2 (done)** — node-graph canvas. Nodes positioned heuristically
  (sources left, sinks right, streams middle). Drag handle→handle issues
  `Link`; drag an edge endpoint reconnects (delete + create); right-click
  or select+Delete issues `Unlink`. Per-channel links collapse to one
  visual edge. Cross-kind and same-direction drags gated before they
  reach the daemon. Layout persistence to
  `$XDG_DATA_HOME/soundworm/ui-layout.json` still TODO — layout is
  recomputed each refresh.
- **ui-v0.3 (done)** — session snapshot management UI in the sidebar:
  save/restore via the daemon's `Snapshot`/`Restore` ops, list read from
  the snapshot dir on disk (as the CLI does). Restore replays links, not
  node positions (those persist separately via ui-layout.json). Shook out
  two pre-existing daemon bugs: `Script`/`Snapshot` had identical
  untagged wire shapes so snapshot save always mis-parsed as Script; and
  the link-id round-trip (`do_link` returned a placeholder 0,
  `destroy_link` keyed off the wrong id) meant `sw unlink` and UI edge
  deletes silently no-oped. Both fixed.
- **ui-v0.4** — metrics overlay: xrun badges on nodes, latency sparklines
  on links, drawn from `GetMetrics` + `XrunObserved` events.
- **ui-v0.5** — rules/script editor pane with `LoadRules`/`LoadScript`.
  Monaco or CodeMirror in the webview.

**Out of scope (parity gaps vs Loopback we will not chase):**
- Virtual audio devices (needs HAL plugin on macOS / kernel driver on
  Windows). PipeWire null-sinks could approximate this on Linux —
  separate decision.
- Per-channel routing matrix UI. PipeWire ports already expose
  channels; deferred until users ask.
- Built-in audio metering (RMS/peak). Would require a tap node in the
  graph; not a v1 concern.

**Packaging:**
- Tauri produces `.deb`/`.rpm`/`.AppImage` on Linux, `.app`/`.dmg` on
  macOS, `.msi`/`.exe` on Windows. Each is a single binary plus the
  bundled webview.
- The `soundworm-ui` crate is **not** part of the default workspace
  build — it pulls in webkit2gtk-4.1-devel on Fedora and adds minutes
  to CI. Build with `cargo build -p soundworm-ui` explicitly, or via
  the Tauri CLI (`cargo tauri dev`). CI gets a separate, optional job.

**Risks:**
- Webview footprint on Linux (webkit2gtk) — acceptable; same cost as
  every other Tauri app.
- IPC backpressure: a busy graph could flood the `Subscribe` stream
  faster than the webview can render. The daemon already bounds the
  per-connection queue (§6 of `docs/IPC.md`), so the UI just needs to
  coalesce updates per animation frame.
- Schema drift between `soundworm-core` types and the UI's TypeScript
  view models. Mitigation: generate TS types from the Rust types with
  `ts-rs` or `specta`, gated behind a UI-only feature flag.

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
