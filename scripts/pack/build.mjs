#!/usr/bin/env node
// Turn a small draft into a validated `.alpack`: fills in format fields, a new pack id, the date and item ids, writes
// inline `text` of embedded items to items/, validates with the app's own reader and zips.
//   node scripts/pack/build.mjs <draft.json> [out.alpack]
// Draft: { title, language, topic, level, description, items: [{kind, title, source, content, estimate, text?}], vocab: [...] }
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const [draftPath, outArg] = process.argv.slice(2);
if (!draftPath) {
  console.error("usage: node scripts/pack/build.mjs <draft.json> [out.alpack]");
  process.exit(2);
}
const d = JSON.parse(readFileSync(draftPath, "utf8"));
const slug = (s) => s.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
const work = mkdtempSync(join(tmpdir(), "alpack-"));
const itemsDir = join(work, "items");
mkdirSync(itemsDir);

const prefix = { article: "a", podcast: "p", video: "v", book: "b", text: "t", subtitle: "s", lyrics: "l" };
const counts = {};
const items = (d.items ?? []).map((it) => {
  const { text, ...rest } = it;
  counts[it.kind] = (counts[it.kind] ?? 0) + 1;
  rest.id ??= `${prefix[it.kind] ?? "i"}${counts[it.kind]}`;
  if (text !== undefined) {
    writeFileSync(join(itemsDir, `${rest.id}.md`), text);
    rest.content = { mode: "embedded", path: `items/${rest.id}.md` };
  }
  return rest;
});
const manifest = {
  format: "alstroemeria-pack",
  formatVersion: 1,
  id: d.id ?? randomUUID(),
  title: d.title,
  language: d.language,
  topic: d.topic,
  level: d.level,
  description: d.description,
  createdBy: d.createdBy ?? "agent:claude",
  createdAt: new Date().toISOString(),
  items,
  vocab: d.vocab ?? [],
};
writeFileSync(join(work, "manifest.json"), JSON.stringify(manifest, null, 2));
const out = outArg ?? join(root, "packs", `${slug(d.language)}-${slug(d.topic ?? d.title)}.alpack`);
mkdirSync(join(out, ".."), { recursive: true });
const r = spawnSync("cargo", ["run", "-q", "-p", "alst-pack", "--example", "probe", "--", "build", join(work, "manifest.json"), out, itemsDir], { cwd: root, encoding: "utf8" });
if (r.status !== 0) {
  console.error(r.stderr.split("\n").slice(-8).join("\n"));
  process.exit(1);
}
process.stdout.write(r.stdout);
