<script lang="ts">
  import LangData from "./LangData.svelte";
  import { needsLangData } from "./langdata-state.svelte";
  import { onMount } from "svelte";
  import { sync } from "./sync.svelte";
  import { api, type Language, type PackInfo, type PackOpened, type PartInfo, type Work } from "./api";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { errorText } from "./langdata-state.svelte";
  import { packInbox } from "./pack-inbox.svelte";
  import MediaImport from "./MediaImport.svelte";
  import PackReview from "./PackReview.svelte";
  import Podcast from "./Podcast.svelte";
  import YouTube from "./YouTube.svelte";

  let { languages, onOpen }: { languages: Language[]; onOpen: (partId: string) => void } = $props();

  let works = $state<Work[]>([]);
  let expanded = $state<string | null>(null);
  let parts = $state<PartInfo[]>([]);

  let tab = $state<"text" | "epub" | "url" | "media" | "podcast" | "youtube">("text");
  let ytdlp = $state(false); // YouTube tab only when yt-dlp is installed (desktop)
  let language = $state(languages[0]?.id ?? "");
  let title = $state("");
  let text = $state("");
  let url = $state("");
  let file = $state<File | null>(null);
  let busy = $state(false);
  let error = $state("");
  let langdataId = $state("");

  // Content packs (SPEC §10): the review screen, the pack filter and the pack view.
  let packs = $state<PackInfo[]>([]);
  let packFilter = $state(""); // "" all, "none" works without a pack, or a pack id
  let opened = $state<PackOpened | null>(null);
  let packError = $state("");
  let packBusy = $state(false);
  let dragging = $state(false);

  const shown = $derived(packFilter === "" ? works : works.filter((w) => (packFilter === "none" ? !w.packId : w.packId === packFilter)));
  const activePack = $derived(packs.find((p) => p.id === packFilter) ?? null);

  async function refresh() {
    works = await api.works();
    packs = await api.packs().catch(() => []);
  }

  async function openPack(open: () => Promise<PackOpened>) {
    packError = "";
    packBusy = true;
    try {
      if (opened) await api.packClose(opened.handle).catch(() => {});
      opened = await open();
    } catch (e) {
      opened = null;
      packError = errorText(e);
    } finally {
      packBusy = false;
    }
  }
  const openPackFile = (f: File) => openPack(() => api.packOpenFile(f));
  const openPackPath = (path: string) => openPack(() => api.packOpenPath(path));

  function pickPack(e: Event & { currentTarget: HTMLInputElement }) {
    const f = e.currentTarget.files?.[0];
    e.currentTarget.value = "";
    if (f) void openPackFile(f);
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    const f = [...(e.dataTransfer?.files ?? [])].find((f) => f.name.toLowerCase().endsWith(".alpack"));
    if (f) void openPackFile(f);
  }

  $effect(() => {
    const p = packInbox.path;
    if (p) {
      packInbox.path = null;
      void openPackPath(p);
    }
  });

  // The desktop webview delivers dropped files as paths through its own event.
  onMount(() => {
    let off: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const t = e.payload.type;
        if (t === "enter" || t === "over") dragging = true;
        else if (t === "leave") dragging = false;
        else if (t === "drop") {
          dragging = false;
          const path = e.payload.paths.find((p) => p.toLowerCase().endsWith(".alpack"));
          if (path) void openPackPath(path);
        }
      })
      .then((f) => (off = f))
      .catch(() => {});
    return () => off?.();
  });

  async function open(w: Work) {
    const ps = await api.parts(w.id);
    if (ps.length === 1) return onOpen(ps[0].id);
    parts = ps;
    expanded = expanded === w.id ? null : w.id;
  }

  async function add(event: Event) {
    event.preventDefault();
    error = "";
    langdataId = "";
    busy = true;
    try {
      if (tab === "text") await api.createWork(language, title, text);
      else if (tab === "epub") {
        if (!file) throw "Choose an .epub file";
        await api.importEpub(language, file);
      } else await api.importUrl(language, url);
      title = text = url = "";
      file = null;
      await refresh();
    } catch (e) {
      error = String(e);
      langdataId = needsLangData(e) ?? "";
    } finally {
      busy = false;
    }
  }

  let seenTick = sync.tick;
  $effect(() => {
    const t = sync.tick;
    if (t !== seenTick) {
      seenTick = t;
      void refresh();
    }
  });

  onMount(() => {
    refresh();
    api.ytdlpStatus().then((s) => (ytdlp = s.available)).catch(() => {});
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="drop" class:dragging ondragover={(e) => { e.preventDefault(); dragging = true; }} ondragleave={() => (dragging = false)} ondrop={onDrop}>
<div class="row head">
  <h1>Library</h1>
  <label class="button">
    {packBusy ? "Opening…" : "Import pack…"}
    <input type="file" hidden onchange={pickPack} disabled={packBusy} />
  </label>
</div>
{#if packError}<p class="error">{packError}</p>{/if}
{#if opened}
  {#key opened.handle}
    <PackReview {opened} onChanged={refresh} onClose={() => (opened = null)} />
  {/key}
{/if}

{#if packs.length > 0}
  <label class="row">
    Show
    <select bind:value={packFilter}>
      <option value="">All works</option>
      <option value="none">Not from a pack</option>
      {#each packs as p (p.id)}<option value={p.id}>Pack: {p.title}</option>{/each}
    </select>
  </label>
{/if}
{#if activePack}
  <section class="packview">
    <strong>{activePack.title}</strong>
    <span class="muted">{activePack.language}{activePack.level ? ` · ${activePack.level}` : ""}{activePack.topic ? ` · ${activePack.topic}` : ""} · {shown.length} of {activePack.itemCount} items imported here</span>
    {#if activePack.description}<p>{activePack.description}</p>{/if}
    <ul class="attrib">
      {#each shown as w (w.id)}
        <li>
          {w.title}
          <span class="muted">— {w.attribution ?? "no attribution"}{w.sourceLicense ? ` · ${w.sourceLicense}` : ""}</span>
          {#if w.sourceUrl}<button class="link" onclick={() => api.openUrl(w.sourceUrl!)}>source</button>{/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}

{#if shown.length === 0 && works.length > 0}
  <p class="muted">No works match this filter.</p>
{:else if works.length === 0}
  <p class="muted">Nothing here yet. Add a text, an EPUB, a web article, or audio/video with subtitles below.</p>
{:else}
  <ul class="works">
    {#each shown as w (w.id)}
      <li>
        <button class="work" onclick={() => open(w)}>
          <strong>{w.title}</strong>
          <span class="muted">
            {w.language} · {w.kind}{w.author ? ` · ${w.author}` : ""}{w.partCount > 1 ? ` · ${w.partCount} parts` : ""}
          </span>
        </button>
        {#if expanded === w.id}
          <ol class="parts">
            {#each parts as p (p.id)}
              <li>
                <button onclick={() => onOpen(p.id)}>{p.title ?? `Part ${p.idx + 1}`}</button>
                <span class="muted">{p.wordCount} words</span>
              </li>
            {/each}
          </ol>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<h2>Add</h2>
<div class="row tabs">
  <button class:primary={tab === "text"} onclick={() => (tab = "text")}>Text</button>
  <button class:primary={tab === "epub"} onclick={() => (tab = "epub")}>EPUB</button>
  <button class:primary={tab === "url"} onclick={() => (tab = "url")}>Web article</button>
  <button class:primary={tab === "media"} onclick={() => (tab = "media")}>Audio / video</button>
  <button class:primary={tab === "podcast"} onclick={() => (tab = "podcast")}>Podcast</button>
  {#if ytdlp}<button class:primary={tab === "youtube"} onclick={() => (tab = "youtube")}>YouTube</button>{/if}
</div>
<label class="row">
  Language
  <select bind:value={language}>
    {#each languages as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
  </select>
</label>
{#if tab === "media"}
  <MediaImport {language} onDone={refresh} />
{:else if tab === "podcast"}
  <Podcast {language} onDone={refresh} />
{:else if tab === "youtube"}
  <YouTube {language} onDone={refresh} />
{:else}
<form onsubmit={add}>
  {#if tab === "text"}
    <input placeholder="Title" bind:value={title} required />
    <textarea placeholder="Paste text" rows="8" bind:value={text} required></textarea>
  {:else if tab === "epub"}
    <input
      type="file"
      accept=".epub,application/epub+zip"
      onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)}
    />
  {:else}
    <input type="url" placeholder="https://…" bind:value={url} required />
  {/if}
  <button class="primary" type="submit" disabled={busy}>{busy ? "Importing…" : "Add"}</button>
  {#if langdataId}<LangData id={langdataId} />{:else if error}<p class="error">{error}</p>{/if}
</form>
{/if}
</div>

<style>
  .drop {
    border: 2px dashed transparent;
    border-radius: 10px;
  }
  .drop.dragging {
    border-color: var(--accent);
  }
  .head {
    align-items: center;
    justify-content: space-between;
  }
  .button {
    cursor: pointer;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.3rem 0.7rem;
  }
  .packview {
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0.6rem 0.8rem;
    margin: 0.5rem 0 0.8rem;
  }
  .attrib {
    margin: 0.3rem 0 0;
    padding-left: 1.2rem;
    font-size: 0.9rem;
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    padding: 0 0.3rem;
    text-decoration: underline;
    cursor: pointer;
  }
  .works,
  .parts {
    list-style: none;
    padding: 0;
    margin: 0 0 1rem;
  }
  .parts {
    padding-left: 1.5rem;
  }
  .work {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    width: 100%;
    text-align: left;
    margin-bottom: 0.4rem;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
  .tabs {
    margin-bottom: 0.4rem;
    flex-wrap: wrap;
  }
</style>
