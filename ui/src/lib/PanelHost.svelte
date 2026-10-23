<script lang="ts">
  import { onMount } from "svelte";
  import { extHost, extUrl } from "./ext";
  import { themeVariables } from "./ext/render";
  import type { ExtInfo } from "./ext/types";

  // Sandboxed iframe for a panel (SPEC §6.9.7). The frame has an opaque origin and no network; it talks to this
  // host with postMessage, and the host relays to the extension's worker or to permitted `alst.*` calls.
  let { ext, panelId, language, refresh, onClose }: {
    ext: ExtInfo;
    panelId: string;
    language: string;
    refresh: number;
    onClose: () => void;
  } = $props();

  let frame: HTMLIFrameElement | undefined = $state();
  const panel = $derived(ext.contributes.panels.find((p) => p.id === panelId));
  const src = $derived(panel ? extUrl(ext.id, panel.entry) : "");

  const post = (msg: unknown) => frame?.contentWindow?.postMessage(msg, "*");

  async function onMessage(e: MessageEvent) {
    // Only this iframe: its origin is opaque, so identify it by window, never by origin.
    if (!frame || e.source !== frame.contentWindow) return;
    const m = e.data as { type?: string; id?: number; method?: string; params?: unknown };
    if (!m || typeof m !== "object") return;
    if (m.type === "alst-ready") {
      post({ type: "alst-theme", cssVariables: themeVariables() });
      return;
    }
    if (m.type !== "alst-call" || typeof m.id !== "number" || typeof m.method !== "string") return;
    try {
      const result =
        m.method === "panel.message"
          ? await extHost.panelMessage(ext, language, panelId, m.params)
          : await extHost.serveApi(ext, `alst.${m.method}`, m.params);
      post({ type: "alst-result", id: m.id, result: result ?? null });
    } catch (err) {
      post({ type: "alst-result", id: m.id, error: { message: err instanceof Error ? err.message : String(err) } });
    }
  }

  onMount(() => {
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  });

  // Tell the panel when the reader's data changed so it can re-ask the extension.
  $effect(() => {
    void refresh;
    post({ type: "alst-event", name: "refresh" });
  });
</script>

<div class="head row">
  <strong>{panel?.title ?? panelId}</strong>
  <span class="muted">{ext.name}</span>
  <button onclick={onClose} aria-label="Close panel">✕</button>
</div>
{#if panel}
  <iframe bind:this={frame} title={panel.title} {src} sandbox="allow-scripts"></iframe>
{:else}
  <p class="error">Panel '{panelId}' not found.</p>
{/if}

<style>
  .head {
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  iframe {
    width: 100%;
    height: 22rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--panel);
  }
</style>
