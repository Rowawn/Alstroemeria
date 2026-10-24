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
parts          id, work_id, idx, title, text, media_hash, timing(json: cue list, see §8), word_count
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

Morphological languages (Japanese, Korean) group morphemes into word tokens and key terms by
lemma: see §16.

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

### 6.9 Extension host design (M3)

This section makes §6.1–6.8 concrete. Where they disagree, this section wins.

#### 6.9.1 Scope of M3
Implemented in M3: manifest loading and validation, registry, capability resolution, install/
uninstall of `.alext`, permissions, the worker runtime (`js` and `wasm`), and these extension
points: **annotator, decorator, panel, command, eventListener**, plus settings and storage.
Deferred to the milestone that first needs them: `tokenizer`/`normalizer` in JS (M5, if ever;
core engines cover planned languages), `lookupProvider` (M5), `importer`/`exporter` (the `importer` contract is fixed in §6.9.10 but not implemented; M8),
`activity` (M9/M11), `native` runtime (M11). The manifest schema accepts their declarations
now; the host ignores them with a logged notice.

#### 6.9.2 Packages, install and loading
- An extension is a folder: `extension.json` plus any files (JS, WASM, HTML, data, `LICENSE`).
  `.alext` = zip of that folder's contents (manifest at the zip root).
- Manifest additions to §6.2: `"license"` (SPDX id, required), `"description"`, `"author"`,
  `"main"` is replaced by `runtime.entry` (path relative to the folder). `targets` is required for
  add-ons and modules (explicit `"*"` for universal ones). Unknown permissions, unsafe paths and unknown
  settings-schema types are validation errors; unknown *fields* are ignored (forward compatibility).
- **Bundled** first-party extensions: `extensions/*` compiled into the app with `include_dir`
  (read-only; can be disabled, not uninstalled; a newer installed version overrides).
- **Installed** extensions: `<app-data>/extensions/<id>/` (one version per id).
- Install (`alst-ext::install`): read zip in memory → validate manifest → reject entries with
  absolute paths, `..`, backslashes, symlinks, or total size > 50 MB (checked against the declared sizes and
  again while reading) → show permission prompt in UI → extract to a temp dir → atomic rename. The UI calls
  `inspect_extension` (validates and returns the manifest summary, writes nothing) before `install_extension`. Uninstall deletes the folder; `extension_state`
  keeps the row with `enabled = 0` so settings survive a reinstall.
- Extension *files* do not sync in M6; `extension_state` does. A device that lacks an enabled
  extension shows it as "missing on this device" (M6 may sync `.alext` as media later).
- Files are served to the webview by a custom protocol `alstext://<id>/<path>`
  (`http://alstext.localhost/<id>/<path>` on Windows/Android), registered in `src-tauri`,
  reading through `alst-ext`. Responses carry `Access-Control-Allow-Origin: *` and correct MIME
  types (`application/wasm` for `.wasm`). HTML files (panels) also carry their own CSP header
  (`default-src 'none'`, scripts/styles/images only from `alstext:`, `connect-src 'none'`, `form-action 'none'`).

#### 6.9.3 Registry and capability resolution (`crates/alst-ext`, pure Rust)
Input: all manifests (bundled + installed), `extension_state` rows, provider choices
(setting `ext.providers.<lang>` = `{ capability: extensionId }`), the host's
`core.*` capability list, and the host API version.
Output, per language: `Resolved { active: [ext ids in run order], annotatorOrder,
decoratorOrder, disabled: [{ id, reason }], fingerprint }`.

Rules, in order:
1. Drop extensions that are disabled, not enabled for this language (`targets`/`languages`),
   not for this platform, or whose `apiVersion` range excludes the host's version.
2. Expand wildcards: `*.x` → `<lang>.x`.
3. For each capability with several providers, keep the user's choice, else the bundled one,
   else the first by id; the others are disabled with reason "another provider is selected".
4. Repeatedly drop extensions with an unmet `requires` (reason names the missing capability)
   until stable. `optional` never disables.
5. Order annotators topologically by provides→requires; a cycle disables every member
   (reason "dependency cycle: a → b → a"). Ties break by id, so order is deterministic.
6. Decorator order: user order (setting `ext.decoratorOrder.<lang>`), then `priority` desc, then id.
7. `fingerprint` = hash of (active ids, versions, settings). Used later by `local_token_cache`.

Clarifications: *Optional* capabilities order the graph when they can (an optional edge that would close a cycle is
ignored). A language extension runs first in `active`. The `native` runtime is disabled with a reason until M11.
`Resolved` also lists `providerChoices` (capabilities with several providers and the chosen one) for the picker.
Extensions that don't apply to the language, are disabled by the user, or are turned off for that language are
simply absent; only platform/API/provider/requirement/cycle problems appear in `disabled`.

Host capabilities in M3: `core.tokenize`, `core.dict`, `core.terms`, `core.events`, `core.storage`,
`core.net`, `core.ui`. Language extensions also implicitly provide `<lang>.tokenize`,
`<lang>.norm` and (if the engine supplies it) `<lang>.lemma`.

#### 6.9.4 Permissions
Declared in the manifest; shown at install/enable; enforced in Rust on every host call
(the UI host tags each call with the worker's extension id; extensions cannot set it).

| Permission | Grants |
|---|---|
| *(always)* | own settings, own `storage`, own events (`<id>/*`), `ui.toast`, the data passed to its hooks |
| `terms:read` | `alst.terms.get/search` |
| `terms:write` | `alst.terms.update` (status, translation, notes, …) — shown as a warning |
| `events:read` | `eventListener` for core events and `alst.events.query` |
| `dict:read` | `alst.dict.lookup` |
| `net:<host>` | `alst.net.fetch` to that host; `net:*.example.org` covers subdomains; HTTPS only |

Limits: `storage` value ≤ 64 KB, ≤ 1,000 keys per extension; `net.fetch` ≤ 10 MB response,
20 s timeout, no cookies, no redirects to non-permitted hosts.

#### 6.9.5 Runtime: workers and the RPC protocol
- One **Web Worker per active extension**, created lazily when a reader for one of its
  languages opens, kept alive while the app runs, terminated on disable/uninstall.
- Workers start from a bootstrap (`ui/src/lib/ext/worker-bootstrap.ts`) that
  installs the `alst` API object, then `import()`s the extension's `runtime.entry` from
  `alstext://`. The bootstrap is bundled inline and started from a `blob:` URL: a worker started from a
  same-origin script URL takes its CSP from that script's response, which for app assets has none, and
  (verified on macOS WebKit) could then reach the network with `fetch(url, {mode: "no-cors"})`; a blob
  worker inherits the page's CSP. `wasm` extensions ship a JS entry that instantiates their `.wasm` (see §6.9.8).
- Messages are JSON-RPC-like: `{ id, method, params }` → `{ id, result }` | `{ id, error: { code, message } }`.
  Host→extension methods: `activate`, `annotate`, `decorate`, `command`, `event`,
  `settingsChanged`, `panelMessage`. A worker serves one extension for all languages; each hook input carries
  `language` and `alst.extension.language` follows the latest call. Core events reach the UI as the Tauri event
  `alst:event` and are forwarded to running workers allowed to see them. Extension→host: the `alst.*` calls in §6.9.7.
- **Timeouts:** `activate` 5 s; `annotate`/`decorate` 2 s per part; `command` 30 s.
  A timeout or thrown error marks the extension *errored* for this run: the reader renders
  without it, a toast names it, and the Extensions screen shows the error. The worker is terminated.
- **CSP** (set in `tauri.conf.json`, replacing `null`): `default-src 'self'`;
  `script-src 'self' 'wasm-unsafe-eval' alstext: http://alstext.localhost`; `worker-src blob:`;
  `connect-src ipc: http://ipc.localhost alstext: http://alstext.localhost`;
  `frame-src alstext: http://alstext.localhost`; `img-src 'self' data: alstext: http://alstext.localhost`;
  `style-src 'self' 'unsafe-inline'`. Workers inherit it, so extensions cannot reach the
  network except via `alst.net.fetch`.
- **Verified in M3** (`ui/src/lib/ext/selftest.ts`, run inside the real webview): a worker cannot call
  Tauri commands directly. It has no `__TAURI_INTERNALS__`/invoke key; the IPC endpoint is reachable by
  `fetch` (it is in `connect-src`) but Tauri answers a request without the per-launch invoke key with an
  error and silently drops one with a wrong key, so no command runs. The key only exists in the main
  window; sandboxed panel iframes don't have it either. The same self-test shows that workers and panels
  cannot reach a loopback canary server over `fetch`, `no-cors` fetch, XHR, WebSocket, `import()`, images
  or form posts. Set `ALST_DATA_DIR` to a scratch folder and create a file named `selftest` in it to run it.
- The sandbox is defence in depth, not a guarantee: extensions are installed deliberately by
  the user, and the install prompt says what they can access.

#### 6.9.6 Reader pipeline with extensions
1. `get_part_view` (Rust) returns `tokens` (the §5.2 `Token[]`, flattened with sentence index)
   in addition to `units`; each `Unit` gains `tokenRange: [first, last]` (inclusive token indices).
2. UI host runs **annotators** in `annotatorOrder`, sequentially. Each receives
   `{ language, tokens }` with the annotations added so far and returns
   `{ [tokenIndex]: { [capability]: value } }`, containing only capabilities it `provides`
   (others are dropped with a dev-console warning). Results are merged into `tokens[i].ann`.
   Annotations are cached per part + fingerprint in memory for the session.
3. UI host runs **decorators** (in parallel) with `{ language, units, tokens }` and gets
   `{ [unitIndex]: RenderHint[] }`. Decorators re-run whenever unit statuses change (after a
   term save); annotators do not.
4. The reader merges hints (§6.9.6.1) and renders.

##### 6.9.6.1 Render hints (M3 set)

```ts
type Role = "noun"|"verb"|"adjective"|"adverb"|"particle"|"function"|"name"
          | "accent-high"|"accent-low"|"accent-drop"|"info"|"warn"|"c1"|"c2"|"c3"|"c4"|"c5"|"c6";
type RenderHint =
  | { kind: "ruby"; text: string }                       // above the unit
  | { kind: "color"; role: Role }                        // text colour
  | { kind: "underline"; role: Role; style?: "solid"|"dotted"|"wavy"|"double" }
  | { kind: "badge"; text: string; role?: Role }         // ≤ 4 chars, superscript
  | { kind: "tooltip"; text: string };                   // plain text
  // reserved, rendered from M5: { kind: "pitchLine"; pattern: ("H"|"L")[]; drop?: number }
```

- **Background is reserved for term status** (the core reader's main signal); decorators can't set it.
- Merge: for `ruby`, `color` and `underline`, the first decorator in `decoratorOrder` wins;
  `badge` and `tooltip` accumulate (badges max 2 shown, tooltips joined by line breaks).
- Roles map to CSS variables `--role-<name>` defined in light and dark themes in `app.css`.
- Unknown hint kinds are ignored (forward compatibility).

#### 6.9.7 Host API v1.0 (what M3 implements)
Defined once as TypeScript in `sdk/alstroemeria.d.ts` (MIT); this list is a summary.

```
alst.extension            { id, version, language }          // language of the current activation
alst.settings.get(): Promise<object>
alst.storage.get(key) / .set(key, value) / .delete(key) / .keys()
alst.terms.get({ ids? , texts?, language }) / .search({ language, query, status? })   // terms:read
alst.terms.update(id, { status?, translation?, notes?, reading? })                    // terms:write
alst.events.emit(type, payload)                       // type is prefixed with the extension id
alst.events.query({ type?, since?, limit? })          // events:read
alst.dict.lookup(language, query)                     // dict:read
alst.net.fetch(url, { method?, headers?, body? }) → { status, headers, text }   // net:<host>
alst.ui.toast(message, { kind?: "info"|"error" })
alst.ui.openPanel(panelId)
alst.ui.openTerm(language, text)                      // opens the term editor in the reader
alst.ui.copyText(text)                                // clipboard, via the host
alst.reader.current() → { workId, partId, language } | null
```

Extension module shape (the `runtime.entry` exports):

```ts
export function activate(ctx): void | Promise<void>;
export function annotate?(input: { language: string; tokens: Token[] }): Promise<Record<number, Record<string, unknown>>>;
export function decorate?(input: { language: string; units: Unit[]; tokens: Token[] }): Promise<Record<number, RenderHint[]>>;
export const commands?: Record<string, (args: unknown) => unknown>;
export function onEvent?(event: Event): void;
export function onSettingsChanged?(settings: object): void;
export function onPanelMessage?(panelId: string, msg: unknown): unknown;
```

**Decorators:** the `decorate` hook is only called for extensions with a `contributes.decorators` entry (it may
return no hints, and then serves as a "the reader's statuses changed" hook; `addon.density` does this).

**Panels:** `contributes.panels: [{ id, title, entry: "panel.html", placement: "sidebar" }]`.
Rendered in `<iframe sandbox="allow-scripts">` from `alstext://`. The panel uses
`sdk/panel.js` (MIT) to `postMessage` to the host, which relays to the extension's worker
(`onPanelMessage`) or to permitted `alst.*` calls. On load the panel
posts `{type:"alst-ready"}` and the host answers `{type:"alst-theme", cssVariables}` so panels match light/dark
mode. Panel→host calls are `{type:"alst-call", id, method, params}` (`panel.message` goes to `onPanelMessage`,
anything else is a permitted `alst.*` call tagged with the panel's extension) answered by `{type:"alst-result", id,
result|error}`; the host also sends `{type:"alst-event", name:"refresh"}` when the reader's data changed. The host
identifies the iframe by `event.source` (its origin is opaque).

**Commands:** `contributes.commands: [{ id, title, context?: "reader"|"term"|"global" }]`;
shown in a command palette (Ctrl/Cmd+K) and, for `term`, in the term editor's menu.

**Settings:** `contributes.settings` is a JSON Schema object whose properties may be
`boolean`, `number`, `string` or `string` with `enum`. The Extensions screen renders a form
from it; values are stored in `extension_state.settings`.

#### 6.9.8 Writing extensions in Rust (WASM ABI v1)
To avoid toolchain dependencies (no `wasm-bindgen`), WASM modules use a tiny JSON-in-memory ABI:

- Exports: `memory`, `alst_alloc(len: u32) -> u32`, `alst_free(ptr: u32, len: u32)`, and for each
  hook `alst_<hook>(ptr: u32, len: u32) -> u64` where the input is UTF-8 JSON and the return
  packs `(out_ptr << 32) | out_len` of a UTF-8 JSON result the host frees.
- Hooks available: `annotate`, `decorate`, `command` (same JSON shapes as §6.9.7; `command` input is `{id, args}`).
  A packed result of `0` means "nothing". The host frees the input and the result with `alst_free`. There are no
  imports, so a module needs no JS glue beyond the shim; a trap (panic) marks the extension errored.
- Host API calls from WASM are not available in ABI v1; the JS shim can call `alst.*` and pass results in.
- `sdk/wasm-shim.js` (MIT) implements the JS side generically. It is vendored into the
  extension folder (the build script copies it for first-party extensions), and the entry is:
  `import { createWasmExtension } from "./wasm-shim.js";`
  `export default createWasmExtension(new URL("./ext.wasm", import.meta.url));`
- An entry may use named exports or a default-exported object with the same members (§6.9.7).
- Build: `cargo build --target wasm32-unknown-unknown --release`; a script
  (`npm run build:extensions`) copies the `.wasm` into the extension folder. Built `.wasm`
  files are git-ignored and produced before `tauri dev/build`.

#### 6.9.9 First-party examples shipped in M3 (MIT)
- **`es.morph-hints`** (Rust → WASM; annotator + decorator; targets `es`; no permissions):
  heuristically tags infinitives (-ar/-er/-ir), gerunds (-ando/-iendo), participles (-ado/-ido)
  and -mente adverbs; provides `es.morph`; decorates with `underline` role `verb`/`adverb`
  and a tooltip ("gerund"). Settings: which categories to show. Proves the WASM path.
- **`addon.density`** (JS; panel + command + eventListener; targets `*`; `events:read`, `terms:read`):
  sidebar panel showing the current part's unknown-word % and its 10 most frequent unknown
  words (click → opens the term editor); remembers per-language history from `part.read`
  events in `storage`; command "Copy unknown words" to the clipboard. Proves language-universal add-ons.

#### 6.9.10 Importers: extension point and the M4 decision

**Contract (reserved, not implemented).** A manifest may declare `contributes.importers: [{ id, title, accepts }]`, with `accepts`
one of `"url"`, `"text"` or `"file:<ext>[,<ext>]"`. The extension's `import(input)` command returns an **import bundle**
(`{ title, kind, author?, sourceUrl?, parts: [{ title?, text, timing? }], media?: [{ url | bytes, name }] }`), which the host
validates and writes through `alst-core::store` and the media store. An extension never writes the store, the media directory or
the file system itself, and its network use stays limited by its `net:` permissions.

**Decision for M4: the podcast RSS and YouTube importers are core features, not extensions.** Reasons:
1. YouTube needs to run a user-installed executable (`yt-dlp`). The permission model (§6.9.4) has no process-spawn permission and a
   worker can't spawn processes; adding one would be an unsandboxed escape hatch, which is what the `native` runtime (M11) is for.
2. Podcast import downloads large binaries (audio) into the media store; host API v1 has no streaming media write, and moving
   hundreds of MB through worker RPC is wasteful. Workers also have no XML parser (`DOMParser`), while the core already has one.
3. Both are small, first-party, and need to work on iOS later (RSS) without a native runtime.
They produce the same import-bundle data as the contract above (`alst-text::import::{rss, ytdlp, media}` return plain data; the
shell stores it), so promoting them into extensions later, or exposing the contract for third-party importers (M8 packs), changes
who produces the bundle, not what is stored. Until then the `importers` key is accepted in manifests and ignored with a notice.

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
| EPUB | 2 | Chapters become parts; images go to media (images still skipped) |
| Web article (URL) | 2 | Fetched by core; readability extraction |
| Subtitles SRT/VTT, lyrics LRC | 4 | Cues become `parts.timing`; the reader highlights and seeks |
| Audio/video file + subtitles | 4 | File goes to media store; cue-synced reader |
| Podcast RSS | 4 | Core importer (§6.9.10); needs a transcript in the feed, else (M11) optional Whisper |
| YouTube subtitles | 4 | Core importer, desktop only, via a `yt-dlp` binary the user installs; subtitles only |

### 8.1 Media store

Media files are content-addressed (`sha256`) and stored under `<app data>/media/ab/abcdef…` (no extension), so they are
deduplicated and sync-friendly. The synced `media` row (`id` = hash, `mime`, `size`, `original_name`) is written by
`alst-core::store`; `alst-core::media` only handles files. Parts reference a file by `parts.media_hash`; several parts
(the chunks of one long file) may share one.

- **Upload** is chunked (`media_upload_begin` / `media_upload_chunk` of 1 MiB / the finishing import command), into
  `media/tmp/` and hashed at the end, so memory stays bounded and Android's JSON-encoded binary bodies stay small per call.
  Half-finished uploads are deleted at startup. Downloads (podcast audio) stream into the same store, max 2 GiB per file.
- **Playback** uses the `alstmedia://` protocol (`http://alstmedia.localhost/<hash>` on Windows/Android) with `Range`
  support, and `media-src` in the CSP. A request is answered with at most 8 MiB, so a plain GET of a big file is still
  bounded. The UI builds URLs with `convertFileSrc(hash, "alstmedia")`.
- A media row whose file is absent on this device (future sync, §9) is reported to the reader as `present: false`.
- Accepted types: whatever the webview can play. The import accepts common audio/video extensions (mp3, m4a, aac, ogg, opus,
  wav, flac, mp4, m4v, webm, mov, mkv) and shows an error in the player when the platform can't decode the file.

### 8.2 `parts.timing`

```json
{ "v": 1, "cues": [ { "start": 1000, "end": 3500, "a": 0, "b": 15 } ] }
```

`start`/`end` are milliseconds from the start of the media file (absolute, so parts of a long file share one file).
`a`/`b` are the cue's range in the part text as **UTF-16 code units**, the same offsets as tokens and reader units.
The part text is the cues' text joined by `\n`; each cue is flattened to one line (markup, `{\an8}`, entities removed).
A parser (`alst-text::cues`) handles SRT, WebVTT (incl. `NOTE`/`STYLE`, settings, inline tags; optional removal of the repeated
lines of rolling auto-captions) and LRC (`ti`/`ar`/`offset` tags, several stamps per line, enhanced word tags dropped; a cue ends
where the next starts). Files are decoded as UTF-8/UTF-16 (BOM), else windows-1252. Files are split into parts of about 5000
characters at cue boundaries; part titles show the time range. A part without `timing` but with `media_hash` (a transcript with no
times) shows the player and plain text.

### 8.3 Cue-synced reader

When a part has timing, the reader shows a player (`<audio>` or `<video>`) and one row per cue. The current cue (the last one
that has started) is highlighted; a cue row's play button, or a click on its blank area, seeks to the cue and plays; clicking a
word still opens the term editor. Previous/next cue, playback speed, and **repeat sentence** (a cue is the unit; it loops until
turned off) are provided. Leaving or finishing a part logs `media.listen` (§4.3) with the seconds actually played.

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
- **First-party extensions** (`extensions/*`) and the **extension SDK** (`sdk/*`): **MIT**, so they can be freely copied as templates.
  Each extension folder carries its own `LICENSE` and sets `"license"` in its manifest.
- Dependencies must be MPL-compatible (MIT, Apache-2.0, BSD, ISC, Zlib, MPL, Unicode are fine).
  **No GPL/AGPL/LGPL code** in the core, which rules out copying Yomitan code (§7).
- Cargo crates set `license = "MPL-2.0"`; `package.json` sets `"license": "MPL-2.0"`.
  Per-file license headers are not required.

## 15. Open decisions

1. Final product name (Alstroemeria is the working name).
2. Whether to import Lute's database directly or only its CSV term export (CSV is simpler).

## 16. Japanese and Korean (M5)

This section adds morphological languages. It extends §5 (tokens), §6.9 (extension points) and §7 (lookup).

### 16.1 Engine and dictionaries
- Core engine `lindera` in `alst-text` (pure Rust; builds for every target incl. Android/iOS).
  Japanese uses **IPADIC** (smaller than UniDic, gives base form + katakana reading;
  permissive licence). Korean uses **ko-dic** (Apache-2.0). Attribution goes in About.
- Dictionaries are **embedded at compile time behind cargo features** `lang-ja` and `lang-ko`
  (on by default). This is the simplest path that works offline on every platform.
  Record the installer/APK size increase in the roadmap; if the installer exceeds ~60 MB,
  M7 moves dictionaries to downloadable "language data" (pinned URL + sha256, or import from file).
- The build session confirms the current Lindera API and dictionary crates and pins versions.

### 16.2 Word grouping (what a reader "word" is)
Morphological analysers split text finer than a learner thinks (`食べ|させ|られ|た`).
The engine therefore **groups morphemes into word tokens**. A grouped token is one `Token`:
`text` = joined surfaces, `lemma` = dictionary form, `ann["<lang>.morphemes"]` =
`[{ surface, base, pos, reading? }]`, `ann["<lang>.pos"]` = the head's universal role (§16.4),
`ann["ja.reading"]` = joined reading in hiragana (Japanese only).

**Japanese (IPADIC POS):**
- A group starts at a head: `動詞-自立`, `形容詞-自立`, or `名詞-サ変接続` immediately followed by
  a form of する (lemma = noun + する).
- It absorbs following morphemes while they are: `助動詞`; `動詞-接尾`; `動詞-非自立`
  (いる, しまう, おく, いく, くる, ある, みる after て/で); `助詞-接続助詞` て/で/ば/たり;
  `名詞-接尾-助動詞語幹` (そう); `形容詞-非自立`/`形容詞-接尾`.
- Everything else (particles, nouns, symbols) is its own token. Max 8 morphemes per group.
- Readings: katakana → hiragana. Words without a dictionary reading get none.

**Korean (ko-dic / Sejong POS):** groups never cross whitespace (eojeol boundary).
- Heads: `VV`, `VA`, `VX`, `VCP`, `VCN`, `XR`, and `NNG`/`XR` followed by `XSV`/`XSA` (lemma = root + 하다 etc.).
- Absorbs: `EP`, `EF`, `EC`, `ETN`, `ETM`, `XSV`, `XSA`, and noun + `XSN` (들; lemma = the noun).
- Particles (`J*`) are separate tokens, so `학교에` → `학교` + `에`, and `학생이에요` → `학생` + `이에요` (lemma 이다).
- Contracted morphemes (e.g. `했` = `하/VV+았/EP`) are expanded using ko-dic's expression field;
  verbal lemma = stem + 다 (`먹었습니다`→먹다, `공부했어요`→공부하다, `예뻐요`→예쁘다, `갔다`→가다).
  Genuinely ambiguous stems (들었다: 듣다/들다) take the analyser's choice.

### 16.3 Term keys
Language manifests gain `"termKey": "surface" | "lemma"` (default `surface`). `ja` and `ko` use
`lemma`: the unit's key is the normalized lemma, so 食べた, 食べない and 食べさせられた all show
and update the status of the term 食べる. The term editor shows the lemma as the term and the
surface as "seen as". Multi-word matching (§5.3) runs over the same keys. `es` is unchanged.

### 16.4 Universal POS roles
Languages that provide `<lang>.pos` map their tags onto the render roles:
`noun`, `name` (proper nouns), `verb`, `adjective` (incl. ja 形容動詞語幹 and ko VA),
`adverb`, `particle` (ja 助詞, ko J*), `function` (auxiliaries, conjunctions, endings, symbols).

### 16.5 Deinflection (lookup scanning)
Used by the lookup popup to find dictionary forms of any span the user clicks or selects, including
spans the tokenizer grouped differently. Written from scratch; **no Yomitan code or rule files**
(GPL-3.0), only our own grammar tables.

- **Engine:** `alst-text::deinflect`, generic and data-driven. A language provides
  `deinflect.json` (path in its manifest: `"deinflection": "deinflect.json"`).
- **Rule format:** each rule is
  `{ "suffixIn": "かった", "suffixOut": "い", "condIn": ["past"], "condOut": ["adj-i"], "reason": "past" }`
  read as: a word ending in `suffixIn` whose current conditions intersect `condIn` can be rewritten
  to end in `suffixOut`, now with conditions `condOut`, adding `reason`.
  Condition names: `v1` (ichidan), `v5` (godan, any row), `vk` (来る), `vs` (する), `adj-i`,
  plus intermediate forms (`past`, `te`, `masu`, `neg`, `tai`, …) for chaining. The surface starts
  with all conditions allowed.
- **Algorithm:** breadth-first from the surface; apply every matching rule; depth ≤ 10;
  dedupe on (text, conditions). Output: candidates `{ text, conditions, reasons[] }`.
- **Validation against dictionaries:** a candidate counts only if a dictionary entry with that
  headword or reading exists and its Yomitan `rules` field (`v1`, `v5`, `vs`, `vk`, `adj-i`, …)
  intersects the candidate's final conditions (candidates with no rules applied always count).
  This needs a new `rules` column in `local_dict_entries` (new migration; existing dictionaries
  are re-imported on next launch or show "re-import to enable conjugation lookup").
- **Japanese rule coverage (required, each with a test):**

| Surface | Lemma | Reasons |
|---|---|---|
| 食べた / 食べない / 食べなかった | 食べる | past / negative / negative, past |
| 食べて / 食べている / 食べてる | 食べる | te / te, progressive / te, progressive |
| 食べられる / 食べさせる / 食べさせられた | 食べる | passive-potential / causative / causative, passive, past |
| 食べます / 食べませんでした | 食べる | polite / polite, negative, past |
| 食べたい / 食べたくない | 食べる | desire / desire, negative |
| 食べよう / 食べれば / 食べたら / 食べろ | 食べる | volitional / ba-conditional / tara-conditional / imperative |
| 書いた / 書かない / 書ける / 書かれる | 書く | past / negative / potential / passive |
| 泳いだ, 話した, 待った, 買った, 帰った, 死んだ, 遊んだ, 読んだ | 泳ぐ, 話す, 待つ, 買う, 帰る, 死ぬ, 遊ぶ, 読む | past |
| 行った / 行って | 行く | past / te (irregular) |
| した / しない / させる / される | する | past / negative / causative / passive |
| 勉強した | 勉強する (and 勉強 if tagged `vs`) | past |
| 来た / 来ない / きた / こない | 来る / くる | past / negative |
| 高かった / 高くない / 高くて / 高ければ / 高そう / 高さ | 高い | past / negative / te / ba-conditional / appearance / noun (-sa) |
| よかった | 良い / いい | past (irregular) |
| 読まされていた | 読む | causative (short), passive, te, progressive, past |
| 食べなきゃ | 食べる | negative, conditional (colloquial) |

  Plus negatives: 帰る must not deinflect to 帰 + る as ichidan when only the godan entry exists
  (dictionary validation handles it); 見る must not produce 見 alone.
- Korean needs no rule file in M5: lookups use the tokenizer's lemma.

### 16.6 Lookup flow
New core command `lookup_scan(language, text, maxLen = 20)` (the text from the clicked position to
the end of the sentence):
for each prefix from longest to shortest → candidates = prefix, its kana-normalized form, and its
deinflections → query entries by headword **or** reading → validate (§16.5). Stop after the longest
prefix with results plus up to 2 shorter ones. Results carry `{ matched, lemma, reading, reasons[], entries }`,
sorted by match length, then frequency (§16.7), then dictionary order.
Languages without a deinflection file keep the M2 behaviour (lookup by key and lemma).
Queries normalize katakana ↔ hiragana for the reading index.

### 16.7 Dictionary meta banks
Import Yomitan `term_meta_bank_*` into a new local table
`local_dict_meta(dict_id, term, reading, mode, data json)` with modes `freq` and `pitch`
(pitch data: `{ reading, pitches: [{ position, nasal?, devoice? }] }`). Kanji banks stay ignored.
Host API addition (permission `dict:read`): `alst.dict.meta(language, { term, reading?, mode })`.
Pitch data is user-imported (e.g. the Kanjium pitch dictionary, CC BY-SA 4.0); nothing is bundled.

### 16.8 Extension point: lookupProvider (implemented in M5)
`contributes.lookupProviders: [{ id, title }]`; the module exports
`lookup(input) → LookupSection[]` with a 2 s timeout, called when the term editor opens.

```ts
input: { language, unit: { text, key, lemma?, status }, tokens: Token[], sentence: string }
type LookupSection = { title: string; items: LookupItem[] };
type LookupItem =
  | { kind: "text"; text: string; role?: Role }
  | { kind: "kv"; label: string; text: string }
  | { kind: "pitch"; reading: string; downstep: number }   // drawn by the core as a pitch graph
  | { kind: "link"; label: string; url: string };          // https only, opens the system browser
```
Plain text only (no HTML). Sections render under the dictionary results in decorator order.

### 16.9 Render-hint additions
- `ruby` gains a segmented form: `{ kind: "ruby", segments: [{ base, rt? }] }`. The bases must
  concatenate to the unit text, otherwise the hint is ignored. This lets furigana sit over kanji only.
- New roles: `accent-heiban`, `accent-atamadaka`, `accent-nakadaka`, `accent-odaka` (light + dark).
- `pitchLine` stays reserved (pitch is shown by colour, badge and the popup graph).
- Reader CSS for CJK: no inter-word spaces are added; `line-break: strict`.

### 16.10 First-party extensions in M5 (MIT)
| Extension | Kind / runtime | Requires → provides | What it does |
|---|---|---|---|
| `lang-ja` | language, declarative | → `ja.tokenize`, `ja.lemma`, `ja.reading`, `ja.pos`, `ja.morphemes` | Lindera IPADIC, `termKey: lemma`, `deinflect.json`, sentence ends `。！？`, lookup URL (Jisho) |
| `lang-ko` | language, declarative | → `ko.tokenize`, `ko.lemma`, `ko.pos`, `ko.morphemes` | Lindera ko-dic, `termKey: lemma`, lookup URLs (Naver, Wiktionary) |
| `ja-furigana` | addon, JS decorator | `ja.reading` | Segmented ruby over kanji (okurigana aligned). Setting: show for all / unknown (status 0–1) / learning (0–3) / none; default unknown |
| `ja-pitch` | addon, JS annotator + decorator + lookupProvider; `dict:read` | `ja.reading` → `ja.pitch` | Pitch from imported meta banks; colours the word by pattern (heiban/atamadaka/nakadaka/odaka) or shows the downstep as a badge (setting); popup pitch graph; tells the user to import a pitch dictionary if none |
| `ko-hanja` | addon, JS lookupProvider + decorator | `ko.lemma` → `ko.hanja` | Hanja for Sino-Korean words, with per-character meanings in the popup; optional hanja ruby (off by default). Data: a TSV `word\thanja\tmeaning`. Bundle a dataset only if its licence is MIT/BSD/Apache/CC0/CC-BY (check libhangul's `hanja.txt` licence); otherwise ship without data and let the user import a TSV |
| `addon-grammar-colors` | addon, JS decorator; targets `*` | `*.pos` | Colours words by role; settings choose which roles. Disabled for `es` with "missing es.pos" — the capability system working as intended |

Mora counting for pitch: each kana is one mora except small ゃゅょぁぃぅぇぉ (attach to the previous);
ー, っ and ん count. Pattern: downstep 0 → heiban; 1 → atamadaka; = mora count → odaka; else nakadaka.
