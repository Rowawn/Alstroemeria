#!/usr/bin/env node
// List a podcast feed's latest episodes (at most 20) as {guid, title, minutes, hasTranscript, hasAudio}.
//   node scripts/pack/rss.mjs <feed-url>
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const url = process.argv[2];
if (!url) {
  console.error("usage: node scripts/pack/rss.mjs <feed-url>");
  process.exit(2);
}
const r = spawnSync("cargo", ["run", "-q", "-p", "alst-pack", "--example", "probe", "--", "rss", url], { cwd: root, encoding: "utf8" });
if (r.status !== 0) {
  console.error(r.stderr.split("\n").slice(-8).join("\n"));
  process.exit(1);
}
process.stdout.write(r.stdout);
