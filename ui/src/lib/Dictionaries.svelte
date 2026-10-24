<script lang="ts">
  import { onMount } from "svelte";
  import { sync } from "./sync.svelte";
  import { api, type DictInfo, type Language } from "./api";

  let { languages }: { languages: Language[] } = $props();

  let dicts = $state<DictInfo[]>([]);
  let language = $state(languages[0]?.id ?? "");
  let file = $state<File | null>(null);
  let busy = $state(false);
  let error = $state("");

  const refresh = async () => (dicts = await api.dictionaries());

  async function add(event: Event) {
    event.preventDefault();
    if (!file) return;
    error = "";
    busy = true;
    try {
      await api.importDictionary(language, file);
      file = null;
      (event.target as HTMLFormElement).reset();
      await refresh();
    } catch (e) {
      error = String(e);
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

  onMount(refresh);
</script>

<h1>Dictionaries</h1>
<p class="muted">
  Import Yomitan-format dictionaries (a .zip with <code>index.json</code> and <code>term_bank_*.json</code>). Data stays
  on this device. Attribution: respect each dictionary's own license.
</p>

{#if dicts.length === 0}
  <p class="muted">No dictionaries imported. Online lookup links in the term editor still work.</p>
{:else}
  <ul>
    {#each dicts as d (d.id)}
      <li class="row">
        <label>
          <input
            type="checkbox"
            checked={d.enabled}
            onchange={async (e) => {
              await api.setDictionaryEnabled(d.id, e.currentTarget.checked);
              await refresh();
            }}
          />
          <strong>{d.title}</strong>
        </label>
        <span class="muted">{d.language} · {d.entryCount.toLocaleString()} entries{#if d.metaCount} · {d.metaCount.toLocaleString()} frequency/pitch rows{/if}</span>
        {#if !d.sourceId && sync.overview && sync.overview.peers.length > 0}
          <span class="muted" title="This dictionary was imported before sync existed">Re-import to share with other devices</span>
        {/if}
        {#if !d.rulesImported && languages.find((l) => l.id === d.language)?.scanLookup}
          <span class="warn" title="This dictionary was imported before conjugation rules were stored">Re-import to enable conjugation lookup</span>
        {/if}
        <button
          onclick={async () => {
            await api.deleteDictionary(d.id);
            await refresh();
          }}>Delete</button
        >
      </li>
    {/each}
  </ul>
{/if}

<h2>Import</h2>
<form onsubmit={add}>
  <label class="row">
    Language
    <select bind:value={language}>
      {#each languages as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
    </select>
  </label>
  <input type="file" accept=".zip,application/zip" onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)} required />
  <button class="primary" type="submit" disabled={busy || !file}>{busy ? "Importing… (large dictionaries take a while)" : "Import"}</button>
  {#if error}<p class="error">{error}</p>{/if}
</form>

<style>
  ul {
    list-style: none;
    padding: 0;
  }
  li {
    margin-bottom: 0.4rem;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .warn {
    color: var(--role-warn);
    font-size: 0.85rem;
  }
</style>
