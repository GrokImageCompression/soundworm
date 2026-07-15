//! Tauri shell for soundworm. Talks to `swd` over the existing NDJSON
//! IPC socket via `soundworm-ipc`. Exposes the list/link ops as Tauri
//! commands and streams live `swd` events to the webview, which renders
//! the node-graph canvas.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use soundworm_core::link::LinkId;
use soundworm_ipc::{
    client::{connect_subscriber, Client},
    default_socket_path, Event, Op, PortRef, ResponseData,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

struct AppState {
    client: Mutex<Option<Client>>,
}

#[tauri::command]
async fn list_nodes(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
    match client.request(Op::ListNodes).await.map_err(|e| e.to_string())? {
        ResponseData::Nodes { nodes } => {
            serde_json::to_value(nodes).map_err(|e| e.to_string())
        }
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[tauri::command]
async fn list_ports(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
    match client.request(Op::ListPorts).await.map_err(|e| e.to_string())? {
        ResponseData::Ports { ports } => {
            serde_json::to_value(ports).map_err(|e| e.to_string())
        }
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[tauri::command]
async fn list_links(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
    match client.request(Op::ListLinks).await.map_err(|e| e.to_string())? {
        ResponseData::Links { links } => {
            serde_json::to_value(links).map_err(|e| e.to_string())
        }
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[tauri::command]
async fn socket_path() -> String {
    default_socket_path().display().to_string()
}

// Mirrors soundworm_snapshots::snapshot_dir's XDG convention. Not shared
// via that crate: the UI is deliberately decoupled from the daemon-side
// workspace, and the layout file is a UI-only concern the daemon never
// reads.
fn layout_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        });
    base.join("soundworm/ui-layout.json")
}

#[tauri::command]
async fn load_layout() -> Result<serde_json::Value, String> {
    match std::fs::read_to_string(layout_path()) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn save_layout(positions: serde_json::Value) -> Result<(), String> {
    let path = layout_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let s = serde_json::to_string(&positions).map_err(|e| e.to_string())?;
    std::fs::write(&path, s).map_err(|e| e.to_string())
}

#[tauri::command]
async fn create_link(
    source_node: String,
    target_node: String,
    state: State<'_, Arc<AppState>>,
) -> Result<u64, String> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
    let op = Op::Link {
        source: PortRef::Named { node: source_node, port: String::new() },
        sink:   PortRef::Named { node: target_node, port: String::new() },
    };
    match client.request(op).await.map_err(|e| e.to_string())? {
        ResponseData::Link { link_id } => Ok(link_id.0),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[tauri::command]
async fn delete_link(
    link_id: u64,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
    let op = Op::Unlink { link_id: LinkId(link_id) };
    client.request(op).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn run_event_pump(app: AppHandle) -> Result<()> {
    let path = default_socket_path();
    let mut rx = connect_subscriber(&path, None).await?;
    while let Some(ev) = rx.recv().await {
        let kind = event_kind(&ev);
        let _ = app.emit("swd-event", serde_json::json!({
            "kind": kind,
            "data": ev,
        }));
    }
    Ok(())
}

fn event_kind(ev: &Event) -> &'static str {
    match ev {
        Event::NodeAppeared { .. }  => "NodeAppeared",
        Event::NodeRemoved { .. }   => "NodeRemoved",
        Event::LinkAppeared { .. }  => "LinkAppeared",
        Event::LinkRemoved { .. }   => "LinkRemoved",
        Event::RulesApplied { .. }  => "RulesApplied",
        Event::LinkRejected { .. }  => "LinkRejected",
        Event::EventsDropped { .. } => "EventsDropped",
        Event::XrunObserved { .. }  => "XrunObserved",
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "soundworm_ui=info,warn".into()),
        )
        .init();

    let state = Arc::new(AppState {
        client: Mutex::new(None),
    });

    tauri::Builder::default()
        .manage(state.clone())
        .setup(move |app| {
            let handle = app.handle().clone();
            let state = state.clone();
            tauri::async_runtime::spawn(async move {
                match Client::connect(&default_socket_path()).await {
                    Ok(c) => {
                        *state.client.lock().await = Some(c);
                        tracing::info!("connected to swd");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to connect to swd; start it with `cargo run --bin swd`");
                    }
                }
                if let Err(e) = run_event_pump(handle).await {
                    tracing::warn!(error = %e, "event pump exited");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_nodes, list_ports, list_links, socket_path,
            create_link, delete_link, load_layout, save_layout,
        ])
        .run(tauri::generate_context!())
        .expect("failed to launch soundworm-ui");
}
