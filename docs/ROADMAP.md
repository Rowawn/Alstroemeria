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
| 5 | **CJK languages** | Opus design ✅, Sonnet build | `lang.ja` (Lindera, furigana, pitch, Yomitan-style lookup), `lang.ko` (+ hanja add-on, grammar colors) |
| 6 | **LAN sync** ✅ (see status below) | Opus design ✅, Sonnet build | Pairing by code and QR, convergence property tests pass, media sync policy, dictionaries and extensions follow |
| 7 | **Releases** | Opus design ✅, Sonnet build | Public GitHub releases for Windows/macOS/Linux (unsigned for now) and Android (release-signed APKs/AAB), desktop updater, downloadable Japanese/Korean language data |
| 8 | **Content packs** | Opus design ✅, Sonnet build | `.alpack` import with review screen; `build-pack` Claude skill + fetch/extract scripts |
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

## M5 — Japanese and Korean (build plan)

Design: SPEC §16 (read fully), plus §6.9.6–6.9.7 for the extension contracts. Steps, each ending with tests passing:

1. **Engine:** add Lindera (IPADIC, ko-dic) to `alst-text` behind features `lang-ja`/`lang-ko`; pin versions.
   Word grouping (§16.2) with golden tests in `tests/fixtures/ja/` and `tests/fixtures/ko/` covering every
   example in §16.2 plus a paragraph of real prose per language. `termKey` (§16.3) in the language definition
   and matcher. Measure tokenizing time for a 5,000-character part on desktop; if over 100 ms on the Android
   emulator, implement `local_token_cache` (§5.3).
2. **Deinflection:** `alst-text::deinflect` engine + `extensions/lang-ja/deinflect.json` written from scratch.
   One test per row of the §16.5 table plus the negative cases.
3. **Dictionaries:** migration adding `local_dict_entries.rules` and `local_dict_meta`; import `rules` and the
   `freq`/`pitch` meta banks; `lookup_scan` (§16.6) with tests on a small fixture Yomitan dictionary
   (hand-made, a dozen entries, no copied JMdict data).
4. **Languages:** `extensions/lang-ja`, `extensions/lang-ko` manifests; registry support for `termKey`,
   `deinflection` and the new engine; term editor shows lemma + "seen as"; popup uses `lookup_scan`.
5. **Extension host additions:** `lookupProvider` (§16.8), `alst.dict.meta`, segmented ruby and the new
   accent roles (§16.9), CJK reader CSS. Update `sdk/alstroemeria.d.ts` and `schemas/extension.schema.json`.
   Vitest for segmented-ruby validation and lookup-provider timeouts.
6. **Add-ons:** `ja-furigana` (alignment unit tests: 食べる, 召し上がる, 取り扱い, お茶, 日本語, 行って),
   `ja-pitch` (mora count + pattern tests), `ko-hanja` (check data licence first; record the decision),
   `addon-grammar-colors`.
7. **Verify:** desktop and Android emulator: a Japanese article shows grouped words, furigana on unknown words,
   grammar colours, conjugated-verb lookup with reasons, and pitch colours after importing a pitch dictionary;
   a Korean article shows particles split from nouns, verb lemmas, and the hanja popup. Record the installer/APK
   size change from the embedded dictionaries.

Out of scope: Chinese, UniDic, pitchLine drawing over text, Korean deinflection rules, downloadable language data (M7 if needed).

### M5 status

Done: steps 1–7. Lindera 6.2.0 (IPADIC, ko-dic) behind `lang-ja`/`lang-ko`; word grouping (§16.2) with goldens for every spec example plus
real prose (`crates/alst-text/tests/fixtures/{ja,ko}`); `termKey: lemma` (term stored as lemma, "seen as" in the editor, space-free multi-word
matching for ja); `alst-text::deinflect` + `extensions/lang-ja/deinflect.json` (226 rules, generated by `scripts/gen-ja-deinflect.py`, one test per §16.5
row plus negatives); migrations 0003 (`rules`, `local_dict_meta`) and 0004 (`local_token_cache`); `lookup_scan`, `dict_meta`, freq/pitch import;
`lang-ja`, `lang-ko`, `ja-furigana`, `ja-pitch`, `ko-hanja`, `addon-grammar-colors`; `lookupProvider`, `alst.dict.meta`, segmented ruby, accent roles,
CJK reader CSS, About tab (attribution). `cargo test --workspace` (160) and `npm test` (64) pass. Spec clarifications are in §16 itself.

**Decided (option a): embedded dictionaries are large; accepted for now.** Android ships per-ABI splits / an app bundle (never a four-ABI universal APK); downloadable language data is revisited in M7. IPADIC is 47.5 MB and ko-dic 85.6 MB of build output, embedded raw. Release builds on this Intel Mac
(baseline = design commit, same machine and targets):

| | before | after | change |
|---|---|---|---|
| macOS `.app` (release) | 11 MB | 147 MB | +136 MB |
| macOS compressed disk image of the `.app` (`hdiutil` UDZO; `tauri build --bundles dmg` failed here on a Finder AppleScript timeout) | 5.9 MB | 45.0 MB | +39 MB |
| Android universal APK, **x86_64 only** (`.so` is stored uncompressed) | 14.1 MB | 153.9 MB | +140 MB |
| Android `libalstroemeria_lib.so` gzip -9 (what a compressed download would carry) | n/a | 37.7 MB | |

The macOS download is under §16.1's ~60 MB line; the Android APK file is not (about 154 MB per ABI as packaged, so a four-ABI universal APK would be roughly
600 MB; per-ABI splits or an app bundle would download roughly 40 MB per device). Everything works, so this is not a blocker, but it is the trigger §16.1 names
for moving dictionaries to downloadable "language data" in M7. Options: (a) accept for now and ship per-ABI/AAB on Android; (b) enable compressed native libs
for Android; (c) make `lang-ja`/`lang-ko` default-off features and download the dictionaries (pinned URL + sha256) in M7; (d) trim ko-dic/IPADIC (e.g. drop
unused columns) with a custom build. Chosen: (a); (b)–(d) stay open for M7. Also: the first build needs network (the Lindera dictionary crates download their source; md5-pinned).
`ko-hanja` adds 6 MB of data (raw; 2.4 MB compressed) on top.

Performance (5,000-character part): `Segmenter` alone on this Mac, release: ja 41 ms, ko 20 ms (`cargo run --release -p alst-text --example bench`).
Android x86_64 emulator, **release APK**, `get_part_view` end to end (tokenize + units + IPC): ja cold 457 ms then 178–347 ms with the cache; ko 231 ms,
then 201–222 ms. Debug builds are 3–10× slower (ja ~900 ms, ko up to 2.8 s). The spec's 100 ms threshold is exceeded, so `local_token_cache` (§5.3) is implemented
(keyed on part + text hash + language definition, 40 most recent parts). It only roughly halves the time: the rest is building units and serializing about
3,000 tokens with morphemes to JS, which is the next thing to trim (for example send `morphemes` only on demand).

Verified (`ui/src/lib/selftest-cjk.ts`; put `cjk` in the `selftest` file of the data folder) on macOS WebKit (release app) and the Android API 34 emulator (release
APK, WebView 113): dictionary import with rules and meta banks; `lookup_scan` returns 食べさせられた → 食べる with "causative, passive, past"; a Japanese article shows
grouped words (食べさせられた, 飲んでいた, 勉強している), furigana only over kanji (食/た), grammar colours, pitch colours after importing a pitch dictionary;
clicking a conjugated verb opens the editor with the lemma, "seen as", reasons, definition and pitch graph; setting a status applies to every inflection;
a Korean article shows particles split from nouns (학교|에서, 친구들|이, 학생|이에요), verb lemmas (공부했어요 → 공부하다) and the hanja section. A desktop screenshot
of the Korean reader was inspected by eye; the Android UI was verified through the DOM only.

Not verified / known gaps: Windows and iOS; Android UI by eye; real JMdict/Kanjium/frequency dictionaries (only the hand-made fixture dictionary was
imported, so lookup quality and import time on a 200k-entry dictionary are untested); real-world prose beyond the fixture paragraphs (IPADIC/ko-dic analysis
errors such as ko `줄` read as 주다 and ja `いた事` as one noun are the analyser's, not the grouping's); Korean hanja shows homophones (공부 gives five
candidates, best first); `ko.hanja` data provenance (BSD header only, see §16.10); the deinflector has no rules for -すぎる, -なさい, -ず, -ん, passive of する compounds
beyond the table; multi-word terms in ja are keyed by lemmas joined with a space, so terms saved before M5 in other languages are unaffected but ja has no migration (none existed).
`adb root` drops the `adb reverse` for `tauri android dev`; run `adb reverse tcp:1420 tcp:1420` again.

## M6 — LAN sync (build plan)

Design: SPEC §9 (read fully) and §4.1–4.4. Steps, each ending with tests passing:

1. **Store:** derived ids (§9.1) for new rows, including reviving tombstones; the one-time re-key step with
   tests (references, `_changes`, events, the deleted-plus-recreated collision); migration `0005_sync.sql`;
   `local_sync_clock` kept on every local write.
2. **Sync core:** `vector`, `changes_since`, `events_since`, `apply` with materialize and
   `local_sync_pending` (§9.2). New crate `crates/alst-sync` with the session state machine (§9.4) over a
   `Transport` trait. Property tests with `proptest` and an in-memory transport (§9.5), including
   sessions cut off mid-batch and batches ending inside a `(site, hlc)` group.
3. **Transport and pairing:** Noise over TCP with `snow` (§9.3), framing, device key pair, listener,
   pinned peers; code pairing (XX plus a 6-digit code) and QR pairing (`XXpsk3`). Loopback tests for: a
   pinned peer syncs, an unknown key is refused, a wrong PSK fails, codes match on both ends.
4. **Discovery:** `mdns-sd` advertise and browse, remembered addresses, manual address entry; Android
   multicast lock and permission in the Android shell.
5. **Assets:** media `have`/`want` with chunked transfer, sha256 check and per-peer policy; on-demand
   "Download now"; dictionary archives stored on import plus the `dictionaries` table and background
   re-import on peers; `.alext` stored on install plus `extension_packages` and install on peers (§9.6).
6. **UI:** Settings → Devices, pairing screens (code; QR shown with the `qrcode` crate as SVG, scanned with
   the Tauri barcode-scanner plugin on Android), top-bar sync indicator, sync triggers (§9.4), reader and
   lists refresh on `sync://applied`.
7. **Verify:** the macOS release app and the user's Android phone (Galaxy, arm64, connected over USB for
   `adb install`) on the same Wi-Fi: pair by code and by QR; edit terms on both offline, then sync, and
   both edits survive; import a podcast episode on the Mac and see it play on the phone; import a
   dictionary on the Mac and look a word up on the phone. Record timings for a first full sync.

Phone test builds: `npm run tauri android build -- --apk --target aarch64` (rustup Rust must come first
on `PATH`), then `zipalign -P 16 4` and `apksigner sign` with `~/.android/debug.keystore` (the standard
debug-key password), then `adb install -r`. Always sign with that same key, so installs keep the user's data.

Out of scope: see the end of SPEC §9.

### M6 status

Done: steps 1–7. Migration `0005_sync.sql` (sync clock, pending rows, `dictionaries` and `extension_packages` synced tables, peer columns); derived ids for
natural-key rows with a one-time re-key (`rekey.rs`, tested for references, `_changes`, events and delete-and-recreate); `alst-core::sync` (`next_batch`, `apply`
with materialize and pending rows); new crate `alst-sync` (session protocol, Noise `XX` / `XXpsk3` over TCP, pairing by code and QR, mDNS, asset exchange with
per-peer media policy, `Service`); dictionaries and `.alext` packages are stored as media on import and installed on peers after a sync (`alst_dict::reconcile`,
`alst_ext::install::reconcile`); Devices tab, QR display and scanner, top-bar sync indicator, sync triggers (start, 30 s after a local write, every 10 min, on
discovery), readers and lists refresh on `sync://applied`, "Download now" for files not yet on a device. The SPEC §9.5 property tests (convergence, sync twice
changes nothing, order independence, cut-off resume, concurrent edits, delete vs revive) pass; breaking the same-`(site, hlc)` group handling on purpose makes them fail.
`cargo test --workspace` and `npm test` (66) pass. Spec edits in §9.2 and §9.4: `next_batch` sends changes and events cut at one key; foreign keys are switched off
(not deferred) while applying a batch; sessions run one direction at a time.

Verified on the macOS release app and a Samsung Galaxy S26-class phone (SM-S948U, Android 17, arm64 release APK, debug key), same Wi-Fi: pairing by code; pairing by QR
(camera scanner); automatic discovery over mDNS; term status edits made on both devices while the phone was offline both survived the sync; a podcast episode in the
Mac library played on the phone; a dictionary imported on the Mac was installed on the phone and gave definitions in the reader; Remove on both sides.
Timings from the phone's log: an incremental sync with a few rows takes 38–92 ms (e.g. 12 rows received 92 ms, 1–2 rows sent 38 ms). A cold first full sync was not
captured, because the library was already shared by the time logging started; the library here is small.

Found only on the device (fixed): Android 17 (targetSdk 37) silently drops LAN connections and mDNS until `ACCESS_LOCAL_NETWORK` is granted; the manifest declares it
and `MainActivity` requests it, but on this phone no prompt appeared and the user had to enable it in Settings, so check that again. The release build (R8) crashed the
barcode scanner, so `proguard-rules.pro` now disables obfuscation and optimization. `confirm()` does nothing in the Tauri webview, so Remove and extension Uninstall
use an inline confirm (no `confirm()` calls remain in the UI). The top bar sat under the status bar and camera cutout (`viewport-fit=cover` plus a safe-area top padding).

Not verified / known gaps: Windows, Linux and iOS; mDNS across more than one Mac/phone pair (only tested here and on loopback); the local-network permission prompt on
Android 17; a large library or large media over Wi-Fi (no throughput figures); the reader's "not downloaded" notice says "another paired device" rather than naming it;
no UI to delete a term; media policy defaults on Android (video off, 500 MB cap) were not exercised on the phone.

## M7 — Releases (build plan)

Design: SPEC §17 (read fully), plus §16.1 and §9.6 for language data. Decisions already made by the user: public GitHub
repo, no paid code signing, downloadable language data, name and app id are final. Steps, each ending with tests passing:

1. **Language data (§17.3):** `scripts/build-langdata.sh`, package format, pinned `crates/alst-text/langdata.json`
   (placeholder URLs until step 6), Lindera loading from a directory, download manager in a crate (sha256, resume,
   cancel; unit tests with a local HTTP fixture), "Import from file", reader and Settings → Languages prompts,
   LAN sync of packages as `langdata` media. Release builds without the embed features; record the new macOS app and
   Android APK sizes in the status.
2. **Release configuration (§17.1, §17.2, §17.6):** `scripts/bump-version.mjs`, `CHANGELOG.md`, bundle settings per
   platform, Android release signing from `keystore.properties`/env, ABI splits, `versionCode`; generate the updater
   key and release keystore into `~/.alstroemeria/` (never the repo) and tell the user to back that folder up;
   `cargo deny` licence policy; `THIRD-PARTY-NOTICES.md` linked from About. Replace M6's blanket R8 disable in
   `proguard-rules.pro` with keep rules for the barcode scanner, and confirm the scanner still works in a release build.
3. **Updates (§17.4):** desktop updater with banner, "Check now" and the opt-out; Android "new version" banner linking
   to the release page. Tests for version comparison and for a `latest.json` with a bad signature being refused.
4. **M6 gaps:** Android 17 local-network permission (request when the Devices tab or pairing opens; if denied or never
   prompted, explain and offer a button that opens the app's system settings); the "not downloaded" notice names
   the device that has the file; a sync benchmark over loopback with a generated library (20k terms, 300 parts, 200 MB of
   media) recording first-sync time and MB/s; exercise the Android media-policy defaults (video off, 500 MB cap).
5. **CI (§17.5):** `ci.yml`, `release.yml` (draft releases, `latest.json`), `langdata.yml`. Validate the workflow files
   locally (e.g. `actionlint`) since they can't run before the repo exists.
6. **Publish (needs the user):** ask the user to create the public GitHub repo and give its URL. Offer to rewrite commit
   authors to their GitHub no-reply address before the first push (history is unpushed, so this is safe) and do it only
   if they agree. Ask before pushing. The user adds the Actions secrets themselves. Run `langdata.yml` for both
   packages and fill in the real URLs and sha256 values. Bump to `0.1.0`, tag, and let `release.yml` make the draft.
7. **Verify:** install the macOS `.dmg` from the draft (documenting the "Open anyway" step); on the Galaxy, follow §17.2's
   switch from the debug-key build (sync to the Mac, uninstall, install the release APK, re-pair, library returns);
   download Japanese language data on one device and get Korean from a peer over LAN; tag `0.1.1` and update the Mac
   app through the updater, and see the Android banner. Windows and Linux are verified only if the user has a machine to
   run them; otherwise record "built by CI, not run".

Out of scope: paid signing and notarization, Play Store and F-Droid listings, Android self-update, iOS (M10), delete-term UI
and other feature requests (they come from the user's testing notes after M7).

### M7 status

Done: steps 1–7. Public repo `https://github.com/Rowawn/Alstroemeria` (history rewritten to the `Rowawn` no-reply identity before the first push). CI on push
(Ubuntu tests, Windows check, licence check). `release.yml`: a tag `v<version>` must equal the version in `tauri.conf.json`; a desktop matrix (tauri-action) builds the
installers and a signed `latest.json` into a draft release; an Android job builds the release-signed APKs; the changelog section is the release body; the user
publishes the draft. `scripts/bump-version.mjs` keeps `tauri.conf.json`, `Cargo.toml`, `package.json`, `Cargo.lock` and the Android `versionCode`
(major*10000 + minor*100 + patch) in step. Updater (`tauri-plugin-updater`, minisign-style key, feed `releases/latest/download/latest.json`): desktop updates in place;
Android shows a banner linking to the release page. Release signing: Android keystore and the updater key live in `~/.alstroemeria/` and in the CI secrets.
Japanese (`ja-ipadic`) and Korean (`ko-kodic`) dictionaries are no longer embedded: zip packages are published as `langdata-<id>-<version>` releases by
`langdata.yml`, pinned with size and sha256 in `crates/alst-text/langdata.json`, and are downloaded, imported from a file, or synced between peers over LAN
(media mime `application/x-alstroemeria-langdata`). Releases `v0.1.0` and `v0.1.1` are published. Changelog in `CHANGELOG.md`.
`cargo test --workspace`, `npm test` (73) and `npm run test:scripts` (6) pass. One test fix found by CI: term lists ordered by `created_at, id` were not
deterministic, now `created_at, rowid` (`store.rs`).

Sizes: macOS .app 18.2 MiB (was about 145 with the embedded dictionaries); Android arm64 APK 22.2 MB (arm 18.7, x86_64 23.8, x86 23.4).
Benchmark over loopback (20k terms, 300 parts, 200 MB media): first sync 4.05 s (about 52 MB/s); a second sync with nothing new 211 ms.

Verified on devices: macOS `.dmg` install and run; the Galaxy (SM-S948U, Android 17) switch from the debug-key build to the release-signed APK (sync to the Mac,
uninstall, install, re-pair, library returned); the local-network permission prompt now appears by itself; the barcode scanner works under R8; media policy and
media download; Japanese data downloaded on the Mac and synced to the phone; Korean data synced Mac to phone over LAN; Windows installed (after the unknown-publisher
warning) with video and audio playing; the updater: 0.1.0 to 0.1.1 worked on macOS and the Android banner appeared and linked to the release.

Known gaps:
- Video on Android: the Galaxy (Chrome 155 WebView) shows the picture and starts the MP4 (H.264 High L3.1 + AAC), then pauses with MediaError code 3,
  `PIPELINE_ERROR_DECODE: Failed to send audio packet for decoding` (about 6.7 s in); pressing play again continues, so it recurs. Audio-only files play. The failure is in
  the audio track of the MP4 (check its codec, profile and channel layout with `ffprobe`); the file's mono or 5.1 AAC or an unusual profile is the first suspect.
  The 0.1.1 message with the error code made this visible. Next: test a file with stereo AAC-LC, and consider a server-side or import-time remux note.
- Sync: a large Android to Windows sync failed once, then succeeded on an immediate retry; the first sync across different Wi-Fi networks gave a network error
  until both devices were on the same network and both apps were restarted (the error text does not say so).
- Linux is built by CI only, not run. Windows and macOS builds are unsigned (macOS first run needs right-click Open; Windows shows "unknown publisher"). The
  Windows and Linux updaters were not run.
- No UI to delete a work or change its language (a wrong-language work was removed with a one-off store call; its orphan media file remains), nor to delete a term.
- `actionlint` was never run on the workflows. The `langdata.yml` summary step failed once for `ja-ipadic` although the release was published.
- `rowawn@pm.me` appears in commit `fc60642` (public).
- The self-test and Japanese `tauri dev` flows need the embed feature or downloaded data.
- Back up `~/.alstroemeria/` (`updater.key`, `release.jks`, `passwords.env`): losing it breaks updates and Android upgrades.

Next: the user tests and records findings; an overhaul and refinement phase is then planned from them.

## M8 — Content packs (build plan)

Design: SPEC §10 (read fully), plus §8 and §6.9.10 for the existing importers and §9.1 for derived ids. Steps, each ending with
tests passing:

1. **Format:** `schemas/pack.schema.json`; new crate `crates/alst-pack` that reads and validates a pack (zip safety, limits,
   licence allowlist, URL schemes), with tests on hand-made fixture packs in `crates/alst-pack/tests/fixtures/` (valid, too
   large, path traversal, unknown `formatVersion`, bad URL, embedded content with a non-allowlisted licence).
2. **One import path:** move the podcast-episode, YouTube and media-file orchestration from `src-tauri/src/media.rs` into crate
   code (`alst-pack` or `alst-text::import`, whichever keeps dependencies simple), keep the existing UI commands working on top
   of it, and add `direct` (by file type) and `embedded` (from the zip, Markdown to plain text). Existing import tests keep passing.
3. **Store:** migration `0006_packs.sql` (`packs`, `works.pack_item`); store functions for packs, "already imported" lookup by
   `(pack_id, pack_item)`, seed vocab (create missing terms at status 0, fill only empty fields, never change status). Tests,
   including a re-import that adds only new items, and a sync test showing two devices that import the same pack converge.
4. **Plan and run:** `open` returns the plan of §10.3; `run` imports the chosen items in the background with progress events,
   cancel and per-item errors. Tests with a local HTTP fixture server (an article page, a small RSS feed with a transcript,
   an SRT file).
5. **UI:** "Import pack…" in the Library, drag-and-drop, review screen, progress and "Retry", pack filter and pack view
   with attributions, attribution line at the end of a work. Desktop `.alpack` file association. `npm test` covers the
   review-screen state.
6. **Skill:** `.claude/skills/build-pack/SKILL.md`, `scripts/pack/` (`probe` as an `alst-pack` example, `rss`, `yt`, `build`),
   `packs/` gitignored. Then make one real pack with the skill (Spanish, automotive, B1, small counts to save credits) and
   record how many tool calls and roughly how many tokens it took.
7. **Verify:** import that pack on the macOS app and on the Galaxy (release build; the user's phone): review screen, a
   `youtube-subs` item showing "Import on a desktop" on Android and arriving there by sync after the Mac imports it, seed
   vocab in the reader popup, re-import skipping existing items. Write "M8 status" here.

Out of scope: see SPEC §10.5.

### M8 status

Done: steps 1–7. New crate `crates/alst-pack` (manifest parse and validation, zip safety, plan, run, and `sources`, the shared fetch-and-store path that
`src-tauri/src/media.rs` now calls for podcast, YouTube and media-file import); `schemas/pack.schema.json`; migration `0006_packs.sql` (`packs`, `works.pack_item`);
store functions `put_pack`, `create_work_in_pack`, `imported_pack_items`, `seed_counts`, `seed_terms` (create at status 0, fill only empty fields, never touch status);
Markdown importer in `alst-text`; `direct` and `embedded` strategies by file type; Tauri commands in `src-tauri/src/pack.rs` (open by path or chunked upload,
plan, run with `alst:pack-progress`, cancel, close, `.alpack` file association); UI: "Import pack…", drag-and-drop, review screen with progress and Retry, pack filter and
pack view with attributions, attribution line at the end of the last part. Skill: `.claude/skills/build-pack/SKILL.md` and `scripts/pack/{probe,rss,yt,build}.mjs`
(`probe`, `rss`, `build` call the `alst-pack` example `probe`); `packs/` is gitignored. SPEC §10.7 records the implementation choices.
`cargo test --workspace`, `npm test` (76) and `npm run check` pass. New tests: pack fixtures (valid, bad URL, unknown version, licence warning, traversal, symlink,
limits), plan and run against a local HTTP server (article, podcast with transcript, direct, embedded text and media; failed item and retry; re-import and
new version; cancel; seeding), store (origin, seed rules), a two-device pack sync convergence test, and review-screen state (Vitest).

The real pack (`packs/es-automotive.alpack`, 6 items, 20 vocab: 3 articles, 2 PodCars episodes with transcripts, 1 video, B1) took about 12 tool calls
(4 web searches, 7 shell calls, 1 draft write) and roughly 15k tokens, most of it the search results; the probes returned only short JSON. Findings: of 15 Spanish
motor podcast feeds only one (PodCars) had `podcast:transcript`; the videos found had only auto subtitles, so the pack uses auto.

Verified on devices: release-key-signed build installed over 0.1.1 on the Galaxy (data kept, migration applied); the picker could not select `.alpack` while the
input had an `accept` filter (fixed: no filter); review screen with the YouTube item unchecked as "Import on a desktop"; podcast episode imported with audio and cue-synced
transcript; re-import showed the items as already imported; seed vocabulary showed in the reader popup; attribution line at the end of an article; the YouTube item
imported on the Mac (dev build, real data) arrived on the phone by sync.

Known gaps:
- Cancel acts between items; a download in progress finishes. Retry re-runs the whole item.
- The JSON schema and the Rust validator are kept equal by hand; no test compares them.
- The Android `.alpack` file association (intent filter generated by Tauri) was not tried; packs open through the picker. Windows and Linux associations and
  command-line opening were not run; macOS `Opened` was not tried with a double-click.
- `direct` needs a known file extension in the address; media in `direct` needs a transcript URL.
- No UI to delete a pack or its works; the pack view lists only works present on this device.

Not committed (the user approves).
