<script lang="ts">
  import { onMount } from "svelte";
  import { extHost, extUrl } from "./ext";
  import { themeVariables } from "./ext/render";
  import type { ExtInfo } from "./ext/types";
  import { mediaUrl } from "./api";

  // Full-size sandboxed iframe for an extension screen (SPEC §11.1), same protocol as panels (§6.9.7): opaque origin, no
  // network, calls relayed by this host. `payload` is sent when the frame is ready and again when it changes (`open` event).
  let { ext, screenId, language, payload = null, nonce = 0 }: {
    ext: ExtInfo;
    screenId: string;
    language: string;
    payload?: unknown;
    nonce?: number;
  } = $props();

  let frame: HTMLIFrameElement | undefined = $state();
  let ready = false;
  const screen = $derived(ext.contributes.screens.find((s) => s.id === screenId));
  const src = $derived(screen ? extUrl(ext.id, screen.entry) : "");

  const post = (msg: unknown) => frame?.contentWindow?.postMessage(msg, "*");
  const sendOpen = () => post({ type: "alst-event", name: "open", payload });

  async function onMessage(e: MessageEvent) {
    if (!frame || e.source !== frame.contentWindow) return;
    const m = e.data as { type?: string; id?: number; method?: string; params?: unknown };
    if (!m || typeof m !== "object") return;
    if (m.type === "alst-ready") {
      ready = true;
      post({ type: "alst-theme", cssVariables: themeVariables(), mediaBase: mediaUrl("").replace(/\/$/, "") });
      if (payload != null) sendOpen();
      return;
    }
    if (m.type !== "alst-call" || typeof m.id !== "number" || typeof m.method !== "string") return;
    try {
      const result =
        m.method === "panel.message"
          ? await extHost.panelMessage(ext, language, screenId, m.params)
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

  $effect(() => {
    void nonce;
    if (ready && payload != null) sendOpen();
  });
</script>

{#if screen}
  <iframe bind:this={frame} title={screen.title} {src} sandbox="allow-scripts" allow="autoplay"></iframe>
{:else}
  <p class="error">Screen '{screenId}' not found.</p>
{/if}

<style>
  iframe {
    width: 100%;
    height: calc(100dvh - 5.5rem);
    min-height: 24rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--panel);
  }
</style>
