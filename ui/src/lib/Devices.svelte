<script lang="ts">
  import { onMount } from "svelte";
  import { sync, syncApi, type MediaPolicy, type SyncPeer } from "./sync.svelte";

  let nameDraft = $state("");
  let qr = $state<{ svg: string; text: string; expiresAt: number } | null>(null);
  let busy = $state("");
  let removing = $state("");
  let error = $state("");
  let notice = $state("");
  let manualAddr = $state("");
  let pasted = $state("");
  let now = $state(Date.now());
  const isAndroid = typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);

  const o = $derived(sync.overview);
  const fp = (s: string) => s.replace(/(.{4})/g, "$1 ").trim();

  onMount(() => {
    void sync.refresh().then(() => (nameDraft = sync.overview?.deviceName ?? ""));
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => {
      clearInterval(timer);
      if (qr) void syncApi.pairStop();
    };
  });

  // The pairing screen closes itself when this device gets paired (the other side did the work).
  $effect(() => {
    if (sync.paired && qr) {
      notice = `Paired with ${sync.paired.name}.`;
      qr = null;
    }
  });
  $effect(() => {
    if (qr && qr.expiresAt < now) qr = null;
  });

  async function run(label: string, f: () => Promise<unknown>) {
    error = "";
    notice = "";
    busy = label;
    try {
      await f();
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  const saveName = () => run("name", () => syncApi.setDeviceName(nameDraft));

  async function startPairing() {
    sync.paired = null;
    await run("pairing", async () => (qr = await syncApi.pairStart()));
  }

  async function cancelPairing() {
    qr = null;
    await syncApi.pairStop();
  }

  const pairWith = (siteId: string | null, address: string | null) =>
    run("pair", async () => {
      const p = await syncApi.pairByCode(siteId, address);
      notice = `Paired with ${p.name}.`;
    });

  const pairQr = (text: string) =>
    run("pair", async () => {
      const p = await syncApi.pairByQr(text);
      notice = `Paired with ${p.name}.`;
      pasted = "";
    });

  async function scan() {
    await run("scan", async () => {
      const { scan, Format, requestPermissions } = await import("@tauri-apps/plugin-barcode-scanner");
      if ((await requestPermissions()) !== "granted") throw new Error("Camera permission is needed to scan the code.");
      const r = await scan({ windowed: false, formats: [Format.QRCode] });
      const p = await syncApi.pairByQr(r.content);
      notice = `Paired with ${p.name}.`;
    });
  }

  async function setPolicy(p: SyncPeer, patch: Partial<MediaPolicy>) {
    await run("policy", () => syncApi.setMediaPolicy(p.siteId, { ...p.mediaPolicy, ...patch }));
  }

  const ago = (ms: number | null) => {
    if (!ms) return "never synced";
    const s = Math.max(0, Math.round((now - ms) / 1000));
    return s < 60 ? "synced just now" : s < 3600 ? `synced ${Math.round(s / 60)} min ago` : `synced ${new Date(ms).toLocaleString()}`;
  };
  const secondsLeft = $derived(qr ? Math.max(0, Math.round((qr.expiresAt - now) / 1000)) : 0);
</script>

<h1>Devices</h1>
<p class="muted">
  Paired devices on the same Wi-Fi sync your library, words and settings directly, with no server or account.
</p>

{#if !sync.available || !o}
  <p class="muted">Sync is not available on this device.</p>
{:else}
  <section>
    <h2>This device</h2>
    <form class="row" onsubmit={(e) => { e.preventDefault(); void saveName(); }}>
      <input bind:value={nameDraft} aria-label="Device name" />
      <button type="submit" disabled={busy === "name" || nameDraft.trim() === o.deviceName}>Save name</button>
    </form>
    <p class="muted">Fingerprint <code>{fp(o.fingerprint)}</code> · port {o.port}</p>
  </section>

  {#if sync.code}
    <div class="code" role="dialog" aria-label="Compare codes">
      <p>Check that <strong>{sync.code.peerName}</strong> shows the same code:</p>
      <p class="big">{sync.code.code}</p>
      <div class="row">
        <button class="primary" onclick={() => syncApi.pairAnswer(true)}>Codes match</button>
        <button onclick={() => syncApi.pairAnswer(false)}>They differ</button>
      </div>
    </div>
  {/if}

  <section>
    <h2>Paired devices</h2>
    {#if o.peers.length === 0}
      <p class="muted">No devices paired yet.</p>
    {/if}
    {#each o.peers as p (p.siteId)}
      <div class="peer">
        <div class="row">
          <strong>{p.name}</strong>
          <span class="muted">{p.syncing ? "syncing…" : p.online ? "online" : "not seen"} · {ago(p.lastSeen)}</span>
          <button onclick={() => syncApi.syncNow(p.siteId)} disabled={p.syncing}>Sync now</button>
          {#if removing === p.siteId}
            <span class="muted">Stop syncing with {p.name}?</span>
            <button
              onclick={() => {
                removing = "";
                void run("unpair", () => syncApi.unpair(p.siteId));
              }}>Remove</button
            >
            <button onclick={() => (removing = "")}>Cancel</button>
          {:else}
            <button onclick={() => (removing = p.siteId)}>Remove</button>
          {/if}
        </div>
        <details>
          <summary>Media and address</summary>
          <p class="muted">Files this device downloads from {p.name} automatically:</p>
          <div class="row">
            <label><input type="checkbox" checked={p.mediaPolicy.audio} onchange={(e) => setPolicy(p, { audio: e.currentTarget.checked })} /> Audio</label>
            <label><input type="checkbox" checked={p.mediaPolicy.video} onchange={(e) => setPolicy(p, { video: e.currentTarget.checked })} /> Video</label>
            <label><input type="checkbox" checked={p.mediaPolicy.images} onchange={(e) => setPolicy(p, { images: e.currentTarget.checked })} /> Images</label>
            <label>
              Largest file (MB, 0 = no limit)
              <input type="number" min="0" value={p.mediaPolicy.maxMb} class="num" onchange={(e) => setPolicy(p, { maxMb: Math.max(0, Math.floor(+e.currentTarget.value || 0)) })} />
            </label>
          </div>
          <p class="muted">Dictionaries and extensions always sync. A file left out here can still be downloaded when you open it.</p>
          <form
            class="row"
            onsubmit={(e) => {
              e.preventDefault();
              const v = new FormData(e.currentTarget).get("addr")?.toString() ?? "";
              void run("addr", () => syncApi.setPeerAddr(p.siteId, v));
            }}
          >
            <input name="addr" placeholder="192.168.1.20:4000" value={p.addr ? `${p.addr}:${p.port}` : ""} />
            <button type="submit">Set address</button>
            <span class="muted">Used when the device isn't found automatically.</span>
          </form>
        </details>
      </div>
    {/each}
    {#if o.peers.length > 1}
      <button onclick={() => syncApi.syncNow()}>Sync all</button>
    {/if}
  </section>

  <section>
    <h2>Pair a new device</h2>
    {#if qr}
      <p>
        On the other device, open <em>Devices</em> and
        {#if isAndroid}scan{:else}scan this code with the phone, or pick this device under “Nearby devices”{/if}.
        Pairing mode ends in {secondsLeft}s.
      </p>
      <div class="qr">{@html qr.svg}</div>
      <button onclick={cancelPairing}>Cancel</button>
    {:else}
      <div class="row">
        <button class="primary" onclick={startPairing} disabled={busy === "pairing"}>Pair new device</button>
        {#if isAndroid}<button onclick={scan} disabled={busy === "scan"}>Scan QR code</button>{/if}
      </div>
      <h3>Nearby devices in pairing mode</h3>
      {#if o.nearby.filter((d) => d.pairing).length === 0}
        <p class="muted">None found. Tap “Pair new device” on the other device first.</p>
      {/if}
      {#each o.nearby.filter((d) => d.pairing) as d (d.siteId)}
        <div class="row">
          <strong>{d.name}</strong>
          <span class="muted">{fp(d.fp)}</span>
          <button onclick={() => pairWith(d.siteId, null)} disabled={busy === "pair"}>Pair</button>
        </div>
      {/each}
      <details>
        <summary>Enter an address or paste a code</summary>
        <form class="row" onsubmit={(e) => { e.preventDefault(); void pairWith(null, manualAddr); }}>
          <input bind:value={manualAddr} placeholder="192.168.1.20:4000" />
          <button type="submit" disabled={busy === "pair" || !manualAddr.trim()}>Pair by address</button>
        </form>
        <form class="row" onsubmit={(e) => { e.preventDefault(); void pairQr(pasted); }}>
          <input bind:value={pasted} placeholder="Pairing code text from the QR" />
          <button type="submit" disabled={busy === "pair" || !pasted.trim()}>Pair with pasted code</button>
        </form>
      </details>
    {/if}
  </section>

  {#if o.assets}
    <p class="muted">Downloading files: {o.assets[0]} of {o.assets[1]}</p>
  {/if}
  {#if o.status.state === "error" && o.status.message}<p class="error">{o.status.message}</p>{/if}
{/if}
{#if error}<p class="error">{error}</p>{/if}
{#if notice}<p class="ok">{notice}</p>{/if}

<style>
  section {
    margin-bottom: 1.5rem;
  }
  .peer {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
    margin-bottom: 0.5rem;
    background: var(--panel);
  }
  .code {
    border: 2px solid var(--accent);
    border-radius: 8px;
    padding: 0.75rem 1rem;
    background: var(--panel);
  }
  .big {
    font-size: 2.2rem;
    letter-spacing: 0.3em;
    font-variant-numeric: tabular-nums;
    margin: 0.25rem 0 0.75rem;
  }
  .qr {
    width: 240px;
    max-width: 100%;
    background: #fff;
    padding: 4px;
    border-radius: 6px;
    margin: 0.5rem 0;
  }
  .qr :global(svg) {
    width: 100%;
    height: auto;
    display: block;
  }
  .num {
    width: 5rem;
  }
  .ok {
    color: var(--role-adjective);
  }
</style>
