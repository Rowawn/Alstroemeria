// Validation of lookup-provider output (SPEC §16.8). Plain text only; links must be https.

import { ROLES, type LookupItem, type LookupSection, type Role } from "./types";

export const MAX_SECTIONS = 8;
export const MAX_ITEMS = 50;

const text = (v: unknown, max: number): string | null => (typeof v === "string" && v.trim() ? v.slice(0, max) : null);

function sanitizeItem(raw: unknown): LookupItem | null {
  if (!raw || typeof raw !== "object") return null;
  const o = raw as Record<string, unknown>;
  switch (o.kind) {
    case "text": {
      const t = text(o.text, 500);
      return t ? { kind: "text", text: t, role: ROLES.includes(o.role as Role) ? (o.role as Role) : undefined } : null;
    }
    case "kv": {
      const label = text(o.label, 80);
      const t = text(o.text, 500);
      return label && t ? { kind: "kv", label, text: t } : null;
    }
    case "pitch": {
      const reading = text(o.reading, 64);
      const d = o.downstep;
      return reading && typeof d === "number" && Number.isInteger(d) && d >= 0 && d <= 64 ? { kind: "pitch", reading, downstep: d } : null;
    }
    case "link": {
      const label = text(o.label, 80);
      const url = text(o.url, 2000);
      if (!label || !url) return null;
      try {
        return new URL(url).protocol === "https:" ? { kind: "link", label, url } : null;
      } catch {
        return null;
      }
    }
    default:
      return null; // unknown kinds: forward compatibility
  }
}

/** Keep well-formed sections and items; everything else is dropped. */
export function sanitizeSections(raw: unknown): LookupSection[] {
  if (!Array.isArray(raw)) return [];
  const out: LookupSection[] = [];
  for (const s of raw.slice(0, MAX_SECTIONS)) {
    if (!s || typeof s !== "object") continue;
    const title = text((s as Record<string, unknown>).title, 80);
    const items = (s as Record<string, unknown>).items;
    if (!title || !Array.isArray(items)) continue;
    const clean = items.slice(0, MAX_ITEMS).map(sanitizeItem).filter((i): i is LookupItem => i !== null);
    if (clean.length) out.push({ title, items: clean });
  }
  return out;
}
