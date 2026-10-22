<script lang="ts">
  import { api, lookupHref, STATUSES, statusLabel, type Language, type TermView } from "./api";

  let { language, text, sentence, onSaved, onClose, onExtend }: {
    language: Language;
    text: string;
    sentence: string;
    onSaved: () => void;
    onClose: () => void;
    onExtend: (side: "left" | "right") => void;
  } = $props();

  let tv = $state<TermView | null>(null);
  let status = $state(0);
  let translation = $state("");
  let notes = $state("");
  let parents = $state<string[]>([]);
  let newParent = $state("");
  let error = $state("");
  let loaded = "";

  // Reload when the selection is extended. Only the first load of an editor instance counts as a lookup event.
  $effect(() => {
    const t = text;
    const first = loaded === "";
    api
      .openTerm(language.id, t, first)
      .then((v) => {
        tv = v;
        status = v.term?.status ?? 0;
        translation = v.term?.translation ?? "";
        notes = v.term?.notes ?? "";
        parents = v.term?.parents.map((p) => p.text) ?? [];
        error = "";
      })
      .catch((e) => (error = String(e)));
    loaded = t;
  });

  async function save(patchStatus?: number) {
    if (!tv) return;
    error = "";
    try {
      if (newParent.trim()) addParent();
      const t = await api.saveTerm(
        language.id,
        tv.text,
        { status: patchStatus ?? status, translation, notes },
        parents,
      );
      status = t.status;
      onSaved();
    } catch (e) {
      error = String(e);
    }
  }

  function addParent() {
    const p = newParent.trim();
    if (p && !parents.includes(p)) parents = [...parents, p];
    newParent = "";
  }

  const useDefinition = (g: string) => (translation = translation ? `${translation}; ${g.split("\n")[0]}` : g.split("\n")[0]);
</script>

<div class="head row">
  <h3>{tv?.text ?? text}</h3>
  <button onclick={onClose} aria-label="Close">✕</button>
</div>
{#if tv && tv.wordCount > 1}<p class="muted">Phrase of {tv.wordCount} words</p>{/if}
<p class="sentence muted">{sentence}</p>
<div class="row">
  <button onclick={() => onExtend("left")} title="Add the previous word">‹ word</button>
  <button onclick={() => onExtend("right")} title="Add the next word">word ›</button>
</div>

<div class="statuses">
  {#each STATUSES as s}
    <button class:active={status === s} class={`b s${s}`} onclick={() => save(s)} title={statusLabel(s)}>{s}</button>
  {/each}
</div>
<p class="muted small">{statusLabel(status)}</p>

<label>
  Translation
  <textarea rows="2" bind:value={translation}></textarea>
</label>
<label>
  Notes
  <textarea rows="2" bind:value={notes}></textarea>
</label>
<div>
  Parents
  <div class="row chips">
    {#each parents as p (p)}
      <span class="chip">{p} <button class="x" onclick={() => (parents = parents.filter((x) => x !== p))}>✕</button></span>
    {/each}
  </div>
  <div class="row">
    <input
      placeholder="Add parent term"
      bind:value={newParent}
      onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), addParent())}
    />
    <button onclick={addParent}>Add</button>
  </div>
</div>
<div class="row actions">
  <button class="primary" onclick={() => save()}>Save</button>
</div>
{#if error}<p class="error">{error}</p>{/if}

{#if language.lookupUrls.length}
  <div class="row links">
    {#each language.lookupUrls as l (l.name)}
      <button onclick={() => api.openUrl(lookupHref(l.url, tv?.text ?? text)).catch((e) => (error = String(e)))}>{l.name} ↗</button>
    {/each}
  </div>
{/if}

{#if tv?.dict.length}
  <div class="dict">
    {#each tv.dict as r, i (i)}
      <div class="entry">
        <div class="muted small">{r.dictTitle}</div>
        <strong>{r.term}</strong>{#if r.reading} <span class="muted">{r.reading}</span>{/if}
        {#each r.glossary as g}
          <div class="gloss">
            <button class="use" onclick={() => useDefinition(g)} title="Add to translation">＋</button>
            <span>{g}</span>
          </div>
        {/each}
      </div>
    {/each}
  </div>
{/if}

<style>
  .head {
    justify-content: space-between;
  }
  h3 {
    margin: 0;
  }
  .sentence {
    font-size: 0.85rem;
    margin: 0.4rem 0;
  }
  .small {
    font-size: 0.8rem;
    margin: 0.2rem 0 0.6rem;
  }
  label {
    display: block;
    margin-bottom: 0.5rem;
  }
  textarea {
    width: 100%;
    display: block;
  }
  .statuses {
    display: flex;
    gap: 0.3rem;
    margin-top: 0.6rem;
    flex-wrap: wrap;
  }
  .b {
    min-width: 2.2rem;
  }
  .b.active {
    outline: 2px solid var(--accent);
    font-weight: bold;
  }
  .s0 {
    background: var(--s0);
  }
  .s1 {
    background: var(--s1);
  }
  .s2 {
    background: var(--s2);
  }
  .s3 {
    background: var(--s3);
  }
  .s4 {
    background: var(--s4);
  }
  .s5 {
    background: var(--s5);
  }
  .chip {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 0 0.5rem;
  }
  .x,
  .use {
    border: none;
    padding: 0 0.3rem;
    background: transparent;
  }
  .actions,
  .links {
    margin: 0.6rem 0;
  }
  .dict {
    border-top: 1px solid var(--border);
    margin-top: 0.6rem;
    padding-top: 0.4rem;
  }
  .entry {
    margin-bottom: 0.7rem;
  }
  .gloss {
    display: flex;
    gap: 0.2rem;
    white-space: pre-line;
    font-size: 0.9rem;
  }
</style>
