// Thin wrapper over the Tauri commands exposed by crates/ui/src/main.rs.
// Frontend code never reaches into `window.__TAURI__` directly.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const listNodes  = () => invoke("list_nodes");
export const listPorts  = () => invoke("list_ports");
export const listLinks  = () => invoke("list_links");
export const socketPath = () => invoke("socket_path");

export const createLink = (sourceNode, targetNode) =>
  invoke("create_link", { sourceNode, targetNode });
export const deleteLink = (linkId) =>
  invoke("delete_link", { linkId });

// Canvas node positions, keyed by node name, persisted to
// $XDG_DATA_HOME/soundworm/ui-layout.json by the Rust side.
export const loadLayout = () => invoke("load_layout");
export const saveLayout = (positions) => invoke("save_layout", { positions });

// Session snapshots. Save/restore go through the daemon; list reads the
// snapshot dir directly, same as the CLI.
export const listSnapshots   = () => invoke("list_snapshots");
export const saveSnapshot    = (name) => invoke("save_snapshot", { name });
export const restoreSnapshot = (name) => invoke("restore_snapshot", { name });

// Per-node latency percentiles + xrun counts. Polled for the overlay.
export const getMetrics = () => invoke("get_metrics");

export function onSwdEvent(handler) {
  return listen("swd-event", (msg) => handler(msg.payload));
}
