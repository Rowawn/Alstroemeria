<script lang="ts">
  import { onMount } from "svelte";
  import { updates } from "./updates.svelte";

  onMount(() => void updates.start());
  let notices = $state<string | null>(null);
  let showing = $state(false);

  // The notices are a lazy chunk of the bundle: no network, loaded on first open.
  async function toggle() {
    showing = !showing;
    if (showing && notices === null) notices = (await import("../../../THIRD-PARTY-NOTICES.md?raw")).default;
  }
</script>

<h1>About</h1>
<p class="muted">Alstroemeria is open source: the core is MPL-2.0, first-party extensions and the SDK are MIT.</p>

<h2>Language data</h2>
<ul>
  <li>
    <strong>Japanese tokenizer:</strong> <a href="https://github.com/lindera/lindera">Lindera</a> (MIT) with the
    <strong>mecab-ipadic-2.7.0</strong> dictionary, © Nara Institute of Science and Technology (NAIST) and ICOT Free Software.
    Redistributed under its permissive notice, which disclaims all warranties.
  </li>
  <li>
    <strong>Korean tokenizer:</strong> Lindera with <strong>mecab-ko-dic</strong> (Apache-2.0), © Eunjeon project.
  </li>
  <li>
    <strong>Korean hanja:</strong> data derived from <a href="https://github.com/libhangul/libhangul">libhangul</a>'s
    <code>hanja.txt</code>, © 2005, 2006 Choe Hwanjin, BSD-3-Clause (full notice in the extension's
    <code>DATA-LICENSE</code>).
  </li>
  <li>
    <strong>Dictionaries you import</strong> (for example JMdict, CC&nbsp;BY-SA 4.0, or Kanjium pitch data, CC&nbsp;BY-SA 4.0) stay
    under their own licences; they are stored on this device only.
  </li>
</ul>

<h2>Updates</h2>
{#if updates.info}
  <p>Version {updates.info.version}.</p>
  <label class="row">
    <input type="checkbox" checked={updates.info.autoCheck} onchange={(e) => updates.setAuto(e.currentTarget.checked)} />
    Check automatically (at start and once a day)
  </label>
  <p>
    <button disabled={updates.checking} onclick={() => updates.check(true)}>
      {updates.checking ? "Checking…" : "Check now"}
    </button>
    {#if updates.upToDate}<span class="muted">You have the latest version.</span>{/if}
    {#if updates.error}<span class="error">{updates.error}</span>{/if}
  </p>
{/if}
<p class="muted">
  Update checks are the only internet traffic Alstroemeria makes by itself (it asks GitHub for the latest version
  number). The other traffic is what you start: importing web pages and podcasts, and downloading language data.
  Turn the check off and it stops.
</p>

<h2>Licences</h2>
<p>
  <button onclick={toggle}>{showing ? "Hide" : "Show"} third-party notices</button>
</p>
{#if showing}
  <pre class="notices">{notices ?? "Loading…"}</pre>
{/if}

<style>
  .notices {
    max-height: 60vh;
    overflow: auto;
    white-space: pre-wrap;
    font-size: 0.75rem;
  }
</style>
