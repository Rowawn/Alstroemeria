<script lang="ts">
  import { errorText } from "./langdata-state.svelte";
  import { api } from "./api";

  let { language, onDone }: { language: string; onDone: () => void } = $props();

  let url = $state("");
  let busy = $state(false);
  let error = $state("");

  async function submit(event: Event) {
    event.preventDefault();
    error = "";
    busy = true;
    try {
      await api.importYoutube(language, url.trim());
      url = "";
      onDone();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<form onsubmit={submit}>
  <input type="url" placeholder="https://www.youtube.com/watch?v=…" bind:value={url} required />
  <p class="muted">Imports the video's subtitles as text, using the yt-dlp you installed. Nothing else is downloaded.</p>
  <button class="primary" type="submit" disabled={busy}>{busy ? "Fetching subtitles…" : "Add"}</button>
  {#if error}<p class="error">{error}</p>{/if}
</form>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
</style>
