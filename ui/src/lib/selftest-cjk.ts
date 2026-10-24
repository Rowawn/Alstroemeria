// On-device verification of Japanese and Korean (roadmap M5). Runs when the `selftest` file in the app data folder
// starts with `cjk`. Drives the real webview: dictionary import with rules and meta banks, grouped words, furigana,
// grammar colours, pitch colours, conjugated-verb lookup, the Korean hanja popup. Results go to the app log through
// `selftest_report`. Not part of normal use.

import { invoke } from "@tauri-apps/api/core";
import { api, type Language } from "./api";
import { extHost, extUrl } from "./ext";
import { makeZip } from "./ext/selftest";

const report = (name: string, ok: boolean, detail = "") => invoke("selftest_report", { name, ok, detail: String(detail).slice(0, 700) });

async function check(name: string, fn: () => Promise<string | void>) {
  try {
    await report(name, true, (await fn()) ?? "");
  } catch (e) {
    await report(name, false, e instanceof Error ? e.message : String(e));
  }
}

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(what: string, fn: () => T | null | undefined | false, ms = 15000): Promise<T> {
  const t0 = Date.now();
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

/** Text of a word span without ruby annotations. */
const plain = (el: Element) => [...el.childNodes].map((n) => (n instanceof Element ? (n.tagName === "RUBY" ? n.firstChild?.textContent : n.textContent) : n.textContent)).join("");
const words = () => [...document.querySelectorAll<HTMLElement>(".w")];
const wordTexts = () => words().map(plain);
const click = (el: Element) => el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
const editor = () => document.querySelector<HTMLElement>("aside.editor");

const JA_DICT = {
  "index.json": JSON.stringify({ title: "Fixture JA", format: 3, revision: "selftest" }),
  "term_bank_1.json": JSON.stringify([
    ["食べる", "たべる", "v1", "v1", 10, ["to eat"], 1, ""],
    ["猫", "ねこ", "n", "", 10, ["cat"], 2, ""],
    ["橋", "はし", "n", "", 10, ["bridge"], 3, ""],
    ["お茶", "おちゃ", "n", "", 10, ["tea"], 4, ""],
    ["飲む", "のむ", "v5", "v5m", 10, ["to drink"], 5, ""],
    ["勉強", "べんきょう", "n", "vs", 10, ["study"], 6, ""],
    ["学生", "がくせい", "n", "", 10, ["student"], 7, ""],
    ["渡る", "わたる", "v5", "v5r", 10, ["to cross"], 8, ""],
  ]),
  "term_meta_bank_1.json": JSON.stringify([
    ["橋", "pitch", { reading: "はし", pitches: [{ position: 2 }] }],
    ["猫", "pitch", { reading: "ねこ", pitches: [{ position: 1 }] }],
    ["お茶", "pitch", { reading: "おちゃ", pitches: [{ position: 0 }] }],
    ["食べる", "pitch", { reading: "たべる", pitches: [{ position: 2 }] }],
    ["橋", "freq", 900],
  ]),
};

const JA_TEXT = "食べさせられた猫は、昨日、お茶を飲んでいた。勉強している学生が橋を渡った。";
const KO_TEXT = "학교에서 친구들이 공부했어요. 학생이에요. 예뻐요.";

export async function run(languages: Language[], _mode: string, nav: { openPart: (id: string) => void; closePart: () => void }) {
  const lang = (id: string) => languages.find((l) => l.id === id);

  await check("languages: ja and ko are installed with lemma keys; ja has scan lookup, ko does not", async () => {
    const ja = lang("ja");
    const ko = lang("ko");
    assert(ja && ko, "ja/ko missing");
    assert(ja.lemmaKeys && ja.spaceless && ja.scanLookup, JSON.stringify(ja));
    assert(ko.lemmaKeys && !ko.spaceless && !ko.scanLookup, JSON.stringify(ko));
  });

  await check("import a rules + pitch/frequency dictionary", async () => {
    const info = await api.importDictionary("ja", makeZip(JA_DICT));
    assert(info.rulesImported && info.entryCount === 8 && info.metaCount === 5, JSON.stringify(info));
    return JSON.stringify(info);
  });

  await check("lookup_scan: conjugated verb with reasons", async () => {
    const r = await api.lookupScan("ja", "食べさせられた猫は");
    assert(r[0].matched === "食べさせられた" && r[0].lemma === "食べる", JSON.stringify(r[0]));
    assert(r[0].reasons.join(", ") === "causative, passive, past", r[0].reasons.join(", "));
    const p = await api.dictMeta("ja", "橋", "pitch");
    assert(p[0].data.pitches[0].position === 2, JSON.stringify(p));
    return `${r[0].matched} → ${r[0].lemma} (${r[0].reasons.join(", ")})`;
  });

  let jaPart = "";
  await check("Japanese article: grouped words, furigana, grammar colours, pitch colours", async () => {
    const w = await api.createWork("ja", "M5 日本語", JA_TEXT);
    jaPart = (await api.parts(w.id))[0].id;
    nav.openPart(jaPart);
    await until("reader words", () => words().length > 5);
    // let the add-ons finish (rubies appear when decorators have run)
    await until("furigana", () => document.querySelector(".w ruby rt"), 20000);
    const t = wordTexts();
    for (const expect of ["食べさせられた", "飲んでいた", "勉強している"]) {
      assert(t.some((x) => x.includes(expect)), `no grouped word '${expect}' in ${JSON.stringify(t)}`);
    }
    const ruby = [...document.querySelectorAll(".w ruby")].map((r) => `${r.firstChild?.textContent}/${r.querySelector("rt")?.textContent}`);
    assert(ruby.includes("食/た"), `食/た missing in furigana ${JSON.stringify(ruby)}`);
    const coloured = (role: string) => words().filter((e) => e.getAttribute("style")?.includes(`--role-${role}`)).length;
    assert(coloured("verb") >= 2 && coloured("particle") >= 3, `grammar colours: verb ${coloured("verb")}, particle ${coloured("particle")}`);
    assert(coloured("accent-atamadaka") >= 1 || coloured("accent-odaka") >= 1, "pitch colours missing");
    return `words ${JSON.stringify(t.slice(0, 12))}; furigana ${JSON.stringify(ruby.slice(0, 8))}; verb ${coloured("verb")} particle ${coloured("particle")}; pitch odaka ${coloured("accent-odaka")} atamadaka ${coloured("accent-atamadaka")} heiban ${coloured("accent-heiban")}`;
  });

  await check("Japanese: click a conjugated verb → lemma, seen as, reasons, pitch graph", async () => {
    const el = words().find((e) => plain(e).includes("食べさせられた"));
    assert(el, "word not found");
    click(el);
    const ed = await until("term editor", () => editor());
    await until("scan results", () => ed.textContent?.includes("causative, passive, past"));
    await until("lemma heading", () => ed.querySelector("h3")?.textContent === "食べる");
    assert(ed.textContent?.includes("seen as") && ed.textContent.includes("食べさせられた"), "seen as missing");
    assert(ed.textContent?.includes("to eat"), "definition missing");
    await until("pitch graph", () => ed.querySelector("svg"), 6000).catch(() => {
      throw new Error(`no pitch graph; editor text: ${ed.textContent?.replace(/\s+/g, " ")}`);
    });
    return ed.textContent?.replace(/\s+/g, " ").slice(0, 300);
  });

  await check("Japanese: status change applies to every inflection of the lemma", async () => {
    const ed = editor();
    assert(ed, "no editor");
    [...ed.querySelectorAll<HTMLButtonElement>(".statuses button")].find((b) => b.textContent === "5")?.click();
    await until("known", () => words().find((e) => plain(e).includes("食べさせられた"))?.classList.contains("s5"));
    const other = await api.createWork("ja", "M5 食べた", "昨日ケーキを食べた。");
    nav.closePart();
    nav.openPart((await api.parts(other.id))[0].id);
    await until("second reader", () => words().some((e) => plain(e) === "食べた"));
    assert(words().find((e) => plain(e) === "食べた")?.classList.contains("s5"), "食べた not status 5");
    nav.closePart();
    nav.openPart(jaPart);
    await until("reader again", () => words().length > 5);
  });

  await check("ko.hanja data is served over alstext://", async () => {
    const t0 = Date.now();
    const res = await fetch(extUrl("ko.hanja", "hanja.tsv"));
    const text = await res.text();
    assert(res.ok && text.length > 1_000_000, `status ${res.status}, ${text.length} chars`);
    return `${res.status}, ${text.length} chars in ${Date.now() - t0} ms`;
  });

  let koPart = "";
  await check("Korean article: particles split, verb lemma in the editor, hanja popup", async () => {
    nav.closePart();
    const w = await api.createWork("ko", "M5 한국어", KO_TEXT);
    koPart = (await api.parts(w.id))[0].id;
    nav.openPart(koPart);
    await until("reader words", () => words().length > 5);
    const t = wordTexts();
    for (const expect of ["학교", "에서", "친구들", "이", "공부했어요", "학생", "이에요"]) assert(t.includes(expect), `no word '${expect}' in ${JSON.stringify(t)}`);
    click(words().find((e) => plain(e) === "공부했어요")!);
    const ed = await until("term editor", () => editor());
    await until("lemma heading", () => ed.querySelector("h3")?.textContent === "공부하다");
    const t0 = Date.now();
    await until("hanja section", () => ed.textContent?.includes("Hanja") && ed.textContent.includes("工夫"), 120000).catch((e) => {
      throw new Error(`${e.message}; ko.hanja status ${extHost.status("ko.hanja")} ${JSON.stringify(extHost.error("ko.hanja"))}`);
    });
    await report("hanja data load time", true, `${Date.now() - t0} ms after the editor opened`);
    return `words ${JSON.stringify(t)}; editor: ${ed.textContent?.replace(/\s+/g, " ").slice(0, 240)}`;
  });

  await check("Korean: no conjugation file, lookup by tokenizer lemma", async () => {
    const tv = await api.openTerm("ko", "예뻐요", false);
    assert(tv.text === "예쁘다" && tv.key === "예쁘다" && tv.seenAs === "예뻐요", JSON.stringify(tv));
  });

  await check("timing: 5,000-character parts (SPEC §16.1)", async () => {
    const out: string[] = [];
    for (const [id, base] of [["ja", JA_TEXT + "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。"], ["ko", KO_TEXT + " 저는 어제 친구와 함께 도서관에 갔습니다."]] as const) {
      let text = "";
      while (text.length < 5000) text += base;
      const w = await api.createWork(id, `timing ${id}`, text);
      const part = (await api.parts(w.id))[0].id;
      const c0 = performance.now();
      await api.partView(part); // cold: loads the dictionary, tokenizes, fills local_token_cache
      const cold = Math.round(performance.now() - c0);
      const times: number[] = [];
      for (let i = 0; i < 3; i++) {
        const t0 = performance.now();
        await api.partView(part);
        times.push(Math.round(performance.now() - t0));
      }
      out.push(`${id}: ${text.length} chars, get_part_view cold ${cold} ms, then cached ${times.join("/")} ms`);
    }
    return out.join("; ");
  });

  await report("DONE", true, "cjk self-test finished");
}
