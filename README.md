# soundworm

Cross-platform audio session manager and router, written in Rust.
Primary target: Fedora / PipeWire.

Repo: https://github.com/GrokImageCompression/soundworm

## Crates

- core             Shared types: Node, Port, Link, AudioBackend trait
- graph            In-memory audio graph
- policy           TOML rules engine, conflict resolution, sessions
- rhai-engine      Scriptable routing (Rhai)
- pipewire-backend Linux PipeWire backend (primary)
- coreaudio-backend macOS CoreAudio backend (stub)
- wasapi-backend   Windows WASAPI backend (stub)
- observability    Xrun log, latency metrics
- snapshots        JSON session save/load
- cli              `sw` command-line tool
- daemon           `swd` background service

## Quick Start (Fedora)

    sudo dnf install git gcc
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    source ~/.cargo/env
    cargo build
    cargo test --workspace
    cargo run --bin swd
    cargo run --bin sw -- help

## Install systemd user service

    mkdir -p ~/.config/systemd/user
    cp contrib/systemd/soundworm.service ~/.config/systemd/user/
    systemctl --user enable --now soundworm
    systemctl --user status soundworm

## Routing Rules

Copy config/rules/default.toml to ~/.config/soundworm/rules/default.toml

Example:

    [[rules]]
    name     = "spotify-to-speakers"
    priority = 10
    [rules.matches]
    node_name = "spotify"
    [rules.action]
    Route = { target = "alsa_output.default" }

    [[rules]]
    name     = "zoom-usb-mic"
    priority = 20
    [rules.matches]
    node_name = "zoom"
    [rules.action]
    Route = { target = "alsa_input.usb_mic" }

## Routing Script (Rhai)

Copy config/scripts/routing.rhai to ~/.config/soundworm/scripts/routing.rhai

Example:

    if node_name == "spotify" || node_name == "vlc" {
        log_route(node_name, "speakers");
        allow()
    } else if node_name == "zoom" || node_name == "teams" {
        log_route(node_name, "usb_headset");
        allow()
    } else {
        deny()
    }

## CLI Reference

    sw list                      List all audio nodes
    sw link   <src> <sink>       Create a route
    sw unlink <link-id>          Remove a route
    sw snapshot save <name>      Save current session
    sw snapshot load <name>      Restore a session
    sw snapshot list             List saved sessions
    sw metrics                   Show latency and xrun stats

## Environment Variables

- RUST_LOG            Default: info. Log level (error/warn/info/debug/trace)
- XDG_CONFIG_HOME     Default: ~/.config. Config directory
- SOUNDWORM_BACKEND   Default: pipewire. Backend override

## License

MIT OR Apache-2.0
