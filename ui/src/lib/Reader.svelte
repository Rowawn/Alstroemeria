<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { api, type Language, type PartView, type Unit } from "./api";
  import { extHost, readerBridge } from "./ext";
  import type { MergedHints } from "./ext/hints";
  import { hintStyle } from "./ext/render";
  import type { ExtInfo, ResolvedView } from "./ext/types";
  import PanelHost from "./PanelHost.svelte";
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
  // Extensions (SPEC §6.9.6): annotators run once per part, decorators after every term change.
  let resolved = $state<ResolvedView | null>(null);
  let hints = $state<MergedHints[]>([]);
  let runGen = 0;
  let openPanelRef = $state<{ ext: ExtInfo; panelId: string } | null>(null);
  let detached = $state<{ text: string } | null>(null);
  let refreshPanels = $state(0);

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

  const panels = $derived(
    (resolved?.active ?? []).flatMap((ext) => ext.contributes.panels.map((p) => ({ ext, panel: p }))),
  );

  async function load() {
    try {
      view = await api.partView(partId);
    } catch (e) {
      error = String(e);
      return;
    }
    void runExtensions();
  }

  async function runExtensions() {
    if (!view) return;
    // Snapshots: $state proxies can't be structured-cloned into a worker.
    const v = $state.snapshot(view) as PartView;
    const gen = ++runGen;
    const lang = v.work.language;
    try {
      resolved ??= await api.resolveExtensions(lang);
      const r = $state.snapshot(resolved) as ResolvedView;
      extHost.retain(new Set(r.active.map((e) => e.id)));
      const tokens = await extHost.annotate(partId, lang, v.tokens, r);
      if (gen !== runGen) return;
      const merged = await extHost.decorate(lang, v.units, tokens, r);
      if (gen !== runGen) return;
      hints = merged;
      refreshPanels++;
    } catch (e) {
      console.warn("extensions failed", e);
    }
  }

  onMount(() => {
    opened = Date.now();
    readerBridge.openPanel = (extId, panelId) => {
      const ext = resolved?.active.find((e) => e.id === extId);
      if (ext) openPanelRef = { ext, panelId };
    };
    readerBridge.openTerm = (lang, text) => {
      if (lang === view?.work.language) {
        sel = null;
        selGen++;
        detached = { text };
      }
    };
    load();
  });

  $effect(() => {
    // Keep the bridge's view of "what is open" current for `alst.reader.current()`.
    readerBridge.current = view ? { workId: view.work.id, partId, language: view.work.language } : null;
  });

  onDestroy(() => {
    runGen++;
    readerBridge.current = null;
    readerBridge.openPanel = () => {};
    readerBridge.openTerm = () => {};
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

  type Cmd = { ext: ExtInfo; id: string; title: string };
  function commandsFor(context: "term" | "reader"): Cmd[] {
    return (resolved?.active ?? []).flatMap((ext) =>
      ext.contributes.commands
        .filter((c) => c.context === context)
        .map((c) => ({ ext, id: c.id, title: c.title })),
    );
  }

  async function runCommand(c: Cmd, context: "term" | "reader") {
    if (!view) return;
    const args =
      context === "term"
        ? { language: view.work.language, text: selectedText, sentence: sentenceText }
        : { language: view.work.language, partId };
    await extHost.runCommand(c.ext, view.work.language, c.id, args);
  }

  const cls = (u: Unit, i: number) =>
    `w s${u.status}${range && i >= range.lo && i <= range.hi ? " sel" : ""}`;
</script>

{#if error}
  <p class="error">{error}</p>
  <button onclick={onBack}>Back</button>
{:else if view}
  <div class="layout" class:open={!!sel || !!detached || !!openPanelRef}>
    <div class="main">
      <div class="row bar">
        <button onclick={onBack}>← Library</button>
        <strong>{view.work.title}</strong>
        <span class="muted">
          {view.parts[idx]?.title ?? `Part ${idx + 1}`} ({idx + 1}/{view.parts.length})
        </span>
        {#each panels as { ext, panel } (ext.id + panel.id)}
          <button
            class:primary={openPanelRef?.ext.id === ext.id && openPanelRef.panelId === panel.id}
            onclick={() =>
              (openPanelRef = openPanelRef?.ext.id === ext.id && openPanelRef.panelId === panel.id ? null : { ext, panelId: panel.id })}
            >{panel.title}</button
          >
        {/each}
      </div>

      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="text" dir={view.rtl ? "rtl" : "ltr"} onmouseup={onPointerUp}>
        {#each units as u, i}{#if u.isWord}{@const h = hints[i]}<span
              class={cls(u, i)}
              style={hintStyle(h)}
              title={h?.tooltip}
              data-i={i}
              >{#if h?.ruby}<ruby>{u.text}<rt>{h.ruby}</rt></ruby>{:else}{u.text}{/if}{#each h?.badges ?? [] as b}<sup
                  class="badge"
                  style={b.role ? `color: var(--role-${b.role})` : ""}>{b.text}</sup
                >{/each}</span
            >{:else}<span class="nw" data-i={i}>{u.text}</span>{/if}{/each}
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

    {#if (sel && range && language) || detached || openPanelRef}
      <div class="side">
        {#if sel && range && language}
          {#key selGen}
            <aside class="editor">
              <TermEditor
                {language}
                text={selectedText}
                sentence={sentenceText}
                commands={commandsFor("term")}
                onCommand={(c) => runCommand(c, "term")}
                onSaved={saved}
                onClose={() => (sel = null)}
                onExtend={extend}
              />
            </aside>
          {/key}
        {:else if detached && language}
          {#key selGen}
            <aside class="editor">
              <TermEditor
                {language}
                text={detached.text}
                sentence=""
                commands={[]}
                onCommand={() => {}}
                onSaved={saved}
                onClose={() => (detached = null)}
                onExtend={() => {}}
              />
            </aside>
          {/key}
        {/if}
        {#if openPanelRef && view}
          <aside class="panel">
            <PanelHost
              ext={openPanelRef.ext}
              panelId={openPanelRef.panelId}
              language={view.work.language}
              refresh={refreshPanels}
              onClose={() => (openPanelRef = null)}
            />
          </aside>
        {/if}
      </div>
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
  .badge {
    font-size: 0.6em;
    margin-left: 1px;
    font-weight: 600;
  }
  ruby rt {
    font-size: 0.55em;
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
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
    .side {
      position: sticky;
      top: 1rem;
      align-self: start;
      max-height: calc(100vh - 2rem);
      overflow: auto;
    }
  }
  @media (max-width: 899px) {
    .side {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      max-height: 60vh;
      overflow: auto;
      z-index: 5;
      gap: 0;
    }
    aside {
      border-radius: 14px 14px 0 0;
      box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.25);
    }
    .layout.open .main {
      padding-bottom: 60vh;
    }
  }
</style>
