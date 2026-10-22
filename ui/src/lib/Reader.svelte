<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Language, type PartView, type Unit } from "./api";
  import TermEditor from "./TermEditor.svelte";

  let { partId, languages, onOpen, onBack }: {
    partId: string;
    languages: Language[];
    onOpen: (partId: string) => void;
    onBack: () => void;
  } = $props();

  let view = $state<PartView | null>(null);
  let error = $state("");
  // Selection as source offsets so it survives a reload that regroups units into a multi-word term.
  let sel = $state<{ start: number; end: number } | null>(null);
  let selGen = $state(0); // new editor instance per click/drag, not per extend
  let finishing = $state(false);
  let markKnown = $state(true);
  let opened = Date.now();

  const language = $derived(languages.find((l) => l.id === view?.work.language));
  const units = $derived(view?.units ?? []);
  const range = $derived.by(() => {
    if (!sel) return null;
    const lo = units.findIndex((u) => u.end > sel!.start);
    let hi = lo;
    units.forEach((u, i) => {
      if (u.start < sel!.end) hi = i;
    });
    return lo < 0 ? null : { lo, hi: Math.max(lo, hi) };
  });
  const selectedText = $derived(range ? units.slice(range.lo, range.hi + 1).map((u) => u.text).join("") : "");
  const sentenceText = $derived(
    range ? units.filter((u) => u.sentence === units[range.lo].sentence).map((u) => u.text).join("").trim() : "",
  );
  const idx = $derived(view?.parts.findIndex((p) => p.id === partId) ?? -1);

  async function load() {
    try {
      view = await api.partView(partId);
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    opened = Date.now();
    load();
  });

  function unitIndex(node: Node | null): number | null {
    const el = node instanceof Element ? node : node?.parentElement;
    const i = el?.closest<HTMLElement>("[data-i]")?.dataset.i;
    return i === undefined ? null : Number(i);
  }

  function pick(from: number, to: number, fresh = true) {
    if (fresh) selGen++;
    let lo = Math.min(from, to);
    let hi = Math.max(from, to);
    while (lo < hi && !units[lo].isWord) lo++;
    while (hi > lo && !units[hi].isWord) hi--;
    if (!units[lo].isWord) return;
    sel = { start: units[lo].start, end: units[hi].end };
  }

  function onPointerUp(e: MouseEvent) {
    const s = window.getSelection();
    if (s && !s.isCollapsed && s.anchorNode && s.focusNode) {
      const a = unitIndex(s.anchorNode);
      const b = unitIndex(s.focusNode);
      if (a !== null && b !== null) {
        s.removeAllRanges();
        return pick(a, b);
      }
    }
    const i = unitIndex(e.target as Node);
    if (i !== null && units[i].isWord) pick(i, i);
  }

  /** Grow the selection by one word on either side (for touch screens, where dragging is awkward). */
  function extend(side: "left" | "right") {
    if (!range) return;
    if (side === "left") {
      let i = range.lo - 1;
      while (i >= 0 && !units[i].isWord) i--;
      if (i >= 0 && units[i].sentence === units[range.lo].sentence) pick(i, range.hi, false);
    } else {
      let i = range.hi + 1;
      while (i < units.length && !units[i].isWord) i++;
      if (i < units.length && units[i].sentence === units[range.hi].sentence) pick(range.lo, i, false);
    }
  }

  async function saved() {
    await load();
  }

  async function finish() {
    await api.finishPart(partId, markKnown, Date.now() - opened);
    finishing = false;
    sel = null;
    const next = view?.parts[idx + 1];
    if (next) onOpen(next.id);
    else onBack();
  }

  const cls = (u: Unit, i: number) =>
    `w s${u.status}${range && i >= range.lo && i <= range.hi ? " sel" : ""}`;
</script>

{#if error}
  <p class="error">{error}</p>
  <button onclick={onBack}>Back</button>
{:else if view}
  <div class="layout" class:open={!!sel}>
    <div class="main">
      <div class="row bar">
        <button onclick={onBack}>← Library</button>
        <strong>{view.work.title}</strong>
        <span class="muted">
          {view.parts[idx]?.title ?? `Part ${idx + 1}`} ({idx + 1}/{view.parts.length})
        </span>
      </div>

      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="text" dir={view.rtl ? "rtl" : "ltr"} onmouseup={onPointerUp}>
        {#each units as u, i}<span class={u.isWord ? cls(u, i) : "nw"} data-i={i}>{u.text}</span>{/each}
      </div>

      <div class="row bar">
        <button disabled={idx <= 0} onclick={() => onOpen(view!.parts[idx - 1].id)}>‹ Previous</button>
        <button onclick={() => (finishing = true)}>Finish part ✓</button>
        <button disabled={idx >= view.parts.length - 1} onclick={() => onOpen(view!.parts[idx + 1].id)}>Next ›</button>
      </div>
      {#if finishing}
        <div class="finish">
          <label>
            <input type="checkbox" bind:checked={markKnown} />
            Mark remaining new words as known
          </label>
          <div class="row">
            <button class="primary" onclick={finish}>Finish</button>
            <button onclick={() => (finishing = false)}>Cancel</button>
          </div>
        </div>
      {/if}
    </div>

    {#if sel && range && language}
      {#key selGen}
        <aside>
          <TermEditor
            {language}
            text={selectedText}
            sentence={sentenceText}
            onSaved={saved}
            onClose={() => (sel = null)}
            onExtend={extend}
          />
        </aside>
      {/key}
    {/if}
  </div>
{/if}

<style>
  .layout {
    display: grid;
    grid-template-columns: 1fr;
    gap: 1rem;
  }
  .text {
    white-space: pre-wrap;
    line-height: 2;
    font-size: 1.15rem;
    margin: 1rem 0;
    user-select: text;
  }
  .w {
    cursor: pointer;
    border-radius: 3px;
    padding: 0 1px;
  }
  .w:hover {
    outline: 1px solid var(--accent);
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
  .s98 {
    text-decoration: underline dotted var(--muted);
  }
  .sel {
    outline: 2px solid var(--accent);
  }
  .bar {
    justify-content: space-between;
  }
  .finish {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.8rem;
    margin-top: 0.6rem;
    background: var(--panel);
  }
  aside {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0.8rem;
  }
  @media (min-width: 900px) {
    .layout.open {
      grid-template-columns: 1fr 22rem;
    }
    aside {
      position: sticky;
      top: 1rem;
      align-self: start;
      max-height: calc(100vh - 2rem);
      overflow: auto;
    }
  }
  @media (max-width: 899px) {
    aside {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      max-height: 60vh;
      overflow: auto;
      border-radius: 14px 14px 0 0;
      box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.25);
      z-index: 5;
    }
    .layout.open .main {
      padding-bottom: 60vh;
    }
  }
</style>
