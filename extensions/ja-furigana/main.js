// Japanese furigana (SPEC §16.10): segmented ruby over kanji, okurigana aligned.

const hira = (s) => s.replace(/[ァ-ヶ]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0x60));
const isKanji = (c) => /[㐀-䶿一-鿿豈-﫿々]/.test(c);

/** Split into alternating runs: [{ kanji: bool, text }]. */
function runs(surface) {
  const out = [];
  for (const ch of surface) {
    const k = isKanji(ch);
    const last = out[out.length - 1];
    if (last && last.kanji === k) last.text += ch;
    else out.push({ kanji: k, text: ch });
  }
  return out;
}

/**
 * Align a surface form with its hiragana reading. Returns segments `[{ base, rt? }]` whose bases concatenate to
 * `surface`: each kanji run gets the part of the reading between the kana around it (so 食べる → 食/た + べる).
 * A kanji run is never split (日本語 → one segment). Returns null if there is nothing to annotate or the
 * reading can't be matched to the kana in the surface; callers then use a single segment or nothing.
 */
export function align(surface, reading) {
  const rs = runs(surface);
  if (!rs.some((r) => r.kanji) || !reading) return null;
  const rd = hira(reading);
  const rr = [...rd];
  // out[i] = reading text for run i (kanji runs only)
  const solve = (i, pos) => {
    if (i === rs.length) return pos === rr.length ? [] : null;
    const run = rs[i];
    if (!run.kanji) {
      const lit = [...hira(run.text)];
      for (let k = 0; k < lit.length; k++) if (rr[pos + k] !== lit[k]) return null;
      const rest = solve(i + 1, pos + lit.length);
      return rest && [null, ...rest];
    }
    const need = [...run.text].length; // at least one reading char per kanji
    const next = rs[i + 1];
    if (!next) {
      const len = rr.length - pos;
      return len >= need ? [rr.slice(pos).join("")] : null;
    }
    // next is a kana run: try each position where it could start (shortest kanji reading first)
    const lit = hira(next.text);
    for (let start = pos + need; start <= rr.length - [...lit].length; start++) {
      if (rr.slice(start, start + [...lit].length).join("") !== lit) continue;
      const rest = solve(i + 1, start);
      if (rest) return [rr.slice(pos, start).join(""), ...rest];
    }
    return null;
  };
  const parts = solve(0, 0);
  if (!parts) return null;
  return rs.map((r, i) => (r.kanji ? { base: r.text, rt: parts[i] } : { base: r.text }));
}

/** Segments for a whole unit made of one or more tokens, each aligned with its own reading. */
export function unitSegments(tokens) {
  const segs = [];
  let any = false;
  for (const t of tokens) {
    const reading = t.ann?.["ja.reading"];
    const a = t.isWord && reading ? align(t.text, reading) : null;
    if (a) {
      segs.push(...a);
      any = true;
    } else if (t.isWord && reading && [...t.text].some(isKanji)) {
      segs.push({ base: t.text, rt: hira(reading) }); // alignment failed: whole word over its reading
      any = true;
    } else if (t.text) segs.push({ base: t.text });
  }
  return any ? segs : null;
}

const MAX_STATUS = { all: 99, unknown: 1, learning: 3 };

export async function decorate({ units, tokens }) {
  const { show } = await alst.settings.get();
  if (show === "none") return {};
  const max = MAX_STATUS[show] ?? 1;
  const out = {};
  units.forEach((u, i) => {
    if (!u.isWord || u.status == null) return;
    if (show !== "all" && u.status > max) return;
    const segs = unitSegments(tokens.slice(u.tokenRange[0], u.tokenRange[1] + 1));
    if (segs) out[i] = [{ kind: "ruby", segments: segs }];
  });
  return out;
}
