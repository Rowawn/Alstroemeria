<script lang="ts">
  import { api } from "./api";
  import { screens } from "./ext/screens.svelte";

  // The host asks for a file on an extension's behalf: a sandboxed frame has no user gesture to open a picker.
  // The file is uploaded to the staging area and only its id goes back to the extension.
  let busy = $state(false);
  let progress = $state(0);
  let error = $state("");

  async function chosen(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    const prompt = screens.filePrompt;
    if (!file || !prompt) return;
    busy = true;
    error = "";
    try {
      const uploadId = await api.uploadStaged(file, (f) => (progress = f));
      screens.filePrompt = null;
      prompt.resolve({ uploadId, name: file.name });
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  function cancel() {
    screens.filePrompt?.resolve(null);
    screens.filePrompt = null;
  }
</script>

{#if screens.filePrompt}
  <div class="back" role="presentation">
    <div class="box" role="dialog" aria-modal="true" aria-label="Choose a file">
      <p>Choose a file to give to this extension.</p>
      <input type="file" accept={screens.filePrompt.accept} onchange={chosen} disabled={busy} />
      {#if busy}<progress value={progress} max="1"></progress>{/if}
      {#if error}<p class="error">{error}</p>{/if}
      <div class="row"><button onclick={cancel}>Cancel</button></div>
    </div>
  </div>
{/if}

<style>
  .back {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.4);
    display: grid;
    place-items: center;
    z-index: 60;
  }
  .box {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1rem;
    width: min(26rem, 92vw);
    display: grid;
    gap: 0.6rem;
  }
</style>
