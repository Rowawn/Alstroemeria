// Builds first-party extensions that need it and vendors SDK files into their folders (SPEC §6.9.8).
//   * extensions/*/Cargo.toml  → cargo build --target wasm32-unknown-unknown --release → ext.wasm
//   * runtime "wasm"           → sdk/wasm-shim.js copied in
//   * contributes.panels       → sdk/panel.js copied in
// The copies and .wasm files are git-ignored; run before `tauri dev/build` (the `tauri` npm script does).

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const extRoot = join(root, "extensions");
const sdk = join(root, "sdk");

for (const name of readdirSync(extRoot)) {
  const dir = join(extRoot, name);
  if (!statSync(dir).isDirectory() || !existsSync(join(dir, "extension.json"))) continue;
  const manifest = JSON.parse(readFileSync(join(dir, "extension.json"), "utf8"));

  if (existsSync(join(dir, "Cargo.toml"))) {
    const crate = readFileSync(join(dir, "Cargo.toml"), "utf8").match(/^name\s*=\s*"([^"]+)"/m)?.[1];
    console.log(`[extensions] building ${manifest.id} (${crate}) to WebAssembly`);
    const r = spawnSync("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown"], { cwd: dir, stdio: "inherit" });
    if (r.status !== 0) {
      console.error("\n[extensions] build failed. If the target is missing: rustup target add wasm32-unknown-unknown");
      process.exit(r.status ?? 1);
    }
    // .cargo/config.toml in the extension folder points cargo at <repo>/target/ext-wasm.
    const out = join(root, "target", "ext-wasm", "wasm32-unknown-unknown", "release", `${crate.replace(/-/g, "_")}.wasm`);
    copyFileSync(out, join(dir, "ext.wasm"));
  }
  if (manifest.runtime?.type === "wasm") copyFileSync(join(sdk, "wasm-shim.js"), join(dir, "wasm-shim.js"));
  if (manifest.contributes?.panels?.length) copyFileSync(join(sdk, "panel.js"), join(dir, "panel.js"));
}
