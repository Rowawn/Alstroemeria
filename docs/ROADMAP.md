# Roadmap

Each milestone ends with something usable and is sized for one or a few focused sessions.
**Model** says which model is worth the credits: design-heavy work → Opus; mechanical work → Sonnet.

| M | Milestone | Model | Done when |
|---|---|---|---|
| 0 | **Spec** | Opus | `docs/SPEC.md`, this roadmap, `CLAUDE.md` exist ✅ |
| 1 | **Skeleton** | Sonnet | See below ✅ |
| 2 | **Reader MVP** ✅ (see status below) | Sonnet (Opus for term matching) | Import text/EPUB/URL; `lang.es` declarative extension; reader with token statuses; term editor popup; Yomitan dictionary import + URL lookups; TSV export for Anki |
| 3 | **Extension host** | Opus design ✅, Sonnet build | Worker runtime, host API v1, decorators/panels/commands; capability graph; one example add-on (e.g. language-universal "unknown-word density" panel) |
| 4 | **Media** ✅ (see status below) | Sonnet | Audio/video + SRT/VTT/LRC cue-synced reader; podcast RSS importer; YouTube subtitles (desktop) |
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

## M3 — Extension host (build plan)

Design: SPEC §6.9 (read §6.9 fully; §6.1–6.8 only for background). Steps, each ending with tests passing:

1. **`crates/alst-ext`** (pure Rust): manifest types + validation (incl. `license`), bundled
   extensions via `include_dir`, installed-folder scan, `.alext` install/uninstall with the zip
   safety checks, capability resolution (§6.9.3) and the permission checker (§6.9.4).
   Unit tests: wildcards, provider choice, unmet requires, cycles, deterministic order,
   API-version and platform filtering, zip-slip rejection, `net:` host matching.
   Replace `alst-text::bundled` with languages loaded from the registry.
   Add `schemas/extension.schema.json` matching the manifest types (for authors; CI validates first-party manifests).
2. **Shell:** `alstext://` protocol; commands `list_extensions`, `resolve_extensions(language)`,
   `install_extension` (raw bytes), `uninstall_extension`, `set_extension_enabled`,
   `set_extension_settings`, `set_provider`, `set_decorator_order`, and `ext_call(extId, method, params)`
   which routes host-API calls with permission checks (storage, terms, events, dict, net).
   Core events are forwarded to the UI as a Tauri event. `get_part_view` adds `tokens` and `Unit.tokenRange`.
3. **CSP** per §6.9.5, then verify the app still works and that a worker cannot invoke commands directly (write down how it was checked).
4. **UI extension host** (`ui/src/lib/ext/`): worker bootstrap, RPC with timeouts and error state,
   annotator/decorator pipeline, hint merging, render hints in the reader, role CSS variables (light + dark).
   Vitest for RPC timeouts and hint merging.
5. **UI screens:** Extensions screen (list, enable per language, provider picker, decorator order,
   permissions, settings form from JSON Schema, install `.alext` with permission prompt, errors and
   disabled reasons); sidebar panel host (sandboxed iframe + relay); command palette (Ctrl/Cmd+K).
6. **SDK:** `sdk/alstroemeria.d.ts`, `sdk/panel.js`, `sdk/wasm-shim.js`, `sdk/README.md` (how to write
   an extension in JS and in Rust), MIT `LICENSE`.
7. **Examples:** `extensions/es-morph-hints` (Rust → WASM, its own small crate excluded from the
   default workspace build) and `extensions/addon-density` (JS). `npm run build:extensions` wired
   before `tauri dev/build`. Add `rustup target add wasm32-unknown-unknown` to the README.
8. **Verify:** desktop end-to-end (both examples visible and configurable; install a zipped copy of
   `addon-density` as `.alext`; disable/enable; a deliberately slow extension times out cleanly);
   then Android: the reader shows `es-morph-hints` underlines and the density panel opens.

Out of scope: lookupProvider, importer/exporter, activity, native runtime, JS tokenizers, extension sync.

### M3 status

Done: all eight steps. `crates/alst-ext` (manifest + validation, registry of bundled/installed extensions, `.alext`
install with zip safety checks, capability resolution, permission checker, `net.fetch`, and `host::call`, which routes
every host-API call with permission checks), `schemas/extension.schema.json`, store methods for `extension_state`,
`settings`, `ext_data` and event queries, `alst-text::bundled` removed (languages come from the registry),
`alstext://` protocol, extension commands (plus `inspect_extension`, so the UI can show permissions before installing),
CSP, UI host (`ui/src/lib/ext/`: worker bootstrap, RPC with timeouts, annotator/decorator pipeline, hint merging),
Extensions screen, sandboxed panel host, command palette (Ctrl/Cmd+K, plus a button for touch), `sdk/`,
`extensions/es-morph-hints` (Rust → WASM) and `extensions/addon-density` (JS). `cargo test --workspace` and
`npm test` (Vitest, 29 tests) pass.

Verified in the real app (macOS WebKit, and the Android API 34 emulator with WebView 113), through the built-in
self-test (`ui/src/lib/ext/selftest.ts`; create a file `selftest` in the app data folder, or set `ALST_DATA_DIR`, see
SPEC §6.9.5): bundled extensions resolve and run; the WASM annotator/decorator underline words in the reader; settings
change the output; the density panel shows stats and history from `part.read` events; installing a zipped copy as
`.alext` with its permission preview; zip-slip rejected; a deliberately slow extension times out after 2 s and the
others keep working; disable/enable; a hostile worker and a hostile panel could not reach a loopback canary server,
could not call Tauri commands, and were refused every permissioned host call. Screenshots on Android show the
underlines, the density panel and the Extensions screen.

Findings that changed the design (small, written into SPEC §6.9):
- A worker started from a same-origin script URL does **not** inherit the CSP on WebKit (a `no-cors` fetch got out).
  Workers are now started from an inlined `blob:` bootstrap with `worker-src blob:`, which inherits it. Verified on both platforms.
- Android's IPC delivers `Uint8Array` bodies as JSON, so the raw-body commands (M2's EPUB/dictionary import too) failed
  there with "expected a binary body"; the shell now accepts both forms (slow for big files over JSON: base64 or chunking
  is a follow-up if large imports are slow on Android).
- Android rewrites `http://alstext.localhost/…` to `alstext://localhost/…`; the protocol handler accepts both.
- `$state` proxies can't be posted to workers; the reader sends snapshots.

Not verified: Windows (the `http://alstext.localhost` form was only seen on Android), iOS, the desktop window by
screenshot (no screen capture here; the same UI code was checked on Android), `tauri dev` with the dev CSP (dev
allows `ws:` and the Vite origin, so it does not prove the CSP), real large `.alext` installs on Android.
Known gaps: `local_token_cache` still absent; a worker serves all languages of an extension, with
`alst.extension.language` following the latest call; panel height is fixed; no keyboard navigation in the Extensions
screen beyond native controls. Next: M4.

## M4 — Media (plan)

Decisions (none changes the architecture, so no pause; the one real fork, importers, is recorded in SPEC §6.9.10):

- **RSS and YouTube are core importers, not extensions.** YouTube needs to spawn a process and RSS/audio needs large binary
  downloads into the media store; neither fits the worker sandbox or host API v1, and the sandbox has no XML parser.
  Both produce the same `ImportedWork` data the future `importer` extension point would return (SPEC §6.9.10).
- **Media store (SPEC §8):** `alst-core::media` (files under `<data>/media/ab/<sha256>`, no extension) + `Store::register_media`
  (synced `media` row). No migration: `media` and `parts.media_hash/timing` already exist.
- **Upload:** chunked commands (`media_upload_begin/chunk/finish`, 1 MiB chunks), so memory stays bounded and Android's
  JSON-encoded binary bodies stay small per call. Hashing happens at finish.
- **Playback:** `alstmedia://` protocol (Range support, bounded reads) + `media-src` in the CSP; the UI builds URLs with `convertFileSrc`.
- **Timing:** `parts.timing = {"v":1,"cues":[{start,end,a,b}]}`: ms, and `a`/`b` are UTF-16 offsets of the cue in the part
  text (same offsets as tokens/units). Cue text is flattened to one line per cue; cues are joined by `\n`. Long files split into
  several parts (~5000 chars, at cue boundaries) that share one `media_hash` with absolute times.
- **Parsers** in `alst-text::cues`: SRT, VTT (incl. rolling auto-caption dedupe), LRC (offset tag, multiple stamps, enhanced word tags).
  Subtitle bytes are decoded as UTF-8/UTF-16 (BOM) or else windows-1252.
- **Reader:** player bar (audio or video), cue rows with a play button, current-cue highlight (rAF), click a cue's blank area to seek,
  previous/next cue, repeat-sentence (a cue = a sentence for repeat), speed. `media.listen` event on finishing a part.
- **Podcast:** two steps (preview feed → pick episode). Needs a transcript (`podcast:transcript` SRT/VTT/JSON/text); episodes without one
  are listed but not importable (Whisper is M11).
- **YouTube:** desktop only; `yt-dlp` found on `PATH` plus the usual install dirs (GUI apps on macOS lack the shell `PATH`);
  the tab is hidden when not found. Subtitles only, as a text work (no video playback inside the webview).

Steps: 1 `alst-text::cues` + tests → 2 `alst-core::media` + store changes + tests → 3 RSS + yt-dlp importers → 4 shell commands + protocol + CSP
→ 5 UI (import forms, player, cue reader) + Vitest → 6 desktop verify (self-test) → 7 Android API 34 verify → 8 docs/status.

### M4 status

Done: media store (`alst-core::media`, `Store::register_media`), `alst-text::cues` (SRT/VTT/LRC), importers for audio/video + subtitle
file, podcast RSS (SRT/VTT/JSON/HTML/text transcripts) and YouTube subtitles (desktop, `yt-dlp`, tab hidden when absent), `alstmedia://`
protocol with Range + `media-src` CSP, chunked upload, cue-synced reader (highlight, click-to-seek, prev/next, repeat sentence, speed,
`media.listen`), SPEC §6.9.10 (importer decision: RSS and YouTube are core), §8, §4.2. `cargo test --workspace` and `npm test`
(34 Vitest tests) pass. No migration was needed.

Verified in the real apps through the media self-test (`ui/src/lib/selftest-media.ts`; put `media <feed url> [<youtube url>]` in the
`selftest` file of the data folder): macOS WebKit and the Android API 34 emulator (WebView 113) both passed every check: chunked
upload (3 chunks), dedupe, LRC lyrics, rejected bad files, seeking/Range playback, DOM-driven reader (highlight follows playback,
click-to-seek, word click opens the editor without seeking, prev/next, repeat loops, speed), `media.listen`, podcast feed with
4 episodes (m4a+SRT, mp4+VTT, wav+JSON; one without transcript refused), real YouTube auto-captions via yt-dlp on desktop
(959 cues → 3 text parts), yt-dlp reported unavailable on Android.

What plays (actually decoded and played in the self-test): AAC/m4a, WAV, H.264 mp4 on both macOS and Android. By `canPlayType` only
(not played; no encoder here): mp3, ogg/opus, flac, webm (VP8/VP9) are supported on both; HEVC "probably" on both (hardware-dependent);
`.mov` works on macOS only; `.mkv` on neither, so such files import but show "can't play this file" in the player.

Not verified: Windows and iOS; the desktop window by screenshot (no capture here; the Android UI was not inspected by eye either,
only through DOM assertions); large files (the 3 MB upload was the largest; Android sends 1 MiB chunks as JSON, so a very large
audiobook will be slow); the release CSP on Android (the dev build allows the Vite origin; `alstmedia` is in both CSPs);
podcast feeds from real servers (only a local feed was used); YouTube videos with manual subtitles (only auto-captions tried).
Known gaps: media files are not removed when a work is deleted (there is no delete yet); no waveform/A-B loop; a multi-word term
spanning two cues renders in the first cue; `ALST_DATA_DIR`-less Android `adb` setup: use `npm run tauri android dev` (a plain debug APK
loads the Vite URL and can't reach IPC). Next: M5.
