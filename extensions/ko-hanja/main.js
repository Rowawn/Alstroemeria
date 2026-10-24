// Korean hanja (SPEC §16.10). Data: `hanja.tsv` (`word<TAB>hanja<TAB>meaning`, sorted) and `hanja-chars.tsv`.
// Replacing the files (or installing a newer copy of this extension with other data) changes the data.

let data = null;
let loading = null;

/**
 * Index over `hanja.tsv` (`hangul<TAB>hanja<TAB>meaning`, sorted by hangul) without parsing it: only line offsets are
 * kept and lookups binary-search the text, so a 275k-row file costs a few milliseconds to open even on slow devices.
 * `charsTsv` (`hanja<TAB>meaning`) is small and goes into a Map.
 */
export function parse(wordsTsv, charsTsv = "") {
  const starts = [0];
  for (let i = wordsTsv.indexOf("\n"); i !== -1 && i + 1 < wordsTsv.length; i = wordsTsv.indexOf("\n", i + 1)) starts.push(i + 1);
  const keyAt = (n) => wordsTsv.slice(starts[n], wordsTsv.indexOf("\t", starts[n]));
  const rowAt = (n) => {
    const end = n + 1 < starts.length ? starts[n + 1] - 1 : wordsTsv.length;
    const [, hanja, meaning = ""] = wordsTsv.slice(starts[n], end).replace(/\n$/, "").split("\t");
    return { hanja, meaning };
  };
  const chars = new Map();
  for (const line of charsTsv.split("\n")) {
    const [hanja, meaning] = line.split("\t");
    if (hanja && meaning) chars.set(hanja, meaning);
  }
  return {
    chars,
    find(word) {
      let lo = 0;
      let hi = starts.length;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (keyAt(mid) < word) lo = mid + 1;
        else hi = mid;
      }
      const out = [];
      for (let n = lo; n < starts.length && keyAt(n) === word; n++) out.push(rowAt(n));
      return out;
    },
  };
}

function load() {
  loading ??= (async () => {
    const get = async (name) => (await fetch(new URL(`./${name}`, import.meta.url))).text();
    const [words, chars] = await Promise.all([get("hanja.tsv"), get("hanja-chars.tsv")]);
    data = parse(words, chars);
    return data;
  })();
  return loading;
}

/** Load the data while the extension activates (5 s) so the 2 s hooks don't pay for the download. */
export async function activate() {
  await load();
}

/** Candidates for a lemma: the word itself, or its stem without a trailing 하다/되다 (공부하다 → 공부). */
export function candidates(lemma, d) {
  const out = [];
  const tries = [lemma];
  const m = /^(.+?)(하다|되다)$/.exec(lemma);
  if (m) tries.push(m[1]);
  for (const t of tries) for (const c of d.find(t)) if (!out.some((o) => o.hanja === c.hanja)) out.push(c);
  return out.slice(0, 5);
}

const isHanja = (c) => /[㐀-䶿一-鿿豈-﫿]/.test(c);

export async function annotate({ tokens }) {
  const d = await load();
  const out = {};
  for (const t of tokens) {
    if (!t.isWord || !t.lemma) continue;
    const c = candidates(t.lemma, d);
    if (c.length) out[t.i] = { "ko.hanja": { hanja: c[0].hanja, alternatives: c.slice(1).map((x) => x.hanja) } };
  }
  return out;
}

export async function decorate({ units, tokens }) {
  const { ruby } = await alst.settings.get();
  if (!ruby) return {};
  const out = {};
  units.forEach((u, i) => {
    if (!u.isWord || u.wordCount !== 1) return;
    const h = tokens[u.tokenRange[0]]?.ann?.["ko.hanja"];
    // Only ruby the whole token when it is the bare word (not 공부했어요, whose hanja covers just 공부).
    if (h && u.text === tokens[u.tokenRange[0]].lemma?.replace(/(하다|되다)$/, "")) out[i] = [{ kind: "ruby", text: h.hanja }];
  });
  return out;
}

export async function lookup({ unit }) {
  const d = await load();
  const lemma = unit.lemma ?? unit.key;
  const c = candidates(lemma, d);
  if (!c.length) return [];
  const items = [];
  for (const { hanja, meaning } of c) {
    items.push(meaning ? { kind: "kv", label: hanja, text: meaning } : { kind: "text", text: hanja });
    for (const ch of new Set([...hanja].filter(isHanja))) {
      const m = d.chars.get(ch);
      if (m) items.push({ kind: "kv", label: ch, text: m });
    }
  }
  return [{ title: "Hanja", items }];
}
