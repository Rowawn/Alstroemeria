# Alstroemeria extension SDK

MIT licensed (see `LICENSE`). The design is in `docs/SPEC.md` §6; this is the practical guide.

An extension is a folder:

```
my-extension/
  extension.json      manifest (JSON Schema: schemas/extension.schema.json)
  main.js             runtime.entry — runs in a Web Worker
  panel.html          optional sidebar panels
  LICENSE
```

Zip the folder's **contents** (`extension.json` at the root of the zip, not inside a folder) and rename it
`.alext`; install it from the app's Extensions screen. The screen shows its permissions first.

## Manifest

```json
{
  "id": "my.extension",
  "name": "My extension",
  "version": "1.0.0",
  "kind": "addon",
  "license": "MIT",
  "apiVersion": "^1.0",
  "targets": ["es"],
  "provides": ["es.mything"],
  "requires": ["es.tokenize"],
  "runtime": { "type": "js", "entry": "main.js" },
  "permissions": ["terms:read"],
  "contributes": {
    "decorators": [{ "id": "mine", "priority": 50 }],
    "panels": [{ "id": "p", "title": "My panel", "entry": "panel.html" }],
    "commands": [{ "id": "go", "title": "Do the thing", "context": "reader" }],
    "settings": { "type": "object", "properties": { "loud": { "type": "boolean", "default": false } } }
  }
}
```

- `id`: lowercase letters, digits, `.`, `-`, `_`. `targets`: language ids or `"*"`.
- **Capabilities** (`provides`/`requires`/`optional`, written `<scope>.<name>`): add-ons depend on capabilities, not on
  other extensions. `*.x` in `requires` means "`x` for whatever language is active". An extension whose requirements
  aren't met is disabled, and the Extensions screen says why. If two extensions provide the same capability the user
  picks one.
- **Permissions** are enforced by the app on every call: `terms:read`, `terms:write`, `events:read`, `dict:read`,
  `net:<host>` (HTTPS only; `net:*.example.org` covers subdomains). Everything else (your own settings and storage,
  toasts, the data passed to your hooks) is always allowed.
- **Settings**: a JSON Schema whose properties are `boolean`, `number`, `string` or `string` with `enum`; the
  Extensions screen renders the form.
- `contributes.lookupProviders`, `importers`, `exporters`, `activities` and the `native` runtime are accepted in the
  manifest but ignored by this version.

## JavaScript

`alstroemeria.d.ts` has every type. The entry exports any of these (named exports, or `export default {…}`):

```js
export function activate({ alst }) {}                       // once per worker start, 5 s
export function annotate({ language, tokens }) {            // per part, 2 s
  return { 3: { "es.mything": "value" } };                  // { tokenIndex: { capability: value } }
}
export function decorate({ language, units, tokens }) {     // per part, and again when term statuses change, 2 s
  return { 3: [{ kind: "underline", role: "verb", style: "dotted" }, { kind: "tooltip", text: "hi" }] };
}
export const commands = { go: (args) => alst.ui.toast("done") };   // 30 s
export function onEvent(event) {}                           // core events with events:read, and your own
export function onPanelMessage(panelId, msg) { return {}; } // from panel.js's alst.send(msg)
```

A hook that throws or exceeds its time limit stops the extension for the rest of the run: the reader carries on
without it and the Extensions screen shows the error (with a Retry button). Workers have no DOM and no network; call
`alst.net.fetch` for the latter. Render hints are a small closed vocabulary (`ruby`, `color`, `underline`, `badge`,
`tooltip`) with colours given as semantic roles (`verb`, `noun`, `accent-high`, …) so themes and dark mode stay
consistent. Background is reserved for term status.

### Panels

`panel.html` runs in a sandboxed iframe (no network, no access to the app). Include `panel.js` from this SDK next to it:

```html
<script type="module">
  import { alst } from "./panel.js";
  const stats = await alst.send({ type: "stats" });     // → your worker's onPanelMessage
  const terms = await alst.call("terms.get", { language: "es", texts: ["hola"] });   // permitted alst.* calls
  alst.on("refresh", () => {});                         // the reader's data changed
  alst.ui.openTerm("es", "hola");
</script>
```

The page gets the app's colours as CSS variables (`--bg`, `--fg`, `--accent`, `--role-verb`, …), so it follows light and
dark mode. `extensions/addon-density` is a complete example (JS, panel, command, event listener).

## Rust (WebAssembly)

Rust compiles to WebAssembly and runs in the same worker, with no toolchain beyond `rustup target add
wasm32-unknown-unknown`. Modules use a tiny JSON-in-memory ABI (SPEC §6.9.8): export `alst_alloc(len) -> u32`,
`alst_free(ptr, len)` and any of `alst_annotate`, `alst_decorate`, `alst_command` taking `(ptr, len)` and returning
`(out_ptr << 32) | out_len` (UTF-8 JSON both ways). `extensions/es-morph-hints` is a complete example: copy its
`Cargo.toml` (crate type `cdylib`) and `src/lib.rs`.

```js
// main.js
import { createWasmExtension } from "./wasm-shim.js";     // copy sdk/wasm-shim.js next to it
export default createWasmExtension(new URL("./ext.wasm", import.meta.url));
```

with `"runtime": { "type": "wasm", "entry": "main.js" }`. The ABI has no host calls; wrap the shim in JS and pass what
the module needs (such as `await alst.settings.get()`) into the input, as `es-morph-hints/main.js` does.

```
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/your_crate.wasm ext.wasm
```

For first-party extensions in this repository, `npm run build:extensions` does the build and copies `wasm-shim.js` and
`panel.js` in.

## Testing

The pure logic of a hook is plain JS or Rust: unit-test it directly (see `ui/src/lib/ext/examples.test.ts`, which runs
the compiled WebAssembly through the shim in Node). `alst` is a global in the worker; stub it with a fake in tests.
