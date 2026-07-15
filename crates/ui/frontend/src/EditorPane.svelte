<script>
  import { onMount } from "svelte";
  import { EditorView, basicSetup } from "codemirror";
  import { EditorState } from "@codemirror/state";
  import { StreamLanguage } from "@codemirror/language";
  import { toml } from "@codemirror/legacy-modes/mode/toml";
  import { rust } from "@codemirror/legacy-modes/mode/rust";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { readConfig, writeConfig, applyRules, applyScript } from "./swd.js";

  let { onclose } = $props();

  let kind = $state("rules"); // "rules" (TOML) | "script" (rhai)
  let msg = $state("");
  let busy = $state(false);
  let host;   // mount point for CodeMirror
  let view;   // EditorView

  // rhai has no dedicated CM mode; it's Rust/C-like, so the rust mode
  // highlights it close enough.
  const langFor = (k) => StreamLanguage.define(k === "rules" ? toml : rust);

  async function load(k) {
    const content = await readConfig(k);
    const state = EditorState.create({
      doc: content,
      extensions: [basicSetup, langFor(k), oneDark, EditorView.lineWrapping],
    });
    if (view) view.setState(state);
    else view = new EditorView({ state, parent: host });
  }

  async function switchKind(k) {
    if (k === kind) return;
    kind = k;
    msg = "";
    await load(k);
  }

  async function apply() {
    busy = true;
    msg = "";
    try {
      await writeConfig(kind, view.state.doc.toString());
      if (kind === "rules") {
        const n = await applyRules();
        msg = `${n} rule${n === 1 ? "" : "s"} loaded`;
      } else {
        await applyScript();
        msg = "script loaded";
      }
    } catch (e) {
      // Daemon rejects a malformed TOML/rhai and keeps the previous one.
      msg = `error: ${e}`;
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    load(kind);
    return () => view?.destroy();
  });
</script>

<div class="editor">
  <header>
    <div class="tabs">
      <button class:active={kind === "rules"} onclick={() => switchKind("rules")}>rules.toml</button>
      <button class:active={kind === "script"} onclick={() => switchKind("script")}>routing.rhai</button>
    </div>
    <span class="msg" class:err={msg.startsWith("error")}>{msg}</span>
    <button class="apply" onclick={apply} disabled={busy}>Apply</button>
    <button class="close" onclick={onclose} aria-label="close">✕</button>
  </header>
  <div class="cm" bind:this={host}></div>
</div>

<style>
  .editor {
    display: flex; flex-direction: column;
    width: min(760px, 90vw); height: min(560px, 82vh);
    background: #1a1d21; border: 1px solid #2f343b; border-radius: 8px;
    box-shadow: 0 12px 40px rgba(0,0,0,0.5); overflow: hidden;
  }
  header {
    display: flex; align-items: center; gap: 10px;
    padding: 8px 10px; border-bottom: 1px solid #2f343b;
  }
  .tabs { display: flex; gap: 4px; }
  .tabs button {
    background: transparent; color: #7a818a; border: 1px solid transparent;
    border-radius: 5px; padding: 4px 10px; font-size: 12px;
    font-family: ui-monospace, Menlo, monospace; cursor: pointer;
  }
  .tabs button.active { background: #23272d; color: #d6dae0; border-color: #2f343b; }
  .msg { flex: 1; font-size: 11px; color: #7a818a; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .msg.err { color: #ff8080; }
  .apply {
    background: #5aa6ff; color: #0b1220; border: none; border-radius: 5px;
    padding: 5px 12px; font-size: 12px; font-weight: 600; cursor: pointer;
  }
  .apply:disabled { opacity: 0.5; cursor: default; }
  .close {
    background: transparent; color: #7a818a; border: none;
    font-size: 14px; cursor: pointer; padding: 4px 6px;
  }
  .cm { flex: 1; min-height: 0; overflow: auto; }
  .cm :global(.cm-editor) { height: 100%; }
</style>
