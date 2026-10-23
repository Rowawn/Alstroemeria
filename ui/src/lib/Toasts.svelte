<script lang="ts">
  import { dismiss, toasts } from "./notify.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each toasts as t (t.id)}
    <div class="toast" class:error={t.kind === "error"}>
      <span>{t.message}</span>
      <button onclick={() => dismiss(t.id)} aria-label="Dismiss">✕</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    top: 0.6rem;
    left: 50%;
    transform: translateX(-50%);
    z-index: 50;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    width: min(32rem, calc(100vw - 2rem));
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    gap: 0.6rem;
    align-items: center;
    justify-content: space-between;
    background: var(--panel);
    border: 1px solid var(--border);
    border-left: 4px solid var(--accent);
    border-radius: 8px;
    padding: 0.5rem 0.7rem;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  }
  .toast.error {
    border-left-color: #d03050;
  }
  button {
    border: none;
    background: none;
    padding: 0 0.2rem;
  }
</style>
