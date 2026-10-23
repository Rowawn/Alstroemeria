<script lang="ts">
  import { api, type Feed } from "./api";

  let { language, onDone }: { language: string; onDone: () => void } = $props();

  let url = $state("");
  let feed = $state<Feed | null>(null);
  let feedUrl = "";
  let busy = $state(false);
  let importing = $state<string | null>(null);
  let error = $state("");

  async function load(event: Event) {
    event.preventDefault();
    error = "";
    busy = true;
    try {
      feed = await api.podcastFeed(url.trim());
      feedUrl = url.trim();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function add(guid: string) {
    error = "";
    importing = guid;
    try {
      await api.podcastImport(language, feedUrl, guid);
      onDone();
    } catch (e) {
      error = String(e);
    } finally {
      importing = null;
    }
  }

  const hasTranscript = (e: Feed["episodes"][number]) => e.transcripts.length > 0 && !!e.audioUrl;
</script>

<form onsubmit={load}>
  <input type="url" placeholder="Podcast feed URL (RSS)" bind:value={url} required />
  <button class="primary" type="submit" disabled={busy}>{busy ? "Loading…" : "Load episodes"}</button>
</form>
{#if error}<p class="error">{error}</p>{/if}
{#if feed}
  <h3>{feed.title}</h3>
  <p class="muted">
    Only episodes whose feed includes a transcript can be imported (a transcript is needed to follow along).
  </p>
  <ul>
    {#each feed.episodes.slice(0, 100) as e (e.guid)}
      <li class="row">
        <span class="grow">
          {e.title}
          <span class="muted">{e.duration ?? ""}</span>
        </span>
        {#if hasTranscript(e)}
          <button disabled={importing !== null} onclick={() => add(e.guid)}>{importing === e.guid ? "Downloading…" : "Add"}</button>
        {:else}
          <span class="muted">no transcript</span>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
  ul {
    list-style: none;
    padding: 0;
  }
  li {
    justify-content: space-between;
    padding: 0.25rem 0;
  }
  .grow {
    flex: 1;
  }
</style>
