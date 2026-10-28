<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./api";
  import { updates } from "./updates.svelte";

  onMount(() => void updates.start());

  const a = $derived(updates.available);
</script>

{#if a && !updates.dismissed}
  <div class="banner" role="status">
    <div>
      <strong>Version {a.version} is available.</strong>
      {#if updates.error}<span class="error">{updates.error}</span>{/if}
      {#if a.notes}<details><summary>What's new</summary><pre>{a.notes}</pre></details>{/if}
    </div>
    <div class="row">
      {#if a.canInstall}
        <button class="primary" disabled={updates.installing} onclick={() => updates.install()}>
          {updates.installing ? "Installing…" : "Install and restart"}
        </button>
      {:else if a.pageUrl}
        <button class="primary" onclick={() => api.openUrl(a.pageUrl!)}>Download</button>
      {/if}
      <button onclick={() => (updates.dismissed = true)}>Later</button>
    </div>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    align-items: flex-start;
    border: 1px solid var(--border, #8886);
    border-radius: 8px;
    padding: 0.5rem 0.75rem;
    margin-bottom: 0.75rem;
  }
  pre {
    white-space: pre-wrap;
    margin: 0.25rem 0 0;
    font-size: 0.8rem;
  }
</style>
