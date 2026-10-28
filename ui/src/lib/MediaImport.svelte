<script lang="ts">
  import { errorText } from "./langdata-state.svelte";
  import { api } from "./api";

  let { language, onDone }: { language: string; onDone: () => void } = $props();

  let title = $state("");
  let media = $state<File | null>(null);
  let subtitle = $state<File | null>(null);
  let busy = $state(false);
  let progress = $state(0);
  let error = $state("");

  async function submit(event: Event) {
    event.preventDefault();
    error = "";
    if (!media || !subtitle) {
      error = "Choose an audio or video file and its subtitle or lyrics file.";
      return;
    }
    busy = true;
    progress = 0;
    try {
      await api.importMedia(language, title, media, subtitle, (f) => (progress = f));
      title = "";
      media = subtitle = null;
      onDone();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<form onsubmit={submit}>
  <input placeholder="Title (optional)" bind:value={title} />
  <label>
    Audio or video
    <input type="file" accept="audio/*,video/*,.mp3,.m4a,.aac,.ogg,.opus,.wav,.flac,.mp4,.m4v,.webm,.mov" onchange={(e) => (media = e.currentTarget.files?.[0] ?? null)} />
  </label>
  <label>
    Subtitles or lyrics (.srt, .vtt, .lrc)
    <input type="file" accept=".srt,.vtt,.lrc,text/vtt,text/plain" onchange={(e) => (subtitle = e.currentTarget.files?.[0] ?? null)} />
  </label>
  <button class="primary" type="submit" disabled={busy}>
    {busy ? (progress < 1 ? `Uploading ${Math.round(progress * 100)}%` : "Importing…") : "Add"}
  </button>
  {#if error}<p class="error">{error}</p>{/if}
</form>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
</style>
