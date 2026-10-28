<script lang="ts">
  import { onMount } from "svelte";
  import { localNetworkState, openAppSettings, requestLocalNetwork, type LocalNetwork } from "./android";

  let perm = $state<LocalNetwork>(localNetworkState());
  /** The system prompt was shown (or could not be): from then on a denial needs the settings page. */
  let asked = $state(false);

  const refresh = () => (perm = localNetworkState());

  onMount(() => {
    if (perm === "denied") {
      requestLocalNetwork();
      asked = true;
    }
    // The prompt, or the settings page, returns to the app: look again.
    const timer = setInterval(refresh, 1000);
    window.addEventListener("focus", refresh);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  });
</script>

{#if perm === "denied"}
  <div class="card">
    <p>
      <strong>Local network access is off.</strong>
      {#if asked}Android may not have shown its question, or it was declined.{/if}
      Without it this phone can't find or reach your other devices. Allow “Nearby devices” or “Local network” for
      Alstroemeria in the app's settings.
    </p>
    <button class="primary" onclick={() => openAppSettings()}>Open app settings</button>
  </div>
{/if}

<style>
  .card {
    border: 1px solid var(--border, #8886);
    border-radius: 8px;
    padding: 0.5rem 0.75rem;
    margin: 0.75rem 0;
  }
</style>
