// WASM extension shim (MIT), ABI v1 (SPEC §6.9.8). Copied into the extension folder by `npm run build:extensions`.
//
//   import { createWasmExtension } from "./wasm-shim.js";
//   export default createWasmExtension(new URL("./ext.wasm", import.meta.url));
//
// The module must export: memory, alst_alloc(len) -> ptr, alst_free(ptr, len), and any of
// alst_annotate / alst_decorate / alst_command (ptr, len) -> u64, where input and output are UTF-8 JSON and the
// result packs (out_ptr << 32) | out_len. A result of 0 means "nothing". The host frees the result with alst_free.

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function createWasmExtension(wasmUrl) {
  let exports = null;

  async function load() {
    if (exports) return exports;
    const res = await fetch(wasmUrl);
    if (!res.ok) throw new Error(`cannot load ${wasmUrl}: HTTP ${res.status}`);
    let instance;
    try {
      ({ instance } = await WebAssembly.instantiateStreaming(res.clone(), {}));
    } catch {
      // Streaming needs the right MIME type; fall back to bytes.
      ({ instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {}));
    }
    exports = instance.exports;
    return exports;
  }

  function run(hook, input) {
    const fn = exports[`alst_${hook}`];
    if (typeof fn !== "function") return undefined;
    const bytes = encoder.encode(JSON.stringify(input));
    const ptr = exports.alst_alloc(bytes.length);
    new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes); // re-read memory.buffer: alloc may grow it
    let packed;
    try {
      packed = BigInt.asUintN(64, fn(ptr, bytes.length));
    } finally {
      exports.alst_free(ptr, bytes.length);
    }
    if (packed === 0n) return undefined;
    const outPtr = Number(packed >> 32n);
    const outLen = Number(packed & 0xffffffffn);
    const text = decoder.decode(new Uint8Array(exports.memory.buffer, outPtr, outLen));
    exports.alst_free(outPtr, outLen);
    return JSON.parse(text);
  }

  return {
    async activate() {
      await load();
    },
    annotate: (input) => run("annotate", input),
    decorate: (input) => run("decorate", input),
    commands: new Proxy({}, { get: (_, id) => (args) => run("command", { id, args }) }),
  };
}
