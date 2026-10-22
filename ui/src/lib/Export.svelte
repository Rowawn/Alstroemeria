<script lang="ts">
  import { api, statusLabel, type Language } from "./api";

  let { languages }: { languages: Language[] } = $props();

  let language = $state(languages[0]?.id ?? "");
  let min = $state(1);
  let max = $state(5);
  let tsv = $state("");
  let count = $state<number | null>(null);
  let message = $state("");
  let error = $state("");

  async function build() {
    error = message = "";
    try {
      const r = await api.exportTsv(language, min, max);
      tsv = r.tsv;
      count = r.count;
    } catch (e) {
      error = String(e);
    }
  }

  async function save() {
    try {
      message = `Saved to ${await api.saveExport(`alstroemeria-${language}.tsv`, tsv)}`;
    } catch (e) {
      error = String(e);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(tsv);
      message = "Copied to clipboard";
    } catch (e) {
      error = String(e);
    }
  }
</script>

<h1>Export for Anki</h1>
<p class="muted">
  Tab-separated text for Anki's File → Import. Columns: term, translation, notes, parents, example sentence, tags.
</p>
<div class="row">
  <label>
    Language
    <select bind:value={language}>
      {#each languages as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
    </select>
  </label>
  <label>
    From
    <select bind:value={min}>
      {#each [0, 1, 2, 3, 4, 5, 98, 99] as s}<option value={s}>{statusLabel(s)}</option>{/each}
    </select>
  </label>
  <label>
    To
    <select bind:value={max}>
      {#each [0, 1, 2, 3, 4, 5, 98, 99] as s}<option value={s}>{statusLabel(s)}</option>{/each}
    </select>
  </label>
  <button class="primary" onclick={build}>Preview</button>
</div>

{#if count !== null}
  <p>{count} term{count === 1 ? "" : "s"}</p>
  <textarea readonly rows="12" value={tsv}></textarea>
  <div class="row">
    <button onclick={save} disabled={count === 0}>Save to Downloads</button>
    <button onclick={copy} disabled={count === 0}>Copy</button>
  </div>
{/if}
{#if message}<p>{message}</p>{/if}
{#if error}<p class="error">{error}</p>{/if}

<style>
  textarea {
    width: 100%;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    margin: 0.4rem 0;
  }
</style>
