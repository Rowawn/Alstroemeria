#!/usr/bin/env node
// Check article URLs with the app's own extractor; prints only {url, ok, title, words, excerpt|error} per URL.
//   node scripts/pack/probe.mjs <url> [<url>...]
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const urls = process.argv.slice(2);
if (urls.length === 0) {
  console.error("usage: node scripts/pack/probe.mjs <url> [<url>...]");
  process.exit(2);
}
const r = spawnSync("cargo", ["run", "-q", "-p", "alst-pack", "--example", "probe", "--", "article", ...urls], { cwd: root, encoding: "utf8" });
if (r.status !== 0) {
  console.error(r.stderr.split("\n").slice(-8).join("\n"));
  process.exit(1);
}
process.stdout.write(r.stdout);
