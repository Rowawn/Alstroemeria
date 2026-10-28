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

## Install

Download the installer for your system from the project's Releases page. The builds are **not code-signed** (no paid
certificates yet), so each system warns on first run:

- **macOS** (`.dmg`): drag Alstroemeria to Applications. The first time, macOS says it can't verify the app. Open
  **System Settings → Privacy & Security**, scroll to the message about Alstroemeria and choose **Open Anyway**.
  (On older macOS, right-click the app and choose Open.)
- **Windows** (`-setup.exe`): SmartScreen shows "Windows protected your PC". Choose **More info → Run anyway**.
  It installs for your user only, without administrator rights.
- **Linux**: make the `.AppImage` executable and run it, or install the `.deb`.
- **Android**: download the `.apk` for your phone (`arm64-v8a` for nearly every phone made since 2017, or the
  universal one) and allow installing from your browser when Android asks.

Japanese and Korean need their word dictionary, a one-time download of 10 to 20 MB (Languages tab). You can also
import it from a file, or a paired device sends it to you the next time you sync.

## Releasing

```
node scripts/bump-version.mjs 0.1.1     # version everywhere; then edit CHANGELOG.md
git tag v0.1.1 && git push --tags       # release.yml builds a draft release; review and publish it
```

Keys and passwords are never in the repository (`~/.alstroemeria/`, SPEC §17.2). Language data packages are built by the
manual "Language data" workflow. `npm run notices` regenerates `THIRD-PARTY-NOTICES.md`. Local release builds that
produce updater artifacts need `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the
environment. Development with the dictionaries compiled in (no download):
`npm run tauri dev -- --features embed-langdata`.
