<script lang="ts">
  import { api, type Language } from "./api";
  import { extHost, readerBridge } from "./ext";
  import type { ExtInfo } from "./ext/types";
  import { tick } from "svelte";

  // Ctrl/Cmd+K: commands contributed by active extensions (SPEC §6.9.7). `reader` commands appear only while a
  // part is open; `global` ones always. Commands run in the extension's worker (30 s limit).
  let { languages }: { languages: Language[] } = $props();

  type Item = { ext: ExtInfo; language: string; id: string; title: string; context: string };
  let open = $state(false);
  let query = $state("");
  let items = $state<Item[]>([]);
  let cursor = $state(0);
  let input: HTMLInputElement | undefined = $state();
  let busy = $state(false);

  const shown = $derived(
    items.filter((i) => `${i.title} ${i.ext.name}`.toLowerCase().includes(query.trim().toLowerCase())),
  );

  async function show() {
    open = true;
    query = "";
    cursor = 0;
    items = [];
    const reader = readerBridge.current;
    const langs = reader ? [reader.language] : languages.map((l) => l.id);
    const seen = new Set<string>();
    const found: Item[] = [];
    for (const lang of langs) {
      try {
        const r = await api.resolveExtensions(lang);
        for (const ext of r.active) {
          for (const c of ext.contributes.commands) {
            const ctx = c.context ?? "global";
            if (ctx === "term" || (ctx === "reader" && !reader)) continue;
            const k = `${ext.id}/${c.id}/${lang}`;
            const kNoLang = `${ext.id}/${c.id}`;
            if (!reader && seen.has(kNoLang)) continue;
            seen.add(k);
            seen.add(kNoLang);
            found.push({ ext, language: lang, id: c.id, title: c.title, context: ctx });
          }
        }
      } catch (e) {
        console.warn("command list failed", e);
      }
    }
    items = found;
    await tick();
    input?.focus();
  }

  async function run(item: Item) {
    busy = true;
    open = false;
    const reader = readerBridge.current;
    const args = item.context === "reader" && reader ? { language: reader.language, partId: reader.partId, workId: reader.workId } : { language: item.language };
    await extHost.runCommand(item.ext, item.language, item.id, args);
    busy = false;
  }

  function onKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      if (open) open = false;
      else void show();
      return;
    }
    if (!open) return;
    if (e.key === "Escape") open = false;
    else if (e.key === "ArrowDown") (e.preventDefault(), (cursor = Math.min(cursor + 1, shown.length - 1)));
    else if (e.key === "ArrowUp") (e.preventDefault(), (cursor = Math.max(cursor - 1, 0)));
    else if (e.key === "Enter" && shown[cursor]) run(shown[cursor]);
  }
</script>

<svelte:window onkeydown={onKey} />

<button class="fab" title="Commands (Ctrl/Cmd+K)" aria-label="Open command palette" disabled={busy} onclick={show}>⌘</button>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={() => (open = false)}>
    <div class="palette" onclick={(e) => e.stopPropagation()} role="dialog" tabindex="-1" aria-label="Command palette">
      <input bind:this={input} bind:value={query} placeholder="Type a command…" oninput={() => (cursor = 0)} />
      {#if shown.length === 0}
        <p class="muted pad">{items.length === 0 ? "No extension commands available here." : "No match."}</p>
      {/if}
      {#each shown as item, i (item.ext.id + item.id + item.language)}
        <button class="item" class:cur={i === cursor} onclick={() => run(item)} onmousemove={() => (cursor = i)}>
          <span>{item.title}</span>
          <span class="muted">{item.ext.name}{item.context === "reader" ? " · reader" : ""}</span>
        </button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .fab {
    position: fixed;
    right: 0.8rem;
    bottom: 0.8rem;
    z-index: 20;
    border-radius: 50%;
    width: 2.4rem;
    height: 2.4rem;
    padding: 0;
    opacity: 0.8;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.35);
    z-index: 40;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
  }
  .palette {
    width: min(34rem, calc(100vw - 2rem));
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0.5rem;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
    max-height: 60vh;
    overflow: auto;
  }
  input {
    width: 100%;
    margin-bottom: 0.4rem;
  }
  .item {
    display: flex;
    justify-content: space-between;
    width: 100%;
    gap: 1rem;
    border: none;
    text-align: left;
    background: transparent;
  }
  .item.cur {
    background: var(--bg);
    outline: 1px solid var(--accent);
  }
  .pad {
    padding: 0.5rem;
  }
</style>
