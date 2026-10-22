<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Language } from "$lib/api";
  import Dictionaries from "$lib/Dictionaries.svelte";
  import Export from "$lib/Export.svelte";
  import Library from "$lib/Library.svelte";
  import Reader from "$lib/Reader.svelte";

  let languages = $state<Language[] | null>(null);
  let tab = $state<"library" | "dictionaries" | "export">("library");
  let partId = $state<string | null>(null);
  let error = $state("");

  onMount(async () => {
    try {
      languages = await api.languages();
    } catch (e) {
      error = String(e);
    }
  });
</script>

<main>
  {#if error}
    <p class="error">{error}</p>
  {:else if languages}
    {#if partId}
      {#key partId}
        <Reader {partId} {languages} onOpen={(id) => (partId = id)} onBack={() => (partId = null)} />
      {/key}
    {:else}
      <nav class="row">
        <button class:primary={tab === "library"} onclick={() => (tab = "library")}>Library</button>
        <button class:primary={tab === "dictionaries"} onclick={() => (tab = "dictionaries")}>Dictionaries</button>
        <button class:primary={tab === "export"} onclick={() => (tab = "export")}>Export</button>
      </nav>
      {#if tab === "library"}
        <Library {languages} onOpen={(id) => (partId = id)} />
      {:else if tab === "dictionaries"}
        <Dictionaries {languages} />
      {:else}
        <Export {languages} />
      {/if}
    {/if}
  {/if}
</main>

<style>
  main {
    max-width: 60rem;
    margin: 0 auto;
    padding: 1rem;
  }
  nav {
    margin-bottom: 0.5rem;
  }
</style>
