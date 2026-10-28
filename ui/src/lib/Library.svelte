<script lang="ts">
  import LangData from "./LangData.svelte";
  import { needsLangData } from "./langdata-state.svelte";
  import { onMount } from "svelte";
  import { sync } from "./sync.svelte";
  import { api, type Language, type PartInfo, type Work } from "./api";
  import MediaImport from "./MediaImport.svelte";
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

  async function refresh() {
    works = await api.works();
  }

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

<h1>Library</h1>

{#if works.length === 0}
  <p class="muted">Nothing here yet. Add a text, an EPUB, a web article, or audio/video with subtitles below.</p>
{:else}
  <ul class="works">
    {#each works as w (w.id)}
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

<style>
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
