// Pitch-accent helpers for the core's pitch graph (SPEC §16.8, §16.10). Mora counting: each kana is one mora except
// the small ゃゅょぁぃぅぇぉ, which attach to the previous one; ー, っ and ん count.

const SMALL = new Set([..."ゃゅょぁぃぅぇぉャュョァィゥェォ"]);

/** Split a kana reading into morae. */
export function morae(reading: string): string[] {
  const out: string[] = [];
  for (const ch of reading) {
    if (SMALL.has(ch) && out.length) out[out.length - 1] += ch;
    else out.push(ch);
  }
  return out;
}

export type PitchPattern = "heiban" | "atamadaka" | "nakadaka" | "odaka";

/** Heiban 0; atamadaka 1; odaka = mora count; anything else nakadaka. */
export function patternOf(moraCount: number, downstep: number): PitchPattern {
  if (downstep === 0) return "heiban";
  if (downstep === 1) return "atamadaka";
  return downstep === moraCount ? "odaka" : "nakadaka";
}

/**
 * High/low per mora, plus one extra entry for what follows the word (a particle), which shows whether the pitch drops
 * after the word: heiban stays high, odaka drops there.
 */
export function highLow(moraCount: number, downstep: number): boolean[] {
  const out: boolean[] = [];
  for (let i = 1; i <= moraCount + 1; i++) {
    if (downstep === 0) out.push(i > 1);
    else if (downstep === 1) out.push(i === 1);
    else out.push(i > 1 && i <= downstep);
  }
  return out;
}
