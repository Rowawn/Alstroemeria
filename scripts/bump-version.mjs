#!/usr/bin/env node
// Set the app version everywhere it is written (SPEC §17.1):
//   node scripts/bump-version.mjs 0.1.1      write it
//   node scripts/bump-version.mjs --print    show the current version and Android versionCode
// Files: src-tauri/tauri.conf.json (version, Android versionCode), src-tauri/Cargo.toml, package.json,
// and the `alstroemeria` entry in Cargo.lock. Crates under crates/ keep their own versions.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;

/** Android versionCode = major*10000 + minor*100 + patch. */
export function versionCode(version) {
  const m = SEMVER.exec(version);
  if (!m) throw new Error(`not a plain semver version (x.y.z): ${version}`);
  const [major, minor, patch] = m.slice(1).map(Number);
  if (minor > 99 || patch > 99) throw new Error("minor and patch must be at most 99 for the versionCode formula");
  return major * 10000 + minor * 100 + patch;
}

/** Replace the version in the `[package]` table of a Cargo.toml text. */
export function setCargoTomlVersion(text, version) {
  const head = text.indexOf("[package]");
  if (head < 0) throw new Error("no [package] table");
  const rest = text.slice(head);
  const re = /^version\s*=\s*"[^"]*"/m;
  if (!re.test(rest)) throw new Error("no version line in [package]");
  return text.slice(0, head) + rest.replace(re, `version = "${version}"`);
}

/** Replace the version of the `alstroemeria` package in a Cargo.lock text. */
export function setCargoLockVersion(text, version) {
  const re = /(name = "alstroemeria"\nversion = )"[^"]*"/;
  if (!re.test(text)) throw new Error("alstroemeria is not in Cargo.lock");
  return text.replace(re, `$1"${version}"`);
}

export function setTauriConf(text, version) {
  const conf = JSON.parse(text);
  conf.version = version;
  conf.bundle ??= {};
  conf.bundle.android = { ...conf.bundle.android, versionCode: versionCode(version) };
  return JSON.stringify(conf, null, 2) + "\n";
}

export function setPackageJson(text, version) {
  const pkg = JSON.parse(text);
  pkg.version = version;
  return JSON.stringify(pkg, null, 2) + "\n";
}

function edit(file, fn) {
  const path = join(root, file);
  writeFileSync(path, fn(readFileSync(path, "utf8")));
}

function main(args) {
  if (args[0] === "--print") {
    const v = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8")).version;
    console.log(`${v} (versionCode ${versionCode(v)})`);
    return;
  }
  const version = args[0];
  if (!version || !SEMVER.test(version)) {
    console.error("usage: node scripts/bump-version.mjs <x.y.z> | --print");
    process.exit(2);
  }
  edit("src-tauri/tauri.conf.json", (t) => setTauriConf(t, version));
  edit("src-tauri/Cargo.toml", (t) => setCargoTomlVersion(t, version));
  edit("package.json", (t) => setPackageJson(t, version));
  edit("Cargo.lock", (t) => setCargoLockVersion(t, version));
  console.log(`version ${version}, Android versionCode ${versionCode(version)}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2));
