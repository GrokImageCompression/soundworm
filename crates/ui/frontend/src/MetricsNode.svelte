<script>
  import { Handle, Position } from "@xyflow/svelte";

  // data: { label, kind, xruns, latencyMs, spark:[ms,...] }
  let { data } = $props();

  const palette = {
    source: { bg: "#1f3a2d", fg: "#a6f0c4", line: "#5dd39e" },
    sink:   { bg: "#3a1f2d", fg: "#f0a6c4", line: "#ff6b9d" },
    filter: { bg: "#23272d", fg: "#d6dae0", line: "#9aa4b1" },
  };
  let c = $derived(palette[data.kind] ?? palette.filter);

  // Sparkline points scaled to an 80x18 box, newest on the right.
  let spark = $derived(data.spark ?? []);
  let points = $derived.by(() => {
    if (spark.length < 2) return "";
    const w = 80, h = 18, max = Math.max(...spark, 0.001);
    const step = w / (spark.length - 1);
    return spark
      .map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * h).toFixed(1)}`)
      .join(" ");
  });
</script>

<div class="mnode" style="background:{c.bg};color:{c.fg};">
  <Handle type="target" position={Position.Left} />
  <div class="label">{data.label}</div>
  <div class="stats">
    {#if data.latencyMs != null}
      <span class="lat">p95 {data.latencyMs.toFixed(1)}ms</span>
    {/if}
    {#if points}
      <svg class="spark" viewBox="0 0 80 18" preserveAspectRatio="none">
        <polyline points={points} fill="none" stroke={c.line} stroke-width="1" />
      </svg>
    {/if}
  </div>
  {#if data.xruns > 0}
    <span class="xrun" title="{data.xruns} xruns">⚠ {data.xruns}</span>
  {/if}
  <Handle type="source" position={Position.Right} />
</div>

<style>
  .mnode {
    position: relative;
    min-width: 150px;
    padding: 8px 12px;
    border: 1px solid #2f343b;
    border-radius: 6px;
    font-size: 12px;
    font-family: ui-monospace, Menlo, monospace;
  }
  .label { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .stats {
    display: flex; align-items: center; gap: 8px;
    margin-top: 4px; height: 18px;
  }
  .lat { font-size: 10px; opacity: 0.8; }
  .spark { width: 80px; height: 18px; }
  .xrun {
    position: absolute; top: -8px; right: -8px;
    background: #d33; color: #fff;
    font-size: 10px; font-weight: 600;
    padding: 1px 5px; border-radius: 8px;
    border: 1px solid #1a1d21;
  }
</style>
