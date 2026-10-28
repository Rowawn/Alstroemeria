<script lang="ts">
  import { onMount } from "svelte";
  import LangData from "./LangData.svelte";
  import { langdata } from "./langdata-state.svelte";

  onMount(() => langdata.start());
</script>

<h1>Languages</h1>
<p class="muted">
  Japanese and Korean split text into words with a dictionary that is not built into the app. Download it once; it is
  also shared between your paired devices.
</p>
{#each langdata.packages as p (p.id)}
  <h2>{p.label}</h2>
  <LangData id={p.id} />
{/each}
