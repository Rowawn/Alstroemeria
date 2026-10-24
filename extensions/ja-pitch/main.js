// Japanese pitch accent (SPEC §16.10). Data comes from imported Yomitan pitch meta banks (`alst.dict.meta`).

const SMALL = new Set([..."ゃゅょぁぃぅぇぉャュョァィゥェォ"]);
const hira = (s) => s.replace(/[ァ-ヶ]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0x60));

/** Mora count: each kana is one mora except the small ゃゅょぁぃぅぇぉ; ー, っ and ん count. */
export function moraCount(reading) {
  let n = 0;
  for (const ch of reading) if (!SMALL.has(ch)) n++;
  return n;
}

/** downstep 0 → heiban; 1 → atamadaka; = mora count → odaka; else nakadaka. */
export function patternOf(reading, downstep) {
  if (downstep === 0) return "heiban";
  if (downstep === 1) return "atamadaka";
  return downstep === moraCount(reading) ? "odaka" : "nakadaka";
}

const cache = new Map(); // lemma → rows
let anyPitch = false;

/** Fill the cache for lemmas in one batched host call. */
async function fetchRows(language, lemmas) {
  const todo = [...new Set(lemmas)].filter((l) => !cache.has(l));
  if (!todo.length) return;
  for (let i = 0; i < todo.length; i += 400) {
    const chunk = todo.slice(i, i + 400);
    const rows = await alst.dict.meta(language, { terms: chunk, mode: "pitch" });
    for (const l of chunk) cache.set(l, []);
    for (const r of rows) cache.get(r.term)?.push(r.data);
  }
}

/** Pick the pitch row matching the word: exact reading, else a reading the word's first morpheme starts. */
function choose(rows, reading, headReading) {
  if (!rows?.length) return null;
  const r = rows.find((d) => hira(d.reading) === reading) ?? rows.find((d) => headReading && hira(d.reading).startsWith(headReading)) ?? (rows.length === 1 ? rows[0] : null);
  return r && r.pitches?.length ? r : null;
}

function pitchFor(tok, rowsByLemma) {
  const lemma = tok.lemma ?? tok.text;
  const reading = tok.ann?.["ja.reading"];
  const head = tok.ann?.["ja.morphemes"]?.[0]?.reading;
  const row = choose(rowsByLemma.get(lemma), reading, head);
  if (!row) return null;
  const p = row.pitches[0].position;
  return { reading: hira(row.reading), downstep: p, pattern: patternOf(hira(row.reading), p), all: row.pitches.map((x) => x.position) };
}

export async function annotate({ language, tokens }) {
  const words = tokens.filter((t) => t.isWord && t.ann?.["ja.reading"]);
  await fetchRows(language, words.map((t) => t.lemma ?? t.text));
  const out = {};
  for (const t of words) {
    const p = pitchFor(t, cache);
    if (p) {
      out[t.i] = { "ja.pitch": p };
      anyPitch = true;
    }
  }
  return out;
}

export async function decorate({ units, tokens }) {
  const { display } = await alst.settings.get();
  if (display === "off") return {};
  const out = {};
  units.forEach((u, i) => {
    if (!u.isWord) return;
    const p = tokens[u.tokenRange[0]]?.ann?.["ja.pitch"];
    if (!p) return;
    out[i] = display === "badge" ? [{ kind: "badge", text: `[${p.downstep}]` }] : [{ kind: "color", role: `accent-${p.pattern}` }];
  });
  return out;
}

export async function lookup({ language, unit, tokens }) {
  const tok = tokens.find((t) => t.isWord);
  const lemma = unit.lemma ?? unit.key;
  if (!lemma) return [];
  await fetchRows(language, [lemma]);
  const p = tok ? pitchFor(tok, cache) : null;
  if (!p) {
    const hint = anyPitch ? "No pitch data for this word." : "No pitch data found. Import a pitch dictionary (for example Kanjium) in Dictionaries.";
    return [{ title: "Pitch accent", items: [{ kind: "text", text: hint }] }];
  }
  const items = p.all.map((d) => ({ kind: "pitch", reading: p.reading, downstep: d }));
  items.push({ kind: "kv", label: "Pattern", text: `${p.pattern} [${p.downstep}]` });
  return [{ title: "Pitch accent", items }];
}
