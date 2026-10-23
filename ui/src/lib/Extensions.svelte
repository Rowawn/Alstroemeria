<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Language } from "./api";
  import { extHost } from "./ext";
  import { extState } from "./ext/store.svelte";
  import type { ExtInfo, ExtensionList, PackageInfo, ResolvedView, SettingSchema } from "./ext/types";
  import { toast } from "./notify.svelte";

  let { languages }: { languages: Language[] } = $props();

  let list = $state<ExtensionList | null>(null);
  let language = $state(languages[0]?.id ?? "");
  let resolved = $state<ResolvedView | null>(null);
  let error = $state("");
  let expanded = $state<string | null>(null);
  let draft = $state<Record<string, unknown>>({});
  let pending = $state<{ file: File; info: PackageInfo } | null>(null);
  let busy = $state(false);

  async function refresh() {
    try {
      list = await api.extensions();
      resolved = language ? await api.resolveExtensions(language) : null;
      error = "";
    } catch (e) {
      error = String(e);
    }
  }

  onMount(refresh);
  $effect(() => {
    void language;
    void refresh();
  });

  const byId = $derived(new Map((list?.extensions ?? []).map((e) => [e.id, e])));
  const reasonFor = (id: string) => resolved?.disabled.find((d) => d.id === id)?.reason;
  const activeHere = (id: string) => resolved?.active.some((e) => e.id === id) ?? false;
  const appliesHere = (e: ExtInfo) => e.kind === "language" ? e.language === language : e.targets.includes("*") || e.targets.includes(language);

  /** Status line for an extension in the selected language. */
  function status(e: ExtInfo): { text: string; tone: "ok" | "warn" | "err" | "muted" } {
    void extState.version;
    const err = extHost.error(e.id);
    if (err) return { text: `Stopped: ${err.message}`, tone: "err" };
    if (!e.enabled) return { text: "Disabled", tone: "muted" };
    if (!appliesHere(e)) return { text: `Not for ${language || "this language"}`, tone: "muted" };
    if (e.languages.length && !e.languages.includes(language)) return { text: `Off for ${language}`, tone: "muted" };
    const reason = reasonFor(e.id);
    if (reason) return { text: `Inactive: ${reason}`, tone: "warn" };
    return activeHere(e.id) ? { text: "Active", tone: "ok" } : { text: "Inactive", tone: "muted" };
  }

  async function act(fn: () => Promise<unknown>, message?: string) {
    busy = true;
    try {
      await fn();
      if (message) toast(message);
    } catch (e) {
      toast(String(e), "error");
    }
    await refresh();
    busy = false;
  }

  const setEnabled = (e: ExtInfo, enabled: boolean) =>
    act(async () => {
      await api.setExtensionEnabled(e.id, enabled);
      if (!enabled) extHost.terminate(e.id);
      else extHost.clearError(e.id);
    });

  const toggleLanguage = (e: ExtInfo, lang: string, on: boolean) => {
    const targets = languages.filter((l) => e.targets.includes("*") || e.targets.includes(l.id)).map((l) => l.id);
    const current = e.languages.length ? e.languages : targets;
    const next = on ? [...new Set([...current, lang])] : current.filter((l) => l !== lang);
    // Every target selected means "all" (so newly added languages are included).
    return act(() => api.setExtensionLanguages(e.id, next.length === targets.length ? [] : next));
  };

  function openSettings(e: ExtInfo) {
    expanded = expanded === e.id ? null : e.id;
    draft = { ...e.settings };
  }

  const schemaOf = (e: ExtInfo): [string, SettingSchema][] => Object.entries(e.contributes.settings?.properties ?? {});

  async function saveSettings(e: ExtInfo) {
    await act(async () => {
      await api.setExtensionSettings(e.id, draft);
      extHost.settingsChanged(e.id, $state.snapshot(draft));
    }, "Settings saved");
  }

  async function uninstall(e: ExtInfo) {
    if (!confirm(`Uninstall ${e.name}? Its settings are kept in case you reinstall it.`)) return;
    await act(async () => {
      await api.uninstallExtension(e.id);
      extHost.terminate(e.id);
    }, `${e.name} uninstalled`);
  }

  async function chooseFile(ev: Event) {
    const input = ev.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      pending = { file, info: await api.inspectExtension(file) };
    } catch (e) {
      toast(`Can't install ${file.name}: ${e}`, "error");
    }
  }

  async function confirmInstall() {
    if (!pending) return;
    const { file, info } = pending;
    pending = null;
    await act(async () => {
      extHost.terminate(info.id);
      extHost.clearError(info.id);
      await api.installExtension(file);
    }, `${info.name} installed`);
  }

  async function move(id: string, dir: -1 | 1) {
    if (!resolved) return;
    const order = [...resolved.decoratorOrder];
    const i = order.indexOf(id);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= order.length) return;
    [order[i], order[j]] = [order[j], order[i]];
    await act(() => api.setDecoratorOrder(language, order));
  }

  const nameOf = (id: string) => byId.get(id)?.name ?? id;
</script>

<div class="row head">
  <h2>Extensions</h2>
  <label class="row">
    Language
    <select bind:value={language}>
      {#each languages as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
    </select>
  </label>
  <label class="install">
    <input type="file" accept=".alext,.zip" onchange={chooseFile} hidden />
    <span class="btn">Install .alext…</span>
  </label>
</div>

{#if error}<p class="error">{error}</p>{/if}

{#if pending}
  <div class="card prompt">
    <h3>Install {pending.info.name} {pending.info.version}?</h3>
    <p class="muted">
      {pending.info.description ?? ""}
      {#if pending.info.author}By {pending.info.author}.{/if} License: {pending.info.license}.
      {#if pending.info.existingVersion}This replaces version {pending.info.existingVersion}.{/if}
    </p>
    {#if pending.info.permissions.length}
      <p><strong>This extension will be able to:</strong></p>
      <ul>
        {#each pending.info.permissions as p (p.id)}
          <li class:warn={p.warning}>{p.warning ? "⚠ " : ""}{p.text}</li>
        {/each}
      </ul>
    {:else}
      <p>It asks for no special access: it can only work on text it is shown and use its own storage.</p>
    {/if}
    <p class="muted small">
      Extensions run in a sandbox without direct network or file access, but they are code you are choosing to run. Only
      install ones you trust.
    </p>
    <div class="row">
      <button class="primary" onclick={confirmInstall}>Install</button>
      <button onclick={() => (pending = null)}>Cancel</button>
    </div>
  </div>
{/if}

{#if resolved && resolved.providerChoices.length}
  <div class="card">
    <h3>Providers for {language}</h3>
    <p class="muted small">Several extensions provide the same capability; exactly one is used.</p>
    {#each resolved.providerChoices as c (c.capability)}
      <label class="row">
        <code>{c.capability}</code>
        <select value={c.chosen} onchange={(e) => act(() => api.setProvider(language, c.capability, e.currentTarget.value))}>
          {#each c.providers as p (p)}<option value={p}>{nameOf(p)}</option>{/each}
        </select>
      </label>
    {/each}
  </div>
{/if}

{#if resolved && resolved.decoratorOrder.length > 1}
  <div class="card">
    <h3>Decorator order for {language}</h3>
    <p class="muted small">Earlier decorators win when two set the same colour, underline or ruby text.</p>
    {#each resolved.decoratorOrder as id, i (id)}
      <div class="row">
        <span>{i + 1}. {nameOf(id)}</span>
        <button disabled={i === 0 || busy} onclick={() => move(id, -1)} aria-label="Move up">↑</button>
        <button disabled={i === resolved.decoratorOrder.length - 1 || busy} onclick={() => move(id, 1)} aria-label="Move down">↓</button>
      </div>
    {/each}
  </div>
{/if}

{#each list?.extensions ?? [] as e (e.id)}
  {@const st = status(e)}
  <div class="card">
    <div class="row top">
      <strong>{e.name}</strong>
      <span class="muted">{e.version}</span>
      <span class="tag">{e.kind}</span>
      <span class="tag">{e.source}</span>
      <span class="tag">{e.runtime}</span>
      <span class="grow"></span>
      <span class={`status ${st.tone}`}>{st.text}</span>
    </div>
    {#if e.description}<p class="muted">{e.description}</p>{/if}
    <div class="muted small">
      <code>{e.id}</code> · {e.license}{e.author ? ` · ${e.author}` : ""}
      {#if e.kind === "language"}· language <code>{e.language}</code>{:else}· for {e.targets.join(", ")}{/if}
      {#if e.platforms}· {e.platforms.join(", ")} only{/if}
    </div>
    {#if e.permissions.length}
      <ul class="perms">
        {#each e.permissions as p (p.id)}<li class:warn={p.warning}>{p.warning ? "⚠ " : ""}{p.text}</li>{/each}
      </ul>
    {/if}
    {#if e.unsupported.length}
      <p class="muted small">Ignored in this version: {e.unsupported.join(", ")}.</p>
    {/if}
    {#if e.provides.length || e.requires.length}
      <p class="muted small">
        {#if e.provides.length}Provides {e.provides.join(", ")}.{/if}
        {#if e.requires.length}Requires {e.requires.join(", ")}.{/if}
      </p>
    {/if}
    <div class="row">
      <label class="row">
        <input type="checkbox" checked={e.enabled} disabled={busy} onchange={(ev) => setEnabled(e, ev.currentTarget.checked)} />
        Enabled
      </label>
      {#if extHost.error(e.id)}
        <button onclick={() => (extHost.clearError(e.id), refresh())}>Retry</button>
      {/if}
      {#if schemaOf(e).length}<button onclick={() => openSettings(e)}>Settings</button>{/if}
      {#if e.source === "installed"}<button onclick={() => uninstall(e)}>Uninstall</button>{/if}
    </div>
    {#if e.kind !== "language" && !e.targets.includes("*") ? e.targets.length > 1 : e.kind !== "language"}
      <div class="row small">
        <span class="muted">Use for:</span>
        {#each languages.filter((l) => e.targets.includes("*") || e.targets.includes(l.id)) as l (l.id)}
          <label class="row">
            <input
              type="checkbox"
              checked={e.languages.length === 0 || e.languages.includes(l.id)}
              disabled={busy}
              onchange={(ev) => toggleLanguage(e, l.id, ev.currentTarget.checked)}
            />
            {l.name}
          </label>
        {/each}
      </div>
    {/if}
    {#if expanded === e.id}
      <form
        class="settings"
        onsubmit={(ev) => (ev.preventDefault(), saveSettings(e))}
      >
        {#each schemaOf(e) as [key, schema] (key)}
          <label class="field">
            <span>{schema.title ?? key}</span>
            {#if schema.type === "boolean"}
              <input type="checkbox" checked={draft[key] === true} onchange={(ev) => (draft[key] = ev.currentTarget.checked)} />
            {:else if schema.enum}
              <select value={String(draft[key] ?? "")} onchange={(ev) => (draft[key] = ev.currentTarget.value)}>
                {#each schema.enum as opt (opt)}<option value={opt}>{opt}</option>{/each}
              </select>
            {:else if schema.type === "number"}
              <input type="number" value={Number(draft[key] ?? 0)} oninput={(ev) => (draft[key] = Number(ev.currentTarget.value))} />
            {:else}
              <input value={String(draft[key] ?? "")} oninput={(ev) => (draft[key] = ev.currentTarget.value)} />
            {/if}
            {#if schema.description}<span class="muted small">{schema.description}</span>{/if}
          </label>
        {/each}
        <div class="row">
          <button class="primary" type="submit" disabled={busy}>Save</button>
          <button type="button" onclick={() => (expanded = null)}>Close</button>
        </div>
      </form>
    {/if}
  </div>
{/each}

{#if list?.errors.length}
  <div class="card">
    <h3>Couldn't load</h3>
    {#each list.errors as e (e.location)}
      <p class="error small"><code>{e.location}</code>: {e.message}</p>
    {/each}
  </div>
{/if}

<style>
  .head {
    justify-content: space-between;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0.7rem 0.9rem;
    margin: 0.6rem 0;
  }
  .card p {
    margin: 0.3rem 0;
  }
  .top {
    gap: 0.5rem;
  }
  .grow {
    flex: 1;
  }
  .tag {
    font-size: 0.72rem;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0 0.45rem;
    color: var(--muted);
  }
  .status {
    font-size: 0.85rem;
  }
  .status.ok {
    color: #2e8b57;
  }
  .status.warn {
    color: #b8860b;
  }
  .status.err {
    color: #d03050;
  }
  .status.muted {
    color: var(--muted);
  }
  .small {
    font-size: 0.8rem;
  }
  ul {
    margin: 0.3rem 0;
    padding-left: 1.2rem;
  }
  li.warn {
    color: #b8860b;
  }
  .btn {
    display: inline-block;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--panel);
    cursor: pointer;
  }
  .prompt {
    border-color: var(--accent);
  }
  .settings {
    margin-top: 0.6rem;
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    align-items: flex-start;
  }
  code {
    font-size: 0.85em;
  }
</style>
