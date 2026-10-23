// Render-hint validation and layering (SPEC §6.9.6.1).

import { ROLES, type RenderHint, type Role, type UnderlineStyle } from "./types";

const STYLES: UnderlineStyle[] = ["solid", "dotted", "wavy", "double"];
const roleOk = (r: unknown): r is Role => typeof r === "string" && (ROLES as readonly string[]).includes(r);

/** Keep only well-formed hints of kinds this host knows; unknown kinds are ignored for forward compatibility. */
export function sanitizeHints(raw: unknown): RenderHint[] {
  if (!Array.isArray(raw)) return [];
  const out: RenderHint[] = [];
  for (const h of raw) {
    if (!h || typeof h !== "object") continue;
    const o = h as Record<string, unknown>;
    switch (o.kind) {
      case "ruby":
        if (typeof o.text === "string" && o.text) out.push({ kind: "ruby", text: o.text.slice(0, 64) });
        break;
      case "color":
        if (roleOk(o.role)) out.push({ kind: "color", role: o.role });
        break;
      case "underline":
        if (roleOk(o.role)) {
          const style = STYLES.includes(o.style as UnderlineStyle) ? (o.style as UnderlineStyle) : undefined;
          out.push({ kind: "underline", role: o.role, style });
        }
        break;
      case "badge":
        if (typeof o.text === "string" && o.text) {
          out.push({ kind: "badge", text: [...o.text].slice(0, 4).join(""), role: roleOk(o.role) ? o.role : undefined });
        }
        break;
      case "tooltip":
        if (typeof o.text === "string" && o.text) out.push({ kind: "tooltip", text: o.text.slice(0, 200) });
        break;
    }
  }
  return out;
}

export type MergedHints = {
  ruby?: string;
  color?: Role;
  underline?: { role: Role; style: UnderlineStyle };
  badges: { text: string; role?: Role }[];
  tooltip?: string;
};

export type DecoratorOutput = { id: string; hints: Record<number, unknown> };

export const MAX_BADGES = 2;

/**
 * Layer hints from several decorators. `outputs` must already be in decorator order: for `ruby`, `color`
 * and `underline` the first decorator that sets one wins; `badge` and `tooltip` accumulate.
 */
export function mergeHints(unitCount: number, outputs: DecoratorOutput[]): MergedHints[] {
  const merged: MergedHints[] = Array.from({ length: unitCount }, () => ({ badges: [] }));
  const tooltips: string[][] = Array.from({ length: unitCount }, () => []);
  for (const out of outputs) {
    for (const [key, raw] of Object.entries(out.hints ?? {})) {
      const i = Number(key);
      if (!Number.isInteger(i) || i < 0 || i >= unitCount) continue;
      const m = merged[i];
      for (const h of sanitizeHints(raw)) {
        if (h.kind === "ruby") m.ruby ??= h.text;
        else if (h.kind === "color") m.color ??= h.role;
        else if (h.kind === "underline") m.underline ??= { role: h.role, style: h.style ?? "solid" };
        else if (h.kind === "badge") m.badges.push({ text: h.text, role: h.role });
        else if (h.kind === "tooltip") tooltips[i].push(h.text);
      }
    }
  }
  merged.forEach((m, i) => {
    m.badges = m.badges.slice(0, MAX_BADGES);
    if (tooltips[i].length) m.tooltip = tooltips[i].join("\n");
  });
  return merged;
}

/**
 * Merge one annotator's result into the tokens' `ann`. Only capabilities it `provides` are kept;
 * the names of dropped ones are returned so the caller can warn.
 */
export function mergeAnnotations(
  tokens: { ann: Record<string, unknown> }[],
  result: unknown,
  provides: string[],
): string[] {
  const dropped = new Set<string>();
  if (!result || typeof result !== "object") return [];
  for (const [key, anns] of Object.entries(result as Record<string, unknown>)) {
    const i = Number(key);
    if (!Number.isInteger(i) || i < 0 || i >= tokens.length || !anns || typeof anns !== "object") continue;
    for (const [cap, value] of Object.entries(anns as Record<string, unknown>)) {
      if (provides.includes(cap)) tokens[i].ann[cap] = value;
      else dropped.add(cap);
    }
  }
  return [...dropped];
}
