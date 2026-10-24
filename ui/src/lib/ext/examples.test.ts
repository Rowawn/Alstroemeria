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

describe("ja.furigana", () => {
  const settingsAlst = (settings: object) => vi.stubGlobal("alst", { settings: { get: async () => settings } });
  afterEach(() => vi.unstubAllGlobals());

  it.each([
    ["食べる", "たべる", [["食", "た"], ["べる"]]],
    ["召し上がる", "めしあがる", [["召", "め"], ["し"], ["上", "あ"], ["がる"]]],
    ["取り扱い", "とりあつかい", [["取", "と"], ["り"], ["扱", "あつか"], ["い"]]],
    ["お茶", "おちゃ", [["お"], ["茶", "ちゃ"]]],
    ["日本語", "にほんご", [["日本語", "にほんご"]]],
    ["行って", "いって", [["行", "い"], ["って"]]],
    ["読まされていた", "よまされていた", [["読", "よ"], ["まされていた"]]],
    ["ビール飲み", "びーるのみ", [["ビール"], ["飲", "の"], ["み"]]],
  ])("aligns %s", async (surface, reading, expected) => {
    const { align } = await loadJs("extensions/ja-furigana/main.js");
    const segs = align(surface, reading);
    expect(segs.map((s: any) => (s.rt ? [s.base, s.rt] : [s.base]))).toEqual(expected);
    expect(segs.map((s: any) => s.base).join("")).toBe(surface);
  });

  it("gives up cleanly when there is nothing to align or the reading doesn't fit the kana", async () => {
    const { align } = await loadJs("extensions/ja-furigana/main.js");
    expect(align("ひらがな", "ひらがな")).toBeNull();
    expect(align("食べる", "")).toBeNull();
    expect(align("食べる", "たのむ")).toBeNull();
    expect(align("食べる", "べる")).toBeNull(); // no reading left for the kanji
  });

  it("decorates by status setting, with a whole-word fallback when alignment fails", async () => {
    const { decorate } = await loadJs("extensions/ja-furigana/main.js");
    const t = (i: number, text: string, reading: string) => ({ i, text, isWord: true, ann: { "ja.reading": reading } });
    const tokens = [t(0, "食べる", "たべる"), t(1, "本", "ほん"), t(2, "漢字", "かんじ")];
    const units = [0, 1, 2].map((i) => ({ text: tokens[i].text, isWord: true, status: [0, 2, 99][i], tokenRange: [i, i] }));
    settingsAlst({ show: "unknown" });
    expect(Object.keys(await decorate({ units, tokens }))).toEqual(["0"]);
    settingsAlst({ show: "learning" });
    expect(Object.keys(await decorate({ units, tokens }))).toEqual(["0", "1"]);
    settingsAlst({ show: "all" });
    const all = await decorate({ units, tokens });
    expect(Object.keys(all)).toEqual(["0", "1", "2"]);
    expect(all[0]).toEqual([{ kind: "ruby", segments: [{ base: "食", rt: "た" }, { base: "べる" }] }]);
    settingsAlst({ show: "none" });
    expect(await decorate({ units, tokens })).toEqual({});
    settingsAlst({ show: "all" });
    const odd = [t(0, "食べる", "のむ")];
    expect((await decorate({ units: [units[0]], tokens: odd }))[0]).toEqual([{ kind: "ruby", segments: [{ base: "食べる", rt: "のむ" }] }]);
  });

  it("joins the segments of a multi-token unit", async () => {
    const { unitSegments } = await loadJs("extensions/ja-furigana/main.js");
    const segs = unitSegments([
      { text: "気", isWord: true, ann: { "ja.reading": "き" } },
      { text: "が", isWord: true, ann: { "ja.reading": "が" } },
      { text: "する", isWord: true, ann: { "ja.reading": "する" } },
    ]);
    expect(segs.map((s: any) => s.base).join("")).toBe("気がする");
    expect(segs[0]).toEqual({ base: "気", rt: "き" });
  });
});

describe("ja.pitch", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("counts morae and classifies patterns", async () => {
    const { moraCount, patternOf } = await loadJs("extensions/ja-pitch/main.js");
    expect(moraCount("きょう")).toBe(2);
    expect(moraCount("がっこう")).toBe(4);
    expect(moraCount("ラーメン")).toBe(4);
    expect([patternOf("さくら", 0), patternOf("さくら", 1), patternOf("さくら", 2), patternOf("さくら", 3), patternOf("はし", 2)]).toEqual([
      "heiban", "atamadaka", "nakadaka", "odaka", "odaka",
    ]);
  });

  async function load(rows: any[], display = "colour") {
    const calls: unknown[] = [];
    vi.stubGlobal("alst", {
      settings: { get: async () => ({ display }) },
      dict: { meta: async (_l: string, q: any) => (calls.push(q), rows.filter((r) => q.terms.includes(r.term))) },
    });
    vi.resetModules();
    return { mod: await loadJs("extensions/ja-pitch/main.js"), calls };
  }
  const row = (term: string, reading: string, position: number) => ({ term, reading, mode: "pitch", data: { reading, pitches: [{ position }] } });
  const tok = (i: number, text: string, reading: string, lemma = text, head?: string) => ({
    i, text, lemma, isWord: true, ann: { "ja.reading": reading, "ja.morphemes": [{ reading: head ?? reading }] },
  });

  it("annotates words in one batched call, choosing the row by reading, and decorates by pattern or badge", async () => {
    const { mod, calls } = await load([row("橋", "はし", 2), row("箸", "はし", 1), row("食べる", "たべる", 2)]);
    const tokens = [tok(0, "橋", "はし"), tok(1, "食べた", "たべた", "食べる", "たべ"), tok(2, "猫", "ねこ")];
    const out = await mod.annotate({ language: "ja", tokens });
    expect(calls).toHaveLength(1);
    expect(out[0]["ja.pitch"]).toMatchObject({ downstep: 2, pattern: "odaka", reading: "はし" });
    expect(out[1]["ja.pitch"]).toMatchObject({ downstep: 2, pattern: "nakadaka", reading: "たべる" });
    expect(out[2]).toBeUndefined();
    tokens.forEach((t: any) => (t.ann["ja.pitch"] = out[t.i]?.["ja.pitch"]));
    const units = tokens.map((t: any, i: number) => ({ isWord: true, tokenRange: [i, i] }));
    expect(await mod.decorate({ units, tokens })).toEqual({ 0: [{ kind: "color", role: "accent-odaka" }], 1: [{ kind: "color", role: "accent-nakadaka" }] });
    const b = await load([], "badge");
    expect(await b.mod.decorate({ units, tokens })).toEqual({ 0: [{ kind: "badge", text: "[2]" }], 1: [{ kind: "badge", text: "[2]" }] });
    const off = await load([], "off");
    expect(await off.mod.decorate({ units, tokens })).toEqual({});
  });

  it("the term editor section draws a graph, or tells the user to import a pitch dictionary", async () => {
    const { mod } = await load([row("橋", "はし", 2)]);
    const sections = await mod.lookup({ language: "ja", unit: { text: "橋", key: "橋", lemma: "橋" }, tokens: [tok(0, "橋", "はし")] });
    expect(sections[0].items).toEqual([{ kind: "pitch", reading: "はし", downstep: 2 }, { kind: "kv", label: "Pattern", text: "odaka [2]" }]);
    const empty = await load([]);
    const none = await empty.mod.lookup({ language: "ja", unit: { text: "猫", key: "猫", lemma: "猫" }, tokens: [tok(0, "猫", "ねこ")] });
    expect(none[0].items[0].text).toContain("Import a pitch dictionary");
  });
});

describe("addon.grammar-colors", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("colours by the language's role according to settings", async () => {
    vi.stubGlobal("alst", { settings: { get: async () => ({ verb: true, particle: false, noun: true }) } });
    const { decorate } = await loadJs("extensions/addon-grammar-colors/main.js");
    const tokens = [
      { ann: { "ja.pos": "verb" } }, { ann: { "ja.pos": "particle" } }, { ann: { "ja.pos": "noun" } }, { ann: {} }, { ann: { "ja.pos": "bogus" } },
    ];
    const units = tokens.map((_, i) => ({ isWord: true, tokenRange: [i, i] }));
    units.push({ isWord: false, tokenRange: [0, 0] });
    expect(await decorate({ language: "ja", units, tokens })).toEqual({ 0: [{ kind: "color", role: "verb" }], 2: [{ kind: "color", role: "noun" }] });
    expect(await decorate({ language: "ko", units, tokens })).toEqual({});
  });
});

describe("ko.hanja", () => {
  afterEach(() => vi.unstubAllGlobals());
  const tsv = ["공부\t工夫\t", "공부\t功夫\t", "학교\t學校\t", "학교\t學敎\t", ""].join("\n");
  const chars = ["學\t배울 학", "校\t학교 교", "工\t장인 공", "夫\t지아비 부", ""].join("\n");

  it("parses the TSV into words and per-character meanings", async () => {
    const { parse, candidates } = await loadJs("extensions/ko-hanja/main.js");
    const d = parse(tsv, chars);
    expect(d.chars.get("學")).toBe("배울 학");
    expect(candidates("학교", d)).toEqual([{ hanja: "學校", meaning: "" }, { hanja: "學敎", meaning: "" }]);
    expect(d.find("가")).toEqual([]);
    expect(d.find("힣")).toEqual([]);
    expect(parse("", "").find("학교")).toEqual([]);
    expect(candidates("공부하다", d).map((c: any) => c.hanja)).toEqual(["工夫", "功夫"]);
    expect(candidates("먹다", d)).toEqual([]);
  });

  it("the bundled data is sorted for binary search and finds known words", async () => {
    const { parse } = await loadJs("extensions/ko-hanja/main.js");
    const words = readFileSync(join(root, "extensions/ko-hanja/hanja.tsv"), "utf8");
    const d = parse(words, readFileSync(join(root, "extensions/ko-hanja/hanja-chars.tsv"), "utf8"));
    const keys = words.split("\n").filter(Boolean).map((l) => l.split("\t")[0]);
    for (let i = 1; i < keys.length; i++) if (keys[i - 1] > keys[i]) throw new Error(`unsorted at ${i}: ${keys[i - 1]} > ${keys[i]}`);
    expect(d.find("학교").map((r: any) => r.hanja)).toContain("學校");
    expect(d.find("공부").map((r: any) => r.hanja)).toContain("工夫");
    expect(d.chars.get("學")).toBeTruthy();
    for (const k of [keys[0], keys[keys.length - 1], keys[Math.floor(keys.length / 2)]]) expect(d.find(k).length).toBeGreaterThan(0);
  });

  it("annotates, shows characters in the term editor, and rubies only bare words when enabled", async () => {
    const { parse } = await loadJs("extensions/ko-hanja/main.js");
    vi.stubGlobal("fetch", async (u: URL) => new Response(String(u).endsWith("chars.tsv") ? chars : tsv));
    vi.stubGlobal("alst", { settings: { get: async () => ({ ruby: true }) } });
    vi.resetModules();
    const mod = await loadJs("extensions/ko-hanja/main.js");
    const tokens = [
      { i: 0, text: "학교", lemma: "학교", isWord: true, ann: {} },
      { i: 1, text: "공부했어요", lemma: "공부하다", isWord: true, ann: {} },
      { i: 2, text: "먹었다", lemma: "먹다", isWord: true, ann: {} },
    ];
    await mod.activate();
    const out = await mod.annotate({ tokens });
    expect(Object.keys(out)).toEqual(["0", "1"]);
    expect(out[0]["ko.hanja"]).toEqual({ hanja: "學校", alternatives: ["學敎"] });
    tokens.forEach((t: any) => (t.ann = out[t.i] ?? {}));
    const units = tokens.map((t: any, i: number) => ({ text: t.text, isWord: true, wordCount: 1, tokenRange: [i, i] }));
    expect(await mod.decorate({ units, tokens })).toEqual({ 0: [{ kind: "ruby", text: "學校" }] });
    const sec = await mod.lookup({ unit: { text: "학교", key: "학교", lemma: "학교" } });
    expect(sec[0].items).toEqual([
      { kind: "text", text: "學校" }, { kind: "kv", label: "學", text: "배울 학" }, { kind: "kv", label: "校", text: "학교 교" },
      { kind: "text", text: "學敎" }, { kind: "kv", label: "學", text: "배울 학" },
    ]);
    expect(parse).toBeTypeOf("function");
  });
});
