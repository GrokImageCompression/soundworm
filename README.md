# soundworm

Cross-platform audio session manager and router — written in Rust.
Primary target: **Fedora / PipeWire**

https://github.com/GrokImageCompression/soundworm

## Crates

| Crate | Purpose |
|---|---|
| core | Shared types: Node, Port, Link, AudioBackend trait |
| graph | In-memory audio graph |
| policy | TOML rules engine, conflict resolution, sessions |
| rhai-engine | Scriptable routing (Rhai) |
| pipewire-backend | Linux PipeWire (primary) |
| coreaudio-backend | macOS CoreAudio (stub) |
| wasapi-backend | Windows WASAPI (stub) |
| observability | Xrun log, latency metrics |
| snapshots | JSON session save/load |
| cli | `sw` command-line tool |
| daemon | `swd` background service |

## Quick Start (Fedora)

```bash
sudo dnf install git gcc
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

cargo build
cargo test --workspace
cargo run --bin swd          # daemon
cargo run --bin sw -- help   # CLI

