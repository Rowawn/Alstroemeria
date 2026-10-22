# Roadmap

Each milestone ends with something usable and is sized for one or a few focused sessions.
**Model** says which model is worth the credits: design-heavy work → Opus; mechanical work → Sonnet.

| M | Milestone | Model | Done when |
|---|---|---|---|
| 0 | **Spec** | Opus | `docs/SPEC.md`, this roadmap, `CLAUDE.md` exist ✅ |
| 1 | **Skeleton** | Sonnet | See below ✅ |
| 2 | **Reader MVP** ✅ (see status below) | Sonnet (Opus for term matching) | Import text/EPUB/URL; `lang.es` declarative extension; reader with token statuses; term editor popup; Yomitan dictionary import + URL lookups; TSV export for Anki |
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

## M2 — Reader MVP (plan)

Decisions taken where the spec was silent (none changes the architecture, so no pause was needed):

- **Crates:** new `alst-text` (tokenizer engine, term matching, importers) and `alst-dict`. `alst-text` stays pure
  (no DB, no Tauri); the shell glues store + tokenizer in a few lines per command. `alst-dict` works on a
  `rusqlite::Connection` and only touches `local_dict_*` tables (migration `0002`, which also is where they live).
- **New dependencies (all MIT/Apache/ISC):** `regex`, `zip`, `roxmltree` (EPUB OPF), `scraper` (HTML → text),
  `ureq` + `encoding_rs` (article fetch), `tauri-plugin-opener` (open lookup URLs in the system browser).
  File pickers are plain `<input type=file>`; bytes go to Rust as raw IPC bodies, so no dialog plugin and the same path works on Android.
- **Bundled language:** `extensions/lang-es/extension.json` (MIT) is embedded into `alst-text::bundled` until the
  M3 registry (`alst-ext`) replaces it. Only the `regex` engine and the `declarative` keys of SPEC §6.7 are read.
- **Tokens:** whitespace and punctuation are tokens too (`isWord:false`), so the concatenated token text equals the source.
  Sentence breaks: a `sentenceEnd` char followed by whitespace/closing quote (minus `exceptions`), or a blank line.
  Spec example change: `¿¡` removed from `sentenceEnd` (they open sentences).
- **Term matching (§5.3):** per-language trie over normalized word sequences, longest match first, within a sentence,
  words separated only by whitespace. Terms are keyed by `text_lc` = normalized words joined by one space.
- **Reader contract:** Rust returns "units" (a word, a multi-word match, or non-word text) with term id/status; UI re-requests the part after each save.
- **Store additions:** parts/work import with parts, `save_term` (find-or-create by `(language, text_lc)`, per-column writes,
  `term.status` event in the same transaction), `set_term_parents` (tombstones), `emit_event`.
- **Deferred inside M2:** `local_token_cache` (tokenizing a part is cheap), EPUB images → media store (needs the §8 media
  store, M4), Yomitan `term_meta_bank` (frequency/pitch, M5), structured-content is flattened to plain text, no
  deinflection (CJK, M5), AnkiConnect exporter, JSON-Schema validation of manifests (M3).

Steps: 1 store + migration 0002 → 2 `alst-text` tokenizer + goldens → 3 matcher + goldens → 4 importers → 5 `alst-dict`
→ 6 shell commands → 7 UI (library, import, reader, editor, dictionaries, export) → 8 desktop run → 9 Android check.

### M2 status

Done: `alst-text` (tokenizer, matcher, importers, TSV), `alst-dict`, store terms/parents/events, migration 0002,
`extensions/lang-es`, shell commands, Svelte UI (library, import text/EPUB/URL, reader, term editor with phrase
selection, dictionaries, Anki export). `cargo test --workspace` passes (goldens in `crates/alst-text/tests/fixtures/es/`;
regenerate with `UPDATE_GOLDEN=1`).

Verified: Rust tests; the real desktop app launches (`npm run tauri dev`, no runtime errors); UI interaction flow
(click, extend to phrase, status colors, lookup links, finish part) in a browser against a JS mock of the commands;
Android debug APK (x86_64) builds.

Not verified: the real desktop window end-to-end (file pickers, EPUB/dictionary upload via raw IPC bodies,
Save to Downloads, opening links in the system browser), and nothing was run on the Android emulator.
Known gaps: no `local_token_cache`, no keyboard shortcuts for status, dictionary lookup is exact-match only
(no lemma/deinflection), Yomitan meta banks ignored, EPUB images skipped. Next: M3.
