// End-to-end checks of the first-party examples without a browser: the real compiled WASM through the real shim,
// and the density add-on's logic against a fake `alst`.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../../..");
// Plain JS files outside the UI project: loaded by path so the type checker doesn't follow them.
const loadJs = (rel: string) => import(/* @vite-ignore */ join(root, rel));
const wasmPath = join(root, "extensions/es-morph-hints/ext.wasm");

const tok = (i: number, text: string, isWord = true, ann = {}) => ({ i, text, isWord, norm: text.toLowerCase(), start: i, end: i + 1, sentence: 0, ann });

describe.skipIf(!existsSync(wasmPath))("es.morph-hints (Rust → WASM through wasm-shim.js)", () => {
  async function load() {
    vi.stubGlobal("fetch", async () => new Response(readFileSync(wasmPath), { headers: { "content-type": "application/wasm" } }));
    const { createWasmExtension } = await loadJs("sdk/wasm-shim.js");
    const ext = createWasmExtension(new URL("file:///ext.wasm"));
    await ext.activate();
    return ext;
  }
  afterEach(() => vi.unstubAllGlobals());

  it("annotates words by suffix and ignores non-words", async () => {
    const ext = await load();
    const tokens = [tok(0, "Estoy"), tok(1, " ", false), tok(2, "corriendo"), tok(3, " "), tok(4, "rápidamente"), tok(5, " "), tok(6, "hablar"), tok(7, " "), tok(8, "comido"), tok(9, " "), tok(10, "casa")];
    const out = await ext.annotate({ language: "es", tokens });
    expect(out).toEqual({
      2: { "es.morph": "gerund" },
      4: { "es.morph": "adverb" },
      6: { "es.morph": "infinitive" },
      8: { "es.morph": "participle" },
    });
  });

  it("decorates with roles, styles and tooltips, honouring settings", async () => {
    const ext = await load();
    const tokens = [tok(0, "corriendo", true, { "es.morph": "gerund" }), tok(1, "hablar", true, { "es.morph": "infinitive" })];
    const units = [0, 1].map((i) => ({ text: tokens[i].text, isWord: true, wordCount: 1, tokenRange: [i, i] }));
    const all = await ext.decorate({ language: "es", units, tokens, settings: { infinitives: true, gerunds: true } });
    expect(all[0]).toEqual([{ kind: "underline", role: "verb", style: "dotted" }, { kind: "tooltip", text: "gerund" }]);
    expect(all[1]).toEqual([{ kind: "underline", role: "verb", style: "solid" }, { kind: "tooltip", text: "infinitive" }]);
    const some = await ext.decorate({ language: "es", units, tokens, settings: { gerunds: false } });
    expect(Object.keys(some)).toEqual(["1"]);
  });

  it("copes with unicode and many tokens, and with input that isn't what it expects", async () => {
    const ext = await load();
    const tokens = Array.from({ length: 5000 }, (_, i) => tok(i, i % 2 ? "ñandú" : "cantando"));
    const out = await ext.annotate({ language: "es", tokens });
    expect(Object.keys(out)).toHaveLength(2500);
    expect(await ext.annotate({ nonsense: true })).toBeUndefined();
    expect(await (ext.commands as any).anything({})).toBeUndefined();
  });
});

describe("addon.density", () => {
  async function load(store: Record<string, unknown> = {}) {
    const calls: [string, unknown][] = [];
    vi.stubGlobal("alst", {
      storage: { get: async (k: string) => store[k], set: async (k: string, v: unknown) => void (store[k] = v) },
      ui: { toast: async (m: string) => void calls.push(["toast", m]), copyText: async (t: string) => void calls.push(["copy", t]) },
    });
    vi.resetModules();
    const mod = await loadJs("extensions/addon-density/main.js");
    return { mod, calls, store };
  }
  afterEach(() => vi.unstubAllGlobals());

  const unit = (key: string, status: number | null, isWord = true) => ({ text: key, isWord, key: isWord ? key : null, status, wordCount: 1 });
  const units = [unit("hola", 99), unit(" ", null, false), unit("gato", 0), unit("perro", 0), unit("gato", 0), unit("casa", 3)];

  it("computes unknown percentage and the most frequent unknown words, without hints", async () => {
    const { mod } = await load();
    expect(mod.decorate({ language: "es", units })).toEqual({});
    const stats = (await mod.onPanelMessage("density", { type: "stats" })) as any;
    expect(stats).toMatchObject({ language: "es", total: 5, unknown: 3, top: [{ key: "gato", count: 2 }, { key: "perro", count: 1 }] });
    expect(await mod.onPanelMessage("density", { type: "other" })).toBeNull();
  });

  it("records part.read events per language and keeps the history bounded", async () => {
    const { mod, store } = await load();
    for (let i = 0; i < 60; i++) await mod.onEvent({ type: "part.read", language: "es", ts: i, payload: { words_read: 100, unknown_count: i } });
    await mod.onEvent({ type: "term.status", language: "es", ts: 1, payload: {} });
    await mod.onEvent({ type: "part.read", language: null, ts: 1, payload: {} });
    expect((store["history:es"] as unknown[]).length).toBe(50);
    expect((store["history:es"] as any[]).at(-1).unknown).toBe(59);
  });

  it("copy-unknown command copies the words and says so", async () => {
    const { mod, calls } = await load();
    await mod.commands["copy-unknown"]();
    expect(calls).toEqual([["toast", "No unknown words in this part."]]);
    mod.decorate({ language: "es", units });
    await mod.commands["copy-unknown"]();
    expect(calls.slice(1)).toEqual([["copy", "gato\nperro"], ["toast", "Copied 2 unknown words."]]);
  });
});
