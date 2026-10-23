# Alstroemeria

Local-first language-immersion app. See `docs/SPEC.md`, `docs/ROADMAP.md` and `CLAUDE.md`.

```
npm install
npm run tauri dev            # desktop
npm run tauri android dev    # Android
cargo test --workspace
npm test                     # Vitest (extension host logic)
```

First-party extensions in `extensions/` include a Rust → WebAssembly one, built by `npm run build:extensions`
(which `npm run tauri …` runs first). It needs the wasm target:

```
rustup target add wasm32-unknown-unknown
```

To write extensions see `sdk/README.md`. Self-test in the real webview: create an empty file named `selftest` in the
app data folder (use `ALST_DATA_DIR=<scratch dir>` on desktop to avoid touching your library) and start the app;
results go to stderr/logcat and `selftest.log`.
