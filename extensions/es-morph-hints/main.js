// Entry for the WASM extension. The WASM ABI has no host calls (SPEC §6.9.8), so this wrapper reads the
// settings through `alst` and passes them in with the decorate input.
import { createWasmExtension } from "./wasm-shim.js";

const wasm = createWasmExtension(new URL("./ext.wasm", import.meta.url));

export default {
  ...wasm,
  async decorate(input) {
    const settings = await alst.settings.get();
    return wasm.decorate({ ...input, settings });
  },
};
