# soundworm-ui

Tauri 2 desktop UI for soundworm. v0.1 scaffold — sidebar list of
nodes/links plus a live event stream from `swd`. Loopback-style
node-graph canvas is the next milestone (see DESIGN.md §15).

## Status

Scaffold only. Connects to `swd` via `soundworm-ipc`, lists nodes/links
on startup, subscribes to backend events and logs them. Not in the
default workspace build; build explicitly.

## Build deps (Fedora)

    sudo dnf install webkit2gtk4.1-devel gtk3-devel \
                     libsoup3-devel javascriptcoregtk4.1-devel \
                     librsvg2-devel

Plus Cargo and the Tauri CLI:

    cargo install tauri-cli --version '^2.0'

## Frontend deps

Node 22+ and npm. From `crates/ui/frontend/`:

    npm install
    npm run build         # produces frontend/dist consumed by Tauri release builds

`cargo tauri dev` runs `npm run dev` for you via `beforeDevCommand`.

## Run

Start the daemon in one terminal:

    RUST_LOG=info cargo run --bin swd

Then from `crates/ui/`:

    cargo tauri dev

Or, after `npm run build`, build the binary directly:

    cargo build -p soundworm-ui --manifest-path crates/ui/Cargo.toml

## Architecture

- Tauri Rust shell links `soundworm-ipc` directly.
- One persistent `Client` (for request/response ops) owned by `AppState`.
- A second connection runs `connect_subscriber` and re-emits every
  `BackendEvent` to the webview via `app.emit("swd-event", ...)`.
- Frontend is Vite + Svelte 5 + [Svelte Flow](https://svelteflow.dev)
  (`@xyflow/svelte`). Canvas renders nodes positioned by media-class
  (sources left, filters middle, sinks right). Edges between nodes are
  deferred until the IPC wire exposes port→node mapping; see
  DESIGN.md §15.

## Why a separate crate, not a feature flag on `daemon`?

The Tauri toolchain pulls webkit2gtk on Linux and bundling tools on
every platform. Keeping it out of the default workspace keeps `cargo
build --workspace` fast and CI deps small. See DESIGN.md §15.
