# Roadmap

Each milestone ends with something usable and is sized for one or a few focused sessions.
**Model** says which model is worth the credits: design-heavy work → Opus; mechanical work → Sonnet.

| M | Milestone | Model | Done when |
|---|---|---|---|
| 0 | **Spec** | Opus | `docs/SPEC.md`, this roadmap, `CLAUDE.md` exist ✅ |
| 1 | **Skeleton** | Sonnet | See below |
| 2 | **Reader MVP** | Sonnet (Opus for term matching) | Import text/EPUB/URL; `lang.es` declarative extension; reader with token statuses; term editor popup; Yomitan dictionary import + URL lookups; TSV export for Anki |
| 3 | **Extension host** | Opus design, Sonnet build | Worker runtime, host API v1, decorators/panels/commands; capability graph; one example add-on (e.g. language-universal "unknown-word density" panel) |
| 4 | **Media** | Sonnet | Audio/video + SRT/VTT/LRC cue-synced reader; podcast RSS importer; YouTube subtitles (desktop) |
| 5 | **CJK languages** | Sonnet (Opus for deinflection) | `lang.ja` (Lindera, furigana, pitch, Yomitan-style lookup), `lang.ko` (+ hanja add-on, grammar colors) |
| 6 | **LAN sync** | Opus | Pairing by QR, convergence property tests pass, media sync policy |
| 7 | **Releases** | Sonnet | Signed installers for Windows/macOS/Linux, Android APK/AAB, updater |
| 8 | **Content packs** | Sonnet | `.alpack` import with review screen; `build-pack` Claude skill + fetch/extract scripts |
| 9 | **SRS module** | Opus design, Sonnet build | FSRS, `.apkg` import/export, concept-linked cards with rotating examples |
| 10 | **iOS / iPadOS** | Sonnet | App runs on iPhone/iPad; LAN sync with permissions; tablet layouts |
| 11 | **Practice tools + native tier** | Opus | Shadowing/phonetics activities; optional Whisper; native extension loading (desktop/Android) |

## M1 — Skeleton (detail)

Goal: prove the stack on desktop **and Android** before any feature work.

1. Cargo workspace + npm workspace per SPEC §12; `.gitignore`; README stub.
2. Tauri 2 app with Svelte 5 + TS + Vite in `ui/`, thin shell in `src-tauri/`.
3. `crates/alst-core`:
   - SQLite open/create in the platform app-data dir; migration runner; migration `0001` with
     the core tables from SPEC §4.2 (works, parts, concepts, terms, term_parents, media,
     extension_state, ext_data, settings, events, `_changes`, local_settings, local_peers).
   - UUIDv7 ids, HLC (SPEC §4.4) with unit tests, site_id generated on first launch.
   - Minimal `store` write layer: insert/update a `works` row and record `_changes`, with tests.
4. Tauri commands: `create_work(title, text)`, `list_works()`. UI: a library list + "add text" form.
5. Runs with `npm run tauri dev` on macOS **and** `npm run tauri android dev` on an emulator or device.
6. `cargo test --workspace` passes.

Not in M1: tokenizing, the reader, extensions, sync.
