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

## 14. Roadmap

- v0.1  PipeWire enumeration, manual link/unlink, snapshots
- v0.2  TOML rules engine, daemon, systemd unit
- v0.3  Rhai scripting, hot reload
- v0.4  Metrics, xrun detection
- v0.5  CoreAudio backend
- v0.6  WASAPI backend
- v1.0  Stable API, semver guarantees
