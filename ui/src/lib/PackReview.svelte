<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api, type PackOpened, type PackProgress } from "./api";
  import LangData from "./LangData.svelte";
  import { errorText, needsLangData } from "./langdata-state.svelte";
  import { Review, selectable, statusLabel } from "./pack-review.svelte";

  let { opened, onClose, onChanged }: { opened: PackOpened; onClose: () => void; onChanged: () => void } = $props();

  // The parent re-creates this component per pack (`{#key}`), so the first value is the one wanted.
  // svelte-ignore state_referenced_locally
  const review = new Review(opened.plan);
  // svelte-ignore state_referenced_locally
  const handle = opened.handle;
  let error = $state("");
  let langdataId = $state("");

  const plan = $derived(review.plan);
  const unfinished = $derived(review.phase === "running");

  onMount(() => {
    const un = listen<PackProgress>("alst:pack-progress", (e) => {
      if (e.payload.handle === handle) review.onProgress(e.payload);
    });
    return () => void un.then((f) => f());
  });

  async function run(ids: string[]) {
    error = "";
    langdataId = "";
    review.start(ids);
    try {
      const report = await api.packRun(handle, ids, review.seedVocab);
      review.finish(report);
      review.replacePlan(await api.packPlan(handle));
      onChanged();
    } catch (e) {
      langdataId = needsLangData(e) ?? "";
      error = errorText(e);
      review.phase = "review";
    }
  }

  const importChosen = () => run(review.chosen);
  function retry() {
    void run(review.prepareRetry());
  }
  async function cancel() {
    await api.packCancel(handle);
  }
  async function close() {
    await api.packClose(handle).catch(() => {});
    onClose();
  }

  const minutes = (m: number | null) => (m ? `${Math.round(m)} min` : "");
  const summary = $derived(review.report);
</script>

<section class="review">
  <h2>{plan.title}</h2>
  <p class="muted">
    {plan.language}{plan.level ? ` · ${plan.level}` : ""}{plan.topic ? ` · ${plan.topic}` : ""}{plan.createdBy ? ` · made by ${plan.createdBy}` : ""}
  </p>
  {#if plan.description}<p>{plan.description}</p>{/if}
  {#if !plan.languageInstalled}
    <p class="error">This pack is for “{plan.language}”, which is not installed. Install or enable that language first (Languages tab).</p>
  {/if}

  <div class="row tools">
    <button onclick={() => review.setAll(true)} disabled={review.phase !== "review"}>Select all</button>
    <button onclick={() => review.setAll(false)} disabled={review.phase !== "review"}>Select none</button>
    <span class="muted">{review.chosen.length} of {plan.items.length} chosen</span>
  </div>

  <ul class="items">
    {#each plan.items as i (i.id)}
      {@const st = review.itemState[i.id]}
      <li class:off={!selectable(i)}>
        <label>
          <input type="checkbox" checked={review.checked[i.id]} disabled={!selectable(i) || review.phase !== "review"} onchange={() => review.toggle(i.id)} />
          <span class="main">
            <strong>{i.title}</strong>
            <span class="muted">
              {i.kind}{i.domain ? ` · ${i.domain}` : ""}{i.license ? ` · ${i.license}` : ""}{i.words ? ` · ~${i.words} words` : ""}{i.minutes ? ` · ${minutes(i.minutes)}` : ""}{i.transcript ? " · transcript" : ""}
            </span>
            {#if statusLabel(i)}<span class="note" class:warn={i.status === "license-warning"}>{statusLabel(i)}</span>{/if}
          </span>
        </label>
        {#if st}
          <span class="state {st}">{st === "running" ? "importing…" : st}</span>
        {/if}
        {#if review.errors[i.id]}<p class="error small">{review.errors[i.id]}</p>{/if}
      </li>
    {/each}
  </ul>

  {#if plan.vocabTotal > 0}
    <label class="row">
      <input type="checkbox" bind:checked={review.seedVocab} disabled={review.phase !== "review"} />
      Add seed vocabulary ({plan.vocabTotal} words: {plan.vocab.new} new, {plan.vocab.fill} fill empty fields, {plan.vocab.existing} already have all fields). Known words keep their status.
    </label>
  {/if}

  {#if unfinished}
    <p>Importing {review.position.index} of {review.position.total}…</p>
    <button onclick={cancel}>Cancel</button>
  {:else if summary}
    <p>
      Imported {summary.imported.length}{summary.skipped.length ? `, skipped ${summary.skipped.length} already in the library` : ""}{summary.failed.length ? `, ${summary.failed.length} failed` : ""}{summary.cancelled ? " (cancelled)" : ""}{summary.vocab ? `. Vocabulary: ${summary.vocab.new} added, ${summary.vocab.fill} filled in` : ""}.
    </p>
  {/if}
  {#if langdataId}<LangData id={langdataId} />{:else if error}<p class="error">{error}</p>{/if}

  <div class="row actions">
    {#if review.phase === "review"}
      <button class="primary" onclick={importChosen} disabled={!review.canImport}>Import</button>
    {/if}
    {#if review.phase === "done" && review.failedIds.length > 0}
      <button class="primary" onclick={retry}>Retry {review.failedIds.length} failed</button>
    {/if}
    <button onclick={close} disabled={unfinished}>{review.phase === "done" ? "Close" : "Cancel"}</button>
  </div>
</section>

<style>
  .review {
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0.8rem;
    margin: 0.6rem 0 1rem;
    background: var(--panel);
  }
  .tools,
  .actions {
    margin: 0.5rem 0;
    flex-wrap: wrap;
  }
  .items {
    list-style: none;
    padding: 0;
    margin: 0 0 0.6rem;
  }
  .items li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.4rem;
    padding: 0.35rem 0;
    border-bottom: 1px solid var(--border);
  }
  .items label {
    display: flex;
    gap: 0.5rem;
    align-items: flex-start;
    flex: 1;
  }
  .main {
    display: flex;
    flex-direction: column;
  }
  .off {
    opacity: 0.75;
  }
  .note {
    font-size: 0.85rem;
  }
  .note.warn,
  .state.failed {
    color: var(--role-warn);
  }
  .state {
    font-size: 0.85rem;
  }
  .small {
    flex-basis: 100%;
    margin: 0;
    font-size: 0.85rem;
  }
</style>
