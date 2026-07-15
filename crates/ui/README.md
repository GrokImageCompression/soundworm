# soundworm-ui

Tauri 2 desktop UI for soundworm. Loopback-style node-graph canvas over
`swd`: nodes laid out by media-class, edges drawn from the live link
set, drag-to-link/reconnect/unlink editing. See DESIGN.md §15.

## Status

Node-graph canvas working (ui-v0.2). Connects to `swd` via
`soundworm-ipc`, renders nodes and edges, drives `Link`/`Unlink` from
the canvas, and updates live from backend events. Sidebar lists raw
links and a recent-event stream. Not in the default workspace build;
build explicitly. Next: snapshot management and metrics overlay.

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
  (sources left, filters middle, sinks right). Edges come from the link
  set collapsed to one visual edge per node pair, using the port→node
  mapping now embedded in the `ListNodes` payload.

## Why a separate crate, not a feature flag on `daemon`?

The Tauri toolchain pulls webkit2gtk on Linux and bundling tools on
every platform. Keeping it out of the default workspace keeps `cargo
build --workspace` fast and CI deps small. See DESIGN.md §15.
