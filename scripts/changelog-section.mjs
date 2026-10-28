#!/usr/bin/env node
// Print the CHANGELOG.md section for a version (the release notes, SPEC §17.1):
//   node scripts/changelog-section.mjs 0.1.0
// Exits non-zero if there is no such section.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

/** Text under `## [version]` up to the next `## ` heading, trimmed; `null` if absent or empty. */
export function section(changelog, version) {
  const lines = changelog.split("\n");
  const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
  if (start < 0) return null;
  let end = lines.findIndex((l, i) => i > start && l.startsWith("## "));
  if (end < 0) end = lines.length;
  const body = lines.slice(start + 1, end).join("\n").trim();
  return body || null;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const version = process.argv[2]?.replace(/^v/, "");
  const text = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "CHANGELOG.md"), "utf8");
  const body = version && section(text, version);
  if (!body) {
    console.error(`CHANGELOG.md has no section for ${version}`);
    process.exit(1);
  }
  console.log(body);
}
