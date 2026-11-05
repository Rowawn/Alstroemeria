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
  import FilePrompt from "$lib/FilePrompt.svelte";
  import ScreenHost from "$lib/ScreenHost.svelte";
  import { closeScreen, nav, screens } from "$lib/ext/screens.svelte";
  import type { ExtInfo } from "$lib/ext/types";
  import Extensions from "$lib/Extensions.svelte";
  import Languages from "$lib/Languages.svelte";
  import Library from "$lib/Library.svelte";
  import Reader from "$lib/Reader.svelte";
  import { packInbox } from "$lib/pack-inbox.svelte";
  import { sync } from "$lib/sync.svelte";
  import UpdateBanner from "$lib/UpdateBanner.svelte";
  import Toasts from "$lib/Toasts.svelte";

  let languages = $state<Language[] | null>(null);
  let tab = $state<string>("library");
  /** Screens contributed by enabled extensions (SPEC §11.1), one tab each. */
  let extScreens = $state<{ ext: ExtInfo; id: string; title: string }[]>([]);
  async function loadScreens() {
    try {
      const list = await api.extensions();
      extScreens = list.extensions
        .filter((e) => e.enabled && (!e.platforms || e.platforms.includes(list.platform)))
        .flatMap((ext) => (ext.contributes.screens ?? []).map((s) => ({ ext, id: s.id, title: s.title })));
    } catch {
      extScreens = [];
    }
  }
  const overlayExt = $derived(screens.overlay ? extScreens.find((s) => s.ext.id === screens.overlay!.extId)?.ext : undefined);
  let partId = $state<string | null>(null);
  let error = $state("");

  // "Open in reader" from an extension screen: close the overlay and show the part.
  $effect(() => {
    if (nav.part) {
      partId = nav.part.partId;
      nav.part = null;
      closeScreen();
    }
  });

  onMount(() => {
    // Core and extension events reach running extensions that may see them (SPEC §6.9.4).
    sync.start();
    void loadScreens();
    const unlisten = listen<CoreEvent>("alst:event", (e) => extHost.deliverEvent(e.payload));
    // A double-clicked `.alpack` (file association): show the Library, which opens the review screen.
    const openPack = (path: string) => {
      partId = null;
      tab = "library";
      packInbox.path = path;
    };
    const unlistenPack = listen<string>("alst:open-pack", (e) => openPack(e.payload));
    api.takePendingPack().then((p) => p && openPack(p)).catch(() => {});
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
    return () => {
      void unlisten.then((fn) => fn());
      void unlistenPack.then((fn) => fn());
    };
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
        {#each extScreens as s (s.ext.id + s.id)}
          <button class:primary={tab === `screen:${s.ext.id}:${s.id}`} onclick={() => (tab = `screen:${s.ext.id}:${s.id}`)}>{s.title}</button>
        {/each}
        <button class:primary={tab === "extensions"} onclick={() => { tab = "extensions"; void loadScreens(); }}>Extensions</button>
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
      {:else if tab.startsWith("screen:")}
        {#each extScreens.filter((s) => `screen:${s.ext.id}:${s.id}` === tab) as s (s.ext.id + s.id)}
          <ScreenHost ext={s.ext} screenId={s.id} language={languages[0]?.id ?? ""} />
        {/each}
      {:else}
        <Export {languages} />
      {/if}
    {/if}
  {/if}
</main>

{#if screens.overlay && overlayExt}
  <div class="overlay">
    <div class="row overlay-bar">
      <strong>{extScreens.find((s) => s.ext.id === screens.overlay!.extId && s.id === screens.overlay!.screenId)?.title}</strong>
      <button onclick={closeScreen} aria-label="Close">✕ Close</button>
    </div>
    {#key screens.overlay.nonce}
      <ScreenHost ext={overlayExt} screenId={screens.overlay.screenId} language={languages?.[0]?.id ?? ""} payload={screens.overlay.payload} nonce={screens.overlay.nonce} />
    {/key}
  </div>
{/if}
<FilePrompt />
<Toasts />
{#if languages}<CommandPalette {languages} />{/if}

<style>
  main {
    max-width: 60rem;
    margin: 0 auto;
    padding: 1rem;
    padding-top: max(1rem, calc(env(safe-area-inset-top) + 0.5rem));
  }
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: var(--bg);
    padding: 0.6rem;
    padding-top: max(0.6rem, env(safe-area-inset-top));
    overflow: auto;
  }
  .overlay-bar {
    justify-content: space-between;
    margin-bottom: 0.4rem;
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
