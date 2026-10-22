# Alstroemeria — Technical Specification

Status: v0.1 (foundation spec, written before any code). Working name: **Alstroemeria**.
This document is the source of truth for architecture decisions. Change it deliberately, in the
same commit as the code that depends on the change.

Sections are numbered so sessions can reference them (`SPEC §6.3`) and read only what they need.

---

## 1. Purpose and principles

A lightweight, local-first app for language acquisition through immersion (articles, books,
podcasts, video, music with lyrics), with vocabulary tracking, dictionaries and Anki export —
in the tradition of Lute and LingQ — plus:

1. **Cross-platform, one codebase:** Windows, macOS, Linux, Android; iOS/iPadOS later.
2. **Local-first, peer-to-peer sync** over the local network. No server, no account.
3. **Everything is an extension.** Languages, language-specific tools, language-universal tools
   and large features (SRS, shadowing) all plug into one versioned API.
4. **Light:** small installers, fast startup, low memory, works fully offline.
5. **Portable content:** an importable *content pack* format that an AI agent or a person can produce.

Non-goals for v1: cloud accounts, AnkiWeb sync, running Anki add-ons, social features.

## 2. Platforms and stack

| Layer | Choice |
|---|---|
| App shell | Tauri 2 |
| Core logic | Rust (Cargo workspace of small crates) |
| UI | Svelte 5 + TypeScript + Vite |
| Storage | SQLite (`rusqlite`, bundled), one database per device + content-addressed media folder |
| CJK tokenizing | Lindera (ja: UniDic or IPADIC, ko: ko-dic, zh: CC-CEDICT) — added in M5 |
| Scheduler (later) | `fsrs-rs` |

Platform notes:
- **Android:** built with `tauri android`. Sync runs only while the app is in the foreground.
- **iOS/iPadOS (M10):** requires full Xcode and an Apple Developer account. Needs
  `NSLocalNetworkUsageDescription` and `NSBonjourServices` in Info.plist for LAN sync.
  App Store rules forbid downloading native code → see §6.4.
- Video/audio play through the platform webview's `<video>/<audio>`; codec support varies on
  Android (prefer H.264/AAC, MP3, Opus/WebM where available).

## 3. Architecture overview

```
┌──────────────────────── UI (Svelte, webview) ─────────────────────────┐
│ Library · Reader · Term editor · Dictionary popup · Settings          │
│ Extension host: one Web Worker per JS/WASM extension (sandboxed)      │
└───────────────▲──────────────────────────────────▲────────────────────┘
                │ Tauri commands/events (typed)     │ host API (RPC via UI)
┌───────────────┴──────────────── Rust core ───────┴────────────────────┐
│ alst-core   DB, migrations, models, HLC, change capture, event log     │
│ alst-text   tokenizer engines, term matching, EPUB/SRT/VTT/LRC parsing │
│ alst-ext    manifests, registry, capability resolution, native tier    │
│ alst-dict   Yomitan dictionary import + lookup                         │
│ alst-sync   discovery, pairing, change exchange (M6)                   │
│ alst-pack   .alpack import/validation (M8)                             │
└────────────────────────────────────────────────────────────────────────┘
```

Rules:
- `src-tauri` is a **thin shell**: command handlers delegate to crates. No business logic there.
- Crates must not depend on Tauri, so they are testable with plain `cargo test`.
- Heavy work (tokenizing, dictionary search, parsing, audio, later ML) lives in the native core and
  is exposed to extensions as **core services** (§6.6). This is how extensions get native speed
  on every platform, including iOS.

## 4. Data model

### 4.1 Conventions

- IDs: UUIDv7 strings (time-ordered, globally unique across devices).
- Every **synced table** has: `id TEXT PRIMARY KEY`, `created_at INTEGER` (unix ms),
  `deleted INTEGER NOT NULL DEFAULT 0` (tombstone; rows are never hard-deleted while sync exists).
  Per-column clocks live in `_changes` (§4.4), not on the row.
- All writes to synced tables go through the core's write layer (`alst-core::store`), never
  raw SQL from elsewhere, so change capture can't be bypassed.
- **Local-only** tables (caches, device secrets, imported dictionary data) are prefixed `local_`.
- Schema changes are numbered SQL migrations in `crates/alst-core/migrations/`.
- Any new table that follows these conventions syncs automatically (§9). New modules get sync for free.

### 4.2 Core tables (synced unless noted)

```
works          id, language, title, kind(text|epub|article|audio|video|lyrics|subtitle),
               author, source_url, source_license, pack_id, tags(json), archived,
               settings(json)
parts          id, work_id, idx, title, text, media_hash, timing(json: cue list), word_count
               -- a chapter, page, episode, or song; reader opens one part at a time
concepts       id, language, kind(lexeme|grammar|phrase), key, meta(json)
               -- the unit of knowledge; terms point to it; future SRS schedules on it (§11)
terms          id, language, text, text_lc, lemma, concept_id, status, translation,
               reading, notes, tags(json), word_count
term_parents   id, term_id, parent_id                -- many-to-many, as in Lute
media          id(=sha256), mime, size, original_name -- file stored at media/<ab>/<hash>
extension_state id(=extension id), enabled, settings(json), languages(json)
ext_data       id, extension_id, key, value(json)    -- per-extension synced key/value store
settings       id(=key), value(json)                 -- synced user settings
events         see §4.3 (append-only, synced as a union)
local_settings key, value                            -- device-only settings      (local)
local_peers    site_id, name, cert_fingerprint, last_seen, last_vector(json)   (local)
local_dict_*   imported dictionary entries/indexes   (local; large, rebuilt per device)
local_token_cache part_id, ext_fingerprint, tokens(blob)                       (local)
```

**Term status** (compatible with Lute): `0` new/unknown, `1–5` learning stages, `98` ignored,
`99` well-known. `text_lc` uses the language's normalizer (§5.2). Multi-word terms have
`word_count > 1`.

### 4.3 Event log

`events(id, hlc, site_id, type, language, subject_type, subject_id, payload json)`

Append-only, never updated. It records what happened, so future modules (statistics, SRS,
"passive exposure" credit) can replay history that already exists. Initial types:

| type | payload |
|---|---|
| `term.status` | `{from, to}` |
| `term.lookup` | `{source: "popup"|"dict", dict_id}` |
| `part.read` | `{words_read, unknown_count, duration_ms}` |
| `media.listen` | `{part_id, seconds}` |
| reserved: `review`, `exposure` | defined by the SRS module (M9) |

Extensions may emit events namespaced by their id (`<ext-id>/<type>`).

### 4.4 Change capture (for sync)

`_changes(tbl, row_id, col, value, hlc, site_id)` holds the latest clock per (table, row, column).
The write layer upserts here whenever it writes a synced column. Conflicts resolve
**per column, last-writer-wins** by `(hlc, site_id)`. So if one device changes a term's status
and another edits its translation at the same time, both edits survive.

**HLC:** 64-bit value: 48 bits of wall-clock ms + 16-bit logical counter; ties broken by
`site_id` (a random 128-bit id generated on first launch, stored in `local_settings`).

## 5. Text pipeline

### 5.1 Flow
`import → work + parts → (tokenize → annotate) → render → user actions → terms/events`

### 5.2 Tokens
A tokenizer produces, per part, a list of sentences, each a list of tokens:

```ts
interface Token {
  i: number;            // index within part
  text: string;         // surface form
  isWord: boolean;      // false for punctuation/whitespace
  norm: string;         // normalized lookup form (lowercase, char substitutions)
  lemma?: string;       // base form, if the language provides `<lang>.lemma`
  start: number; end: number;   // UTF-16 offsets into part text (matches JS strings)
  ann: Record<string, unknown>; // annotations keyed by capability, e.g. "ja.reading": "かんじ"
}
```

### 5.3 Term matching
After tokenizing, the core matches **multi-word terms** greedily (longest match first) over
`norm` sequences, using a per-language trie of `terms.text_lc` where `word_count > 1`.
Languages without spaces (ja, zh) match on token sequences in the same way.
Results are cached in `local_token_cache`, keyed by part + a fingerprint of the active
extensions, so changing extensions invalidates the cache.

### 5.4 Rendering
The reader renders each token as a span carrying status and annotation-driven **render
hints** (§6.5). Extensions never get direct DOM access to the reader.

## 6. Extension system

### 6.1 Kinds

| kind | Purpose | Example |
|---|---|---|
| `language` | Defines a language: tokenizer, normalization, base capabilities | `lang.es`, `lang.ja` |
| `addon` | Builds on capabilities; language-specific or universal | `ja.pitch-overlay`, `ko.hanja`, `grammar-colors` |
| `module` | A large feature with its own screens and tables | `srs`, `shadowing` |

All three kinds use the same manifest and API. First-party modules use only the public API,
so if the API can host the SRS module, it can host anything.

### 6.2 Manifest (`extension.json`)

```json
{
  "id": "ja.pitch-overlay",
  "name": "Pitch Accent Overlay",
  "version": "1.0.0",
  "kind": "addon",
  "apiVersion": "^1.0",
  "targets": ["ja"],
  "provides": ["ja.pitch-render"],
  "requires": ["ja.reading", "ja.pitch"],
  "optional": ["*.pos"],
  "runtime": { "type": "js", "entry": "main.js" },
  "platforms": ["windows", "macos", "linux", "android", "ios"],
  "permissions": ["net:www.example-dict.org", "audio"],
  "contributes": {
    "decorators": [{ "id": "pitch", "priority": 50 }],
    "panels": [],
    "commands": [],
    "lookupProviders": [],
    "importers": [],
    "exporters": [],
    "activities": [],
    "settings": { "$schema": "JSON Schema for this extension's settings" }
  }
}
```

- `targets`: language ids, or `"*"` for language-universal. `kind: "language"` uses `"language": "<id>"` instead.
- `platforms`: defaults to all platforms for non-native runtimes; required for `native`.
- JSON Schemas for manifests live in `schemas/` and are validated on install.

### 6.3 Capabilities

Capabilities are named data or services, `<scope>.<name>`. Scope is a language id, `core`, or
`*` in a `requires` (meaning "this capability for whatever language is active").

- Language extensions provide base capabilities: `<lang>.tokenize`, `<lang>.lemma`,
  `<lang>.reading`, `<lang>.pos`, `<lang>.pitch`, `<lang>.hanja`, …
- Add-ons `require` capabilities, not specific extensions. Any extension providing `ja.pitch`
  satisfies a pitch-accent overlay.
- The registry (`alst-ext`) builds a dependency graph per language, orders annotators
  topologically, and disables (with an explanation in the UI) extensions whose requirements
  aren't met.
- Exactly one provider per capability per language is active. If several are installed, the user picks.

### 6.4 Runtimes

| runtime | Where it runs | Platforms | Notes |
|---|---|---|---|
| `declarative` | Core interprets JSON | all | Simple languages need nothing else (see §6.7) |
| `js` | Dedicated Web Worker in the webview | all | Default for add-ons |
| `wasm` | Web Worker, loaded via a JS shim | all | **Rust compiles here.** Recommended for heavy extension logic |
| `native` | Dynamic library loaded by the core (C ABI) | desktop, Android | **Unsandboxed**, labelled as such in the UI. Not on iOS. Reserved; implemented in M11 |

Why JS/WASM run in the webview rather than in a Rust-side WASM engine: iOS forbids JIT
outside WebKit, while the webview JIT-compiles WASM on every platform. Popular native
extensions can be **promoted into the core**, which makes them available on iOS too.

**Sandbox:** workers have no DOM access. The app's CSP sets `connect-src` to IPC only, so
network access goes through `alst.net.fetch`, which the core checks against the manifest's
`net:` permissions.

### 6.5 Extension points

| Point | Kinds | Contract |
|---|---|---|
| tokenizer | language | `text → Sentence[]`, or select a core engine declaratively. One per language |
| normalizer | language | `string → string` for `norm` / `text_lc` |
| annotator | language, addon | `(tokens) → per-token annotations` under capability keys it `provides` |
| decorator | addon | `(token, ann) → RenderHint[]` (declarative; see below) |
| lookupProvider | language, addon | `(token, context) → LookupResult[]` shown in the popup |
| panel | addon, module | sidebar/popup tab, rendered in a sandboxed `<iframe>` that talks to the host via `postMessage` |
| command | all | an action for the command palette or context menu |
| importer | addon, module | new source type → works + parts |
| exporter | addon, module | terms/sentences → file or external app (Anki TSV, AnkiConnect) |
| activity | module | a full screen of its own (SRS review, shadowing) |
| eventListener | all | subscribe to the event log (read-only) |

**Render hints** form a small fixed vocabulary that the reader knows how to draw:
`ruby{text}` (furigana/romanization above), `underline{style,color}`, `color{token: role}`,
`background{role}`, `badge{text}`, `pitchLine{pattern}`, `tooltip{text}`.
Colors are semantic *roles* (`noun`, `verb`, `particle`, `accent-high`, …) that themes map to
actual colors, so dark mode and accessibility stay consistent. Hints from several decorators
layer in `priority` order, and the user can reorder them.

### 6.6 Host API (v1, sketch)

Available to JS/WASM extensions as `alst.*`. Calls are async RPC via the UI to the core.

```
alst.core.tokenize(engine, text, opts)    // native tokenizer engines (regex, lindera-ja/ko/zh, char)
alst.dict.lookup(language, query, opts)   // installed dictionaries (Yomitan format)
alst.terms.get(ids|text) / .update(id, patch) / .search(q)
alst.concepts.get / .link(termId, conceptId)
alst.events.emit(type, payload) / .on(type, cb)
alst.storage.get/set/delete(key)          // synced, namespaced to the extension (ext_data)
alst.net.fetch(url, init)                 // only hosts allowed by the manifest
alst.media.play(hash, from, to) / .segment(...)   // audio control for shadowing etc.
alst.ui.toast / .openPanel / .openActivity
alst.settings.get()                       // validated against the manifest's settings schema
```

The API is semver-versioned. Breaking changes need a new major version; the host refuses
extensions whose `apiVersion` range it can't satisfy and explains why.

### 6.7 Declarative language example

```json
{
  "id": "lang.es", "name": "Spanish", "version": "1.0.0", "kind": "language",
  "language": "es", "apiVersion": "^1.0",
  "runtime": { "type": "declarative" },
  "provides": ["es.tokenize"],
  "declarative": {
    "tokenizer": { "engine": "regex", "wordChars": "a-zA-ZÀ-ÖØ-öø-ÿ'’-" },
    "sentenceEnd": ".!?…",
    "exceptions": ["Sr.", "Sra.", "EE.UU."],
    "charSubstitutions": { "’": "'" },
    "rtl": false,
    "lookupUrls": [
      { "name": "WordReference", "url": "https://www.wordreference.com/es/en/translation.asp?spen=###" }
    ]
  }
}
```

A CJK language selects a core engine: `"tokenizer": { "engine": "lindera", "dict": "unidic" }`.

### 6.8 Distribution
v1: install from a local `.alext` file (a zip of the extension folder), plus first-party extensions
bundled with the app. A registry/catalogue is out of scope until later.

## 7. Dictionaries

- **Yomitan format** (zip of `index.json` + `term_bank_*.json`, `term_meta_bank_*.json`,
  `kanji_bank_*.json`) is the primary import format. It covers JMdict, pitch accent and frequency
  data, and many Korean and Chinese dictionaries. Imported into `local_dict_*` tables with an
  index on headword and reading.
- **URL lookups** (`###` placeholder) open in an embedded view or the system browser.
- Dictionary *configuration* syncs; dictionary *data* is imported per device. Data files can
  optionally sync as media.
- Licensing: JMdict/EDICT are CC-BY-SA, so show attribution in About. Yomitan's code is GPL-3.0:
  don't copy it into the core; write deinflection independently or ship it in a separately
  licensed extension.

## 8. Media and import

| Source | M | Handling |
|---|---|---|
| Plain text, paste | 2 | Split into parts by size or blank lines |
| EPUB | 2 | Chapters become parts; images go to media |
| Web article (URL) | 2 | Fetched by core; readability extraction |
| Subtitles SRT/VTT, lyrics LRC | 4 | Cues become `parts.timing`; the reader highlights and seeks |
| Audio/video file + subtitles | 4 | File goes to media store; cue-synced reader |
| Podcast RSS | 4 | Importer extension; transcript if provided, else optional Whisper (M11) |
| YouTube subtitles | 4 | Desktop only, via a `yt-dlp` binary the user installs |

Media files are content-addressed (`sha256`) and stored under `media/ab/abcdef…`, so they are
deduplicated and sync-friendly.

## 9. LAN sync (M6)

- **Discovery:** mDNS service `_alstroemeria._tcp`.
- **Pairing:** device A shows a QR code (or six-digit code) encoding
  `{site_id, name, addr, port, cert_fingerprint, one_time_secret}`. B connects, both pin each
  other's self-signed certificate fingerprints (`local_peers`). From then on, only pinned peers.
- **Transport:** WebSocket over TLS (rustls).
- **Protocol:**
  1. `hello {site_id, schema_version, vector}`. `vector` maps site_id → highest HLC seen from that site.
  2. Each side sends `_changes` rows newer than the peer's vector, in batches.
  3. The receiver applies each column change if `(hlc, site)` is newer than its own clock.
  4. `events` merge as a set union by id.
  5. Media: exchange `have/want` lists of hashes; transfer lazily, filtered by a per-device
     policy (e.g. "phone: no video").
- **Schema version mismatch:** refuse to sync and tell the user which device to update.

## 10. Content packs (`.alpack`, M8)

A zip with `manifest.json` and an optional `items/` folder.

```json
{
  "format": "alstroemeria-pack",
  "formatVersion": 1,
  "id": "uuid",
  "title": "Spanish: automotive vocabulary",
  "language": "es",
  "topic": "automotive",
  "level": "B1",
  "description": "…",
  "createdBy": "human | agent:<name>",
  "createdAt": "2026-10-22T00:00:00Z",
  "items": [
    {
      "id": "a1",
      "kind": "article | podcast | video | book | lyrics | text",
      "title": "…",
      "source": { "url": "…", "license": "CC-BY-4.0 | unknown | …", "attribution": "…" },
      "content": { "mode": "fetch", "strategy": "readability | rss-episode | youtube-subs | direct" },
      "tags": ["engine", "maintenance"]
    },
    {
      "id": "a2", "kind": "text", "title": "…",
      "source": { "url": "…", "license": "CC-BY-SA-4.0", "attribution": "…" },
      "content": { "mode": "embedded", "path": "items/a2.md" }
    }
  ],
  "vocab": [ { "text": "bujía", "translation": "spark plug", "notes": "" } ]
}
```

- **`fetch` mode (default):** the pack only references the source. The app fetches and extracts it
  on import. This keeps packs small and avoids redistributing copyrighted text.
- **`embedded` mode:** only for openly licensed or user-owned content.
- On import, the app shows the item list with sources and licenses, then creates works with
  `pack_id` set. Seed vocabulary becomes terms with status 0 and the translations filled in.
- **Agent workflow:** a Claude Code skill (`.claude/skills/build-pack/`). Claude chooses sources
  and writes metadata. Local scripts handle fetching, extraction and transcripts, so the model
  never reads whole pages. Items per kind are capped.

## 11. Anki and the SRS module

**Now (M2):** export terms and sentences to Anki-importable TSV. Desktop: AnkiConnect exporter.

**Later (M9) — `module: srs`:**
- FSRS scheduling via `fsrs-rs`. Cards render HTML/CSS templates in the webview.
- `.apkg` import and export: notes, note types, templates, media and review history
  (handle both `collection.anki2` and zstd-compressed `collection.anki21b`).
  Compatible with Anki's files; no AnkiWeb sync; Anki add-ons not supported.
- **Concept-linked cards:** cards link to one or more `concepts`. Memory state can be kept per
  concept, with the cards acting as an example pool. At review time, the scheduler picks an
  example not seen recently, and the rating updates the concept. Grouping uses
  `<lang>.lemma`/`concept_id` for words and `*.grammar` for patterns.
- **Immersion feedback:** `exposure` events from reading and listening can lightly count
  toward a concept's memory state.
- The scheduling policy is itself swappable (an extension point within the module), because
  concept-level scheduling is experimental.

## 12. Repository layout

```
/
├─ CLAUDE.md                 short project guide for agents
├─ Cargo.toml                Rust workspace
├─ package.json              UI workspace (npm)
├─ docs/SPEC.md, docs/ROADMAP.md
├─ schemas/                  JSON Schemas: extension manifest, pack manifest
├─ crates/
│  ├─ alst-core/             db, migrations/, store (write layer), hlc, events
│  ├─ alst-text/             tokenizer engines, term matching, epub/srt/vtt/lrc parsing
│  ├─ alst-ext/              manifest parsing, registry, capability graph
│  ├─ alst-dict/             (M2) Yomitan import + lookup
│  ├─ alst-sync/             (M6)
│  └─ alst-pack/             (M8)
├─ src-tauri/                thin Tauri shell (commands → crates)
├─ ui/                       Svelte app + extension worker host
└─ extensions/               first-party: lang-es, lang-ja, lang-ko, addon-*, module-*
```

Crates are created when their milestone starts, not before.

## 13. Testing

- Rust: `cargo test --workspace`. Each crate has unit tests; tokenizers get **golden tests**
  (fixture text → expected tokens JSON) in `crates/alst-text/tests/fixtures/<lang>/`.
- Migrations: a test applies all migrations to an empty DB and checks the schema.
- Sync (M6): property tests showing that two replicas converge whatever order changes are applied in.
- UI: Vitest for logic (render-hint layering, RPC); manual checks for visuals.
- Schemas: every first-party manifest and example pack validates in CI.

## 14. Licensing

- **Core app** (everything outside `extensions/`): **MPL-2.0** (`LICENSE` at the repo root).
  Modifications to MPL-licensed files must stay open, but the app can be combined with code under
  any license, it's compatible with app-store distribution (incl. iOS), and third-party
  extensions may use any license.
- **First-party extensions** (`extensions/*`): **MIT**, so they can be freely copied as templates.
  Each extension folder carries its own `LICENSE` and sets `"license"` in its manifest.
- Dependencies must be MPL-compatible (MIT, Apache-2.0, BSD, ISC, Zlib, MPL, Unicode are fine).
  **No GPL/AGPL/LGPL code** in the core, which rules out copying Yomitan code (§7).
- Cargo crates set `license = "MPL-2.0"`; `package.json` sets `"license": "MPL-2.0"`.
  Per-file license headers are not required.

## 15. Open decisions

1. Final product name (Alstroemeria is the working name).
2. Whether to import Lute's database directly or only its CSV term export (CSV is simpler).
