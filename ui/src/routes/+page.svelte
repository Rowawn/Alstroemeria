<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  type Work = { id: string; language: string; title: string; kind: string; created_at: number };

  let works = $state<Work[]>([]);
  let title = $state("");
  let text = $state("");
  let error = $state("");

  async function refresh() {
    works = await invoke<Work[]>("list_works");
  }

  async function add(event: Event) {
    event.preventDefault();
    error = "";
    try {
      await invoke("create_work", { title, text });
      title = "";
      text = "";
      await refresh();
    } catch (e) {
      error = String(e);
    }
  }

  onMount(refresh);
</script>

<main>
  <h1>Library</h1>

  {#if works.length === 0}
    <p class="muted">No texts yet. Add one below.</p>
  {:else}
    <ul>
      {#each works as w (w.id)}
        <li>{w.title}</li>
      {/each}
    </ul>
  {/if}

  <h2>Add text</h2>
  <form onsubmit={add}>
    <input placeholder="Title" bind:value={title} required />
    <textarea placeholder="Paste text" rows="8" bind:value={text}></textarea>
    <button type="submit">Add</button>
    {#if error}<p class="error">{error}</p>{/if}
  </form>
</main>

<style>
  :global(body) {
    font-family: system-ui, sans-serif;
    margin: 0;
    color: #222;
    background: #fafafa;
  }
  main {
    max-width: 40rem;
    margin: 0 auto;
    padding: 1.5rem 1rem;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  input,
  textarea,
  button {
    font: inherit;
    padding: 0.5rem;
  }
  .muted {
    color: #777;
  }
  .error {
    color: #b00020;
  }
</style>
