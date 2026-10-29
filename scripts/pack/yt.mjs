#!/usr/bin/env node
// Check that videos have subtitles in a language: prints {url, title, minutes, subs: "manual"|"auto"|"none"} per URL.
//   node scripts/pack/yt.mjs <language> <url> [<url>...]
import { spawnSync } from "node:child_process";

const [lang, ...urls] = process.argv.slice(2);
if (!lang || urls.length === 0) {
  console.error("usage: node scripts/pack/yt.mjs <language> <url> [<url>...]");
  process.exit(2);
}
const has = (obj, l) => Object.keys(obj ?? {}).some((k) => k === l || k.startsWith(`${l}-`));
const out = urls.map((url) => {
  const r = spawnSync("yt-dlp", ["--dump-single-json", "--skip-download", "--no-warnings", url], { encoding: "utf8", maxBuffer: 64 << 20 });
  if (r.status !== 0) return { url, ok: false, error: (r.stderr || "yt-dlp failed").trim().split("\n").pop() };
  const j = JSON.parse(r.stdout);
  const subs = has(j.subtitles, lang) ? "manual" : has(j.automatic_captions, lang) ? "auto" : "none";
  return { url, ok: true, title: j.title, minutes: j.duration ? Math.round(j.duration / 60) : null, subs };
});
console.log(JSON.stringify(out));
