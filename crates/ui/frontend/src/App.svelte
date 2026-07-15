<script>
  import { onMount } from "svelte";
  import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";

  import { Position } from "@xyflow/svelte";
  import {
    listNodes, listLinks, socketPath, onSwdEvent,
    createLink, deleteLink, loadLayout, saveLayout,
    listSnapshots, saveSnapshot, restoreSnapshot, getMetrics,
  } from "./swd.js";
  import { layoutNodes } from "./layout.js";
  import MetricsNode from "./MetricsNode.svelte";
  import EditorPane from "./EditorPane.svelte";

  const nodeTypes = { metrics: MetricsNode };
  const SPARK_LEN = 30; // rolling p95 samples kept per node

  let socket = $state("");
  let status = $state("connecting…");
  let nodes  = $state([]);
  let edges  = $state([]);
  let links  = $state([]);   // raw IPC links, shown in sidebar
  let events = $state([]);   // ring buffer of recent events
  let savedLayout = $state({}); // node name → {x, y}, persisted to disk
  let snapshots = $state([]);   // saved snapshot names
  let snapName  = $state("");   // save-snapshot input
  let editorOpen = $state(false); // rules/script editor overlay
  // node id → { latencyMs, xruns }, refreshed from GetMetrics. Plain
  // (not $state): read imperatively, changes land via the nodes reassign.
  let metricsById = {};
  let sparksById = {};          // node id → [p95 ms, ...] rolling buffer

  function nodeKind(media_class) {
    if (!media_class) return "unknown";
    if (media_class.includes("Source") || media_class.endsWith("Output/Audio")) return "source";
    if (media_class.includes("Sink")   || media_class.endsWith("Input/Audio"))  return "sink";
    return "filter";
  }

  // Mirror of soundworm_core::node::MediaKind so the UI can refuse
  // cross-kind drags before they reach the daemon.
  function mediaKind(media_class) {
    if (!media_class) return "other";
    const lc = media_class.toLowerCase();
    if (lc.includes("midi"))  return "midi";
    if (lc.includes("audio")) return "audio";
    if (lc.includes("video")) return "video";
    return "other";
  }

  function isValidConnection(conn) {
    // conn = { source, target, sourceHandle, targetHandle }
    const a = nodes.find((n) => n.id === conn.source);
    const b = nodes.find((n) => n.id === conn.target);
    if (!a || !b) return false;
    // No source→source or sink→sink. Same-class drags can't actually
    // produce a valid link in PipeWire (no compatible direction), so
    // gate them up front.
    if (a.sw.kind === b.sw.kind && a.sw.kind !== "filter") return false;
    // Same media kind on both ends (audio↔audio, midi↔midi).
    if (mediaKind(a.sw.media_class) !== mediaKind(b.sw.media_class)) return false;
    return true;
  }

  function toFlowNode(nv) {
    // NodeView on the wire is `{ node: Node, ports: [Port] }`.
    const n = nv.node;
    const kind = nodeKind(n.media_class);
    const m = metricsById[String(n.id)];
    return {
      id: String(n.id),
      // Custom node renders label + metrics overlay and styles itself.
      type: "metrics",
      position: { x: 0, y: 0 }, // assigned by layoutNodes()
      // Source handle on the right, target on the left, so drags read
      // left-to-right and match the 3-column layout.
      sourcePosition: Position.Right,
      targetPosition: Position.Left,
      data: {
        label: n.name,
        kind,
        xruns: m?.xruns ?? 0,
        latencyMs: m?.latencyMs ?? null,
        spark: sparksById[String(n.id)] ?? [],
      },
      // Stash the node name so onconnect can pass it back to the daemon
      // without a second lookup.
      sw: { kind, media_class: n.media_class, name: n.name, raw: n },
    };
  }

  function portIdEq(a, b) {
    // PortId serializes as a 1-tuple newtype: {"0": 42} or sometimes 42.
    const av = typeof a === "object" && a !== null ? a[0] ?? a : a;
    const bv = typeof b === "object" && b !== null ? b[0] ?? b : b;
    return String(av) === String(bv);
  }
  function portKey(p) {
    const v = typeof p === "object" && p !== null ? p[0] ?? p : p;
    return String(v);
  }

  function buildEdges(rawLinks, ports) {
    // port id → node id
    const portToNode = new Map();
    for (const p of ports) portToNode.set(portKey(p.id), portKey(p.node_id));

    const seen = new Set();
    const out = [];
    for (const l of rawLinks) {
      const srcNode = portToNode.get(portKey(l.source_port));
      const dstNode = portToNode.get(portKey(l.sink_port));
      if (!srcNode || !dstNode) continue;
      // Multiple per-channel links collapse to a single visual edge
      // between two nodes; the sidebar still lists the raw set.
      const key = `${srcNode}->${dstNode}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({
        id: `e${portKey(l.id)}`,
        source: srcNode,
        target: dstNode,
        animated: true,
        reconnectable: true,
        style: "stroke:#5aa6ff;stroke-width:1.5",
        data: { linkId: Number(portKey(l.id)) },
      });
    }
    return out;
  }

  async function refresh() {
    try {
      const [rawNodes, rawLinks] = await Promise.all([
        listNodes(), listLinks(),
      ]);
      // Ports come embedded inside each node now — flatten for the
      // port→node map used by buildEdges.
      const allPorts = rawNodes.flatMap((n) => n.ports ?? []);
      nodes = layoutNodes(rawNodes.map(toFlowNode), savedLayout);
      links = rawLinks;
      edges = buildEdges(rawLinks, allPorts);
      status = `connected — ${rawNodes.length} nodes, ${allPorts.length} ports, ${rawLinks.length} links`;
    } catch (e) {
      status = `error: ${e}`;
    }
  }

  function pushEvent(ev) {
    const ts = new Date().toLocaleTimeString();
    events = [{ ts, ...ev }, ...events].slice(0, 200);
  }

  // Poll per-node latency + xrun counts and fold them into node data so
  // the overlay (badge, p95, sparkline) updates without relaying out.
  async function refreshMetrics() {
    let m;
    try {
      m = await getMetrics();
    } catch (e) {
      return; // metrics are best-effort; don't disturb status
    }
    const next = {};
    for (const nl of m.nodes ?? []) {
      const k = portKey(nl.node_id);
      next[k] = { latencyMs: nl.p95_ms, xruns: 0 };
      const buf = sparksById[k] ?? [];
      buf.push(nl.p95_ms);
      if (buf.length > SPARK_LEN) buf.shift();
      sparksById[k] = buf;
    }
    for (const [nid, cnt] of m.xrun_by_node ?? []) {
      const k = portKey(nid);
      next[k] = { latencyMs: next[k]?.latencyMs ?? null, xruns: cnt };
    }
    metricsById = next;
    nodes = nodes.map((n) => ({
      ...n,
      data: {
        ...n.data,
        xruns: metricsById[n.id]?.xruns ?? 0,
        latencyMs: metricsById[n.id]?.latencyMs ?? null,
        spark: sparksById[n.id] ?? [],
      },
    }));
  }

  // Persist current node positions on drag-stop. Merge into savedLayout
  // so positions of nodes not currently present survive, and so the
  // next event-driven refresh keeps the dragged position instead of
  // snapping back to the heuristic column.
  function persistLayout() {
    const positions = { ...savedLayout };
    for (const n of nodes) {
      if (n.sw?.name && n.position) {
        positions[n.sw.name] = { x: n.position.x, y: n.position.y };
      }
    }
    savedLayout = positions;
    saveLayout(positions).catch((e) => { status = `layout save failed: ${e}`; });
  }

  async function refreshSnapshots() {
    try {
      snapshots = await listSnapshots();
    } catch (e) {
      status = `snapshot list failed: ${e}`;
    }
  }

  async function onSaveSnapshot() {
    const name = snapName.trim();
    if (!name) return;
    try {
      await saveSnapshot(name);
      snapName = "";
      await refreshSnapshots();
      pushEvent({ kind: "SnapshotSaved", data: { name } });
    } catch (e) {
      status = `snapshot save failed: ${e}`;
    }
  }

  async function onRestoreSnapshot(name) {
    try {
      const { applied, skipped } = await restoreSnapshot(name);
      // The daemon replays links through the backend, which emits
      // LinkAppeared/LinkRemoved; the event subscriber refreshes the
      // canvas. No manual refresh needed.
      status = `restored '${name}': ${applied} applied, ${skipped} skipped`;
      pushEvent({ kind: "SnapshotRestored", data: { name, applied, skipped } });
    } catch (e) {
      status = `snapshot restore failed: ${e}`;
    }
  }

  function nodeNameFor(id) {
    const n = nodes.find((nd) => nd.id === id);
    return n?.sw?.name ?? null;
  }

  async function onConnect(conn) {
    // conn = { source: nodeId, target: nodeId, sourceHandle, targetHandle }
    const src = nodeNameFor(conn.source);
    const dst = nodeNameFor(conn.target);
    if (!src || !dst) {
      status = `link failed: unknown node id`;
      return;
    }
    try {
      const linkId = await createLink(src, dst);
      pushEvent({ kind: "LinkCreated", data: { link_id: linkId, src, dst } });
      // No refresh here. The pipewire-backend now emits LinkAppeared
      // for user-created links (it resolves port→node from its own
      // registry map), so the event subscriber's refresh will pick up
      // the authoritative state and replace Svelte Flow's optimistic
      // edge.
    } catch (e) {
      status = `link failed: ${e}`;
      // Drop only the failing optimistic edge. A full refresh here
      // races with any earlier-successful link whose LinkAppeared
      // event hasn't been applied to the daemon's graph yet — that
      // race would wipe its optimistic edge too.
      edges = edges.filter(
        (ed) => !(
          ed.source === conn.source &&
          ed.target === conn.target &&
          ed.data?.linkId == null
        )
      );
    }
  }

  async function onDelete({ edges: del }) {
    if (!del?.length) return;
    let failed = false;
    for (const e of del) {
      const linkId = e.data?.linkId;
      if (linkId == null) continue;
      try {
        await deleteLink(linkId);
        pushEvent({ kind: "LinkDeleted", data: { link_id: linkId } });
      } catch (err) {
        status = `unlink failed: ${err}`;
        failed = true;
      }
    }
    // Same race as onConnect: LinkRemoved event will refresh on success.
    if (failed) await refresh();
  }

  // Drag an edge's source or target handle onto a different node.
  // PipeWire has no atomic "move link" primitive, so we delete the old
  // link and create a new one. The daemon will re-emit LinkRemoved +
  // LinkAppeared; the next refresh shows authoritative state.
  async function onReconnect(oldEdge, newConnection) {
    const oldLinkId = oldEdge.data?.linkId;
    const src = nodeNameFor(newConnection.source);
    const dst = nodeNameFor(newConnection.target);
    if (!src || !dst) {
      status = `reconnect failed: unknown node id`;
      return;
    }
    try {
      if (oldLinkId != null) await deleteLink(oldLinkId);
      const newId = await createLink(src, dst);
      pushEvent({ kind: "LinkReconnected", data: { old: oldLinkId, new: newId, src, dst } });
      // LinkRemoved + LinkAppeared events will drive the refresh.
    } catch (err) {
      status = `reconnect failed: ${err}`;
      await refresh();
    }
  }

  // Right-click an edge to delete. Svelte Flow doesn't expose an
  // onedgedoubleclick event, but onedgecontextmenu fires on right-
  // click and we suppress the browser's default menu.
  async function onEdgeContextMenu({ event, edge }) {
    event?.preventDefault?.();
    const linkId = edge.data?.linkId;
    if (linkId == null) return;
    try {
      await deleteLink(linkId);
      pushEvent({ kind: "LinkDeleted", data: { link_id: linkId } });
    } catch (err) {
      status = `unlink failed: ${err}`;
      await refresh();
    }
  }

  onMount(async () => {
    socket = await socketPath();
    savedLayout = await loadLayout();
    await refresh();
    await refreshSnapshots();
    await refreshMetrics();
    const unlisten = await onSwdEvent((ev) => {
      pushEvent(ev);
      const k = ev.kind;
      if (k === "NodeAppeared" || k === "NodeRemoved" ||
          k === "LinkAppeared" || k === "LinkRemoved") {
        // Port-level events don't have a kind on the wire yet (see
        // DESIGN.md §15 — richer IPC event coverage). Refreshing on
        // node/link deltas pulls fresh ports too.
        refresh();
      } else if (k === "XrunObserved") {
        refreshMetrics(); // reflect the new xrun immediately
      }
    });
    const metricsTimer = setInterval(refreshMetrics, 1000);
    return () => { clearInterval(metricsTimer); unlisten?.(); };
  });
</script>

<div class="app">
  <header>
    <h1>soundworm</h1>
    <span class="status">{status}</span>
    <span class="hint">drag handle→handle to link · drag edge endpoint to reconnect · right-click edge (or select + Delete) to unlink</span>
    <button class="edit-btn" onclick={() => (editorOpen = true)}>✎ edit config</button>
    <span class="socket">{socket}</span>
  </header>

  <div class="body">
    <div class="canvas">
      <SvelteFlow
        bind:nodes
        bind:edges
        {nodeTypes}
        fitView
        onconnect={onConnect}
        ondelete={onDelete}
        isValidConnection={isValidConnection}
        onreconnect={onReconnect}
        onedgecontextmenu={onEdgeContextMenu}
        onnodedragstop={persistLayout}
        proOptions={{ hideAttribution: true }}
      >
        <Background />
        <Controls />
        <MiniMap
          nodeColor={(n) => {
            const k = n.sw?.kind;
            if (k === "source") return "#5dd39e";
            if (k === "sink")   return "#ff6b9d";
            return "#9aa4b1";
          }}
          nodeStrokeColor="#1a1d21"
          nodeStrokeWidth={2}
          maskColor="rgba(15,17,20,0.7)"
          style="background:#2a2f36;border:1px solid #3a4049;border-radius:4px;" />
      </SvelteFlow>
    </div>

    <aside class="sidebar">
      <section>
        <h2>Snapshots ({snapshots.length})</h2>
        <form class="snap-save" onsubmit={(e) => { e.preventDefault(); onSaveSnapshot(); }}>
          <input
            type="text"
            placeholder="snapshot name"
            bind:value={snapName}
            spellcheck="false" />
          <button type="submit" disabled={!snapName.trim()}>Save</button>
        </form>
        <ul class="list">
          {#each snapshots as s}
            <li class="snap-row">
              <span class="snap-name">{s}</span>
              <button class="restore" onclick={() => onRestoreSnapshot(s)}>Restore</button>
            </li>
          {:else}
            <li class="muted">none saved</li>
          {/each}
        </ul>
      </section>
      <section>
        <h2>Links ({links.length})</h2>
        <ul class="list">
          {#each links as l}
            <li>#{portKey(l.id)}: {portKey(l.source_port)} → {portKey(l.sink_port)}</li>
          {:else}
            <li class="muted">none</li>
          {/each}
        </ul>
      </section>
      <section>
        <h2>Events</h2>
        <ul class="list events">
          {#each events as e}
            <li><span class="ts">{e.ts}</span> <b>{e.kind}</b> {JSON.stringify(e.data)}</li>
          {:else}
            <li class="muted">waiting…</li>
          {/each}
        </ul>
      </section>
    </aside>
  </div>

  {#if editorOpen}
    <div class="overlay">
      <EditorPane onclose={() => (editorOpen = false)} />
    </div>
  {/if}
</div>

<style>
  :global(:root) {
    --bg: #1a1d21;
    --panel: #23272d;
    --border: #2f343b;
    --text: #d6dae0;
    --muted: #7a818a;
    --accent: #5aa6ff;
  }
  :global(body) {
    margin: 0;
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, "Segoe UI", Roboto, system-ui, sans-serif;
  }
  .app { display: flex; flex-direction: column; height: 100vh; }
  header {
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    display: flex; align-items: baseline; gap: 14px;
  }
  header h1 { margin: 0; font-size: 15px; color: var(--accent); letter-spacing: .5px; }
  .status { font-size: 12px; }
  .hint   { font-size: 11px; color: var(--muted); }
  .edit-btn {
    margin-left: auto;
    background: var(--panel); color: var(--text);
    border: 1px solid var(--border); border-radius: 5px;
    padding: 4px 10px; font-size: 12px; cursor: pointer;
  }
  .edit-btn:hover { border-color: var(--accent); }
  .socket { font-size: 11px; color: var(--muted); margin-left: 12px; }
  .overlay {
    position: fixed; inset: 0; z-index: 20;
    background: rgba(0,0,0,0.55);
    display: flex; align-items: center; justify-content: center;
  }
  .body { flex: 1; display: grid; grid-template-columns: 1fr 320px; min-height: 0; }
  .canvas { background: var(--bg); }
  .sidebar { background: var(--panel); border-left: 1px solid var(--border); overflow-y: auto; }
  .sidebar section { padding: 10px 14px; border-bottom: 1px solid var(--border); }
  .sidebar h2 {
    margin: 0 0 6px 0; font-size: 11px; text-transform: uppercase;
    letter-spacing: .8px; color: var(--muted);
  }
  .list { list-style: none; padding: 0; margin: 0; font: 12px ui-monospace, Menlo, monospace; }
  .list li { padding: 3px 0; border-bottom: 1px solid rgba(255,255,255,0.03); }
  .events .ts { color: var(--muted); margin-right: 6px; }
  .muted { color: var(--muted); }

  .snap-save { display: flex; gap: 6px; margin-bottom: 8px; }
  .snap-save input {
    flex: 1; min-width: 0;
    background: var(--bg); color: var(--text);
    border: 1px solid var(--border); border-radius: 4px;
    padding: 4px 6px; font: 12px ui-monospace, Menlo, monospace;
  }
  .snap-save input:focus { outline: none; border-color: var(--accent); }
  .snap-save button, .snap-row .restore {
    flex-shrink: 0; white-space: nowrap;
    background: var(--accent); color: #0b1220; border: none;
    border-radius: 4px; padding: 4px 8px; font-size: 11px;
    cursor: pointer;
  }
  .snap-save button:disabled { background: var(--border); color: var(--muted); cursor: default; }
  .snap-row { display: flex; align-items: center; gap: 8px; }
  .snap-row .snap-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .snap-row .restore { background: transparent; color: var(--accent); border: 1px solid var(--border); }
  .snap-row .restore:hover { border-color: var(--accent); }

  /* Dark-theme overrides for Svelte Flow's default widgets. */
  :global(.svelte-flow__controls) {
    background: var(--panel);
    border: 1px solid var(--border);
    box-shadow: none;
  }
  :global(.svelte-flow__controls-button) {
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    color: var(--text);
    fill: var(--text);
  }
  :global(.svelte-flow__controls-button:hover) {
    background: #2c313a;
  }
  :global(.svelte-flow__controls-button svg) { fill: var(--text); }

  :global(.svelte-flow__minimap) {
    background: var(--panel);
    border: 1px solid var(--border);
  }
  :global(.svelte-flow__minimap-mask) {
    fill: rgba(0,0,0,0.55);
  }
  :global(.svelte-flow__minimap-node) {
    fill: var(--accent);
    stroke: none;
  }

  :global(.svelte-flow__attribution) { display: none; }
</style>
