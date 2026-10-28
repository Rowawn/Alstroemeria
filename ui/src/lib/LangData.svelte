<script lang="ts">
  import { onMount } from "svelte";
  import { langdata, megabytes } from "./langdata-state.svelte";

  let { id }: { id: string } = $props();

  const pkg = $derived(langdata.get(id));
  const progress = $derived(langdata.progress[id]);
  const error = $derived(langdata.errors[id] ?? "");
  let picker = $state<HTMLInputElement | null>(null);

  onMount(() => langdata.start());
</script>

{#if pkg}
  <div class="card">
    {#if pkg.installed}
      <p>{pkg.label} language data is installed.</p>
    {:else}
      <p>
        <strong>{pkg.label} needs its language data</strong> (≈ {megabytes(pkg.size)}).
        {#if !pkg.published}
          It has not been published yet, so use <em>Import from file</em>, or get it from a paired device.
        {:else}
          It is downloaded once and then works offline.
        {/if}
      </p>
      {#if progress !== undefined}
        <progress value={progress} max="1"></progress>
        <button onclick={() => langdata.cancel(id)}>Cancel</button>
      {:else}
        <div class="row">
          <button class="primary" disabled={!pkg.published} onclick={() => langdata.download(id)}>Download now</button>
          <button onclick={() => picker?.click()}>Import from file</button>
          <input
            bind:this={picker}
            type="file"
            accept=".zip"
            hidden
            onchange={(e) => {
              const f = e.currentTarget.files?.[0];
              e.currentTarget.value = "";
              if (f) void langdata.importFile(f, id);
            }}
          />
        </div>
        <p class="muted">A paired device that has it will also send it the next time you sync.</p>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
    {/if}
  </div>
{/if}

<style>
  .card {
    border: 1px solid var(--border, #8884);
    border-radius: 8px;
    padding: 0.75rem 1rem;
    margin: 0.75rem 0;
  }
  progress {
    width: 100%;
  }
</style>
