<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { api, type Language } from "$lib/api";
  import About from "$lib/About.svelte";
  import CommandPalette from "$lib/CommandPalette.svelte";
  import Devices from "$lib/Devices.svelte";
  import Dictionaries from "$lib/Dictionaries.svelte";
  import { extHost } from "$lib/ext";
  import type { CoreEvent } from "$lib/ext/types";
  import Export from "$lib/Export.svelte";
  import Extensions from "$lib/Extensions.svelte";
  import Languages from "$lib/Languages.svelte";
  import Library from "$lib/Library.svelte";
  import Reader from "$lib/Reader.svelte";
  import { sync } from "$lib/sync.svelte";
  import UpdateBanner from "$lib/UpdateBanner.svelte";
  import Toasts from "$lib/Toasts.svelte";

  let languages = $state<Language[] | null>(null);
  let tab = $state<"library" | "dictionaries" | "languages" | "export" | "extensions" | "devices" | "about">("library");
  let partId = $state<string | null>(null);
  let error = $state("");

  onMount(() => {
    // Core and extension events reach running extensions that may see them (SPEC §6.9.4).
    sync.start();
    const unlisten = listen<CoreEvent>("alst:event", (e) => extHost.deliverEvent(e.payload));
    api
      .languages()
      .then(async (l) => {
        languages = l;
        // Developer-only: on-device verification of the extension host (see selftest.ts).
        const selftest = await invoke<string | null>("selftest_enabled").catch(() => null);
        if (selftest?.startsWith("cjk")) {
          (await import("$lib/selftest-cjk")).run(l, selftest, { openPart: (id) => (partId = id), closePart: () => (partId = null) });
        } else if (selftest?.startsWith("media")) {
          (await import("$lib/selftest-media")).run(l, selftest, { openPart: (id) => (partId = id), closePart: () => (partId = null) });
        } else if (selftest !== null) {
          (await import("$lib/ext/selftest")).run(l, selftest, {
            openPart: (id) => (partId = id),
            showExtensions: () => (tab = "extensions"),
          });
        }
      })
      .catch((e) => (error = String(e)));
    return () => void unlisten.then((fn) => fn());
  });
</script>

<main>
  <UpdateBanner />
  {#if error}
    <p class="error">{error}</p>
  {:else if languages}
    {#if partId}
      {#key partId}
        <Reader {partId} {languages} onOpen={(id) => (partId = id)} onBack={() => (partId = null)} />
      {/key}
    {:else}
      <nav class="row">
        <button class:primary={tab === "library"} onclick={() => (tab = "library")}>Library</button>
        <button class:primary={tab === "dictionaries"} onclick={() => (tab = "dictionaries")}>Dictionaries</button>
        <button class:primary={tab === "languages"} onclick={() => (tab = "languages")}>Languages</button>
        <button class:primary={tab === "export"} onclick={() => (tab = "export")}>Export</button>
        <button class:primary={tab === "extensions"} onclick={() => (tab = "extensions")}>Extensions</button>
        <button class:primary={tab === "devices"} onclick={() => (tab = "devices")}>Devices</button>
        <button class:primary={tab === "about"} onclick={() => (tab = "about")}>About</button>
        {#if sync.available && sync.overview && sync.overview.peers.length > 0}
          <button
            class="sync {sync.overview.status.state}"
            title={sync.overview.status.message ?? (sync.overview.status.state === "syncing" ? "Syncing…" : "Sync is up to date")}
            onclick={() => (tab = "devices")}
            aria-label="Sync status: {sync.overview.status.state}"
          >
            <span class="dot"></span>
            {sync.overview.status.state === "syncing" ? "Syncing…" : sync.overview.status.state === "error" ? "Sync error" : "Synced"}
          </button>
        {/if}
      </nav>
      {#if tab === "library"}
        <Library {languages} onOpen={(id) => (partId = id)} />
      {:else if tab === "dictionaries"}
        <Dictionaries {languages} />
      {:else if tab === "languages"}
        <Languages />
      {:else if tab === "extensions"}
        <Extensions {languages} />
      {:else if tab === "devices"}
        <Devices />
      {:else if tab === "about"}
        <About />
      {:else}
        <Export {languages} />
      {/if}
    {/if}
  {/if}
</main>

<Toasts />
{#if languages}<CommandPalette {languages} />{/if}

<style>
  main {
    max-width: 60rem;
    margin: 0 auto;
    padding: 1rem;
    padding-top: max(1rem, calc(env(safe-area-inset-top) + 0.5rem));
  }
  nav {
    margin-bottom: 0.5rem;
    flex-wrap: wrap;
  }
  .sync {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
  }
  .dot {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 50%;
    background: var(--role-adjective);
  }
  .sync.syncing .dot {
    background: var(--accent);
    animation: pulse 1s infinite;
  }
  .sync.error .dot {
    background: var(--role-warn);
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
</style>
