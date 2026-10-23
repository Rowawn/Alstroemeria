import type { MergedHints } from "./hints";

/** Inline style for a unit's merged hints (colors come from the `--role-*` theme variables). */
export function hintStyle(h: MergedHints | undefined): string {
  if (!h) return "";
  const parts: string[] = [];
  if (h.color) parts.push(`color: var(--role-${h.color})`);
  if (h.underline) {
    parts.push(
      "text-decoration-line: underline",
      `text-decoration-style: ${h.underline.style}`,
      `text-decoration-color: var(--role-${h.underline.role})`,
      "text-decoration-thickness: 2px",
      "text-underline-offset: 3px",
    );
  }
  return parts.join("; ");
}

/** CSS custom properties panels receive so they match the app's light/dark theme (SPEC §6.9.7). */
export function themeVariables(): Record<string, string> {
  const style = getComputedStyle(document.documentElement);
  const names = ["bg", "fg", "muted", "panel", "border", "accent", "s0", "s1", "s2", "s3", "s4", "s5"];
  const roles = [
    "noun", "verb", "adjective", "adverb", "particle", "function", "name",
    "accent-high", "accent-low", "accent-drop", "info", "warn", "c1", "c2", "c3", "c4", "c5", "c6",
  ].map((r) => `role-${r}`);
  const out: Record<string, string> = {};
  for (const n of [...names, ...roles]) out[`--${n}`] = style.getPropertyValue(`--${n}`).trim();
  return out;
}
