// Cue lookup for the cue-synced reader (SPEC §8). `parts.timing` cues are sorted by start time.

export type Cue = { start: number; end: number; a: number; b: number };
export type Timing = { v: number; cues: Cue[] };

/** Index of the last cue that has started by `ms`, or -1 before the first one. */
export function cueAt(cues: Cue[], ms: number): number {
  let lo = 0;
  let hi = cues.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (cues[mid].start <= ms) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

/** For each cue, the indexes of the reader units that start inside it (units are in text order). */
export function groupUnits(units: { start: number }[], cues: Cue[]): number[][] {
  const groups: number[][] = cues.map(() => []);
  let k = 0;
  units.forEach((u, i) => {
    while (k + 1 < cues.length && cues[k + 1].a <= u.start) k++;
    if (cues.length && u.start >= cues[0].a) groups[k].push(i);
  });
  return groups;
}

export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60) % 60;
  const sec = String(s % 60).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** Where to jump when "repeat" ends a pass: the locked cue's start, or null to keep playing. */
export function repeatTarget(cues: Cue[], locked: number, ms: number): number | null {
  const c = cues[locked];
  return c && ms >= c.end - 40 ? c.start : null;
}
