// Runs inside each extension's dedicated Web Worker (SPEC §6.9.5). Installs the `alst` API, imports the
// extension's entry from `alstext://`, and dispatches host→extension calls to its exports.
// The worker has no DOM and no Tauri API; everything goes through RPC to the UI host.

import { RpcChannel, RpcFailure } from "./rpc";

type Hooks = {
  activate?: (ctx: unknown) => unknown;
  annotate?: (input: unknown) => unknown;
  decorate?: (input: unknown) => unknown;
  lookup?: (input: unknown) => unknown;
  commands?: Record<string, (args: unknown) => unknown>;
  onEvent?: (event: unknown) => unknown;
  onSettingsChanged?: (settings: unknown) => unknown;
  onPanelMessage?: (panelId: string, msg: unknown) => unknown;
};

const scope = self as unknown as {
  postMessage(msg: unknown): void;
  onmessage: ((e: { data: unknown }) => void) | null;
  alst?: unknown;
};

let hooks: Hooks = {};
const extension = { id: "", version: "", language: "" };

// Extensions call the host through this channel; the host tags every call with this worker's extension id.
const rpc = new RpcChannel(scope, (method, params) => dispatch(method, params));
const call = (method: string, params?: unknown) => rpc.call(`alst.${method}`, params);

const alst = {
  extension,
  settings: { get: () => call("settings.get") },
  storage: {
    get: (key: string) => call("storage.get", { key }),
    set: (key: string, value: unknown) => call("storage.set", { key, value }),
    delete: (key: string) => call("storage.delete", { key }),
    keys: () => call("storage.keys"),
  },
  terms: {
    get: (q: unknown) => call("terms.get", q),
    search: (q: unknown) => call("terms.search", q),
    update: (id: string, patch: unknown) => call("terms.update", { id, patch }),
  },
  events: {
    emit: (type: string, payload?: unknown) => call("events.emit", { type, payload }),
    query: (q?: unknown) => call("events.query", q ?? {}),
  },
  dict: {
    lookup: (language: string, query: string) => call("dict.lookup", { language, query }),
    meta: (language: string, q: { term?: string; terms?: string[]; reading?: string; mode: "freq" | "pitch" }) => call("dict.meta", { language, ...q }),
  },
  net: { fetch: (url: string, init?: Record<string, unknown>) => call("net.fetch", { url, ...init }) },
  ui: {
    toast: (message: string, opts?: { kind?: "info" | "error" }) => call("ui.toast", { message, kind: opts?.kind }),
    openPanel: (panelId: string) => call("ui.openPanel", { panelId }),
    openTerm: (language: string, text: string) => call("ui.openTerm", { language, text }),
    copyText: (text: string) => call("ui.copyText", { text }),
  },
  reader: { current: () => call("reader.current") },
};
scope.alst = alst;

/** Normalize `export default { ... }` and named exports into one shape (named exports win over default's). */
function hooksOf(mod: Record<string, unknown>): Hooks {
  const def = mod.default && typeof mod.default === "object" ? (mod.default as Record<string, unknown>) : {};
  return { ...def, ...mod } as Hooks;
}

async function dispatch(method: string, params: unknown): Promise<unknown> {
  const p = (params ?? {}) as Record<string, unknown>;
  if (typeof p.language === "string") extension.language = p.language;
  switch (method) {
    case "activate": {
      const a = p as { entry: string; extension: { id: string; version: string }; language: string };
      extension.id = a.extension.id;
      extension.version = a.extension.version;
      extension.language = a.language;
      hooks = hooksOf(await import(/* @vite-ignore */ a.entry));
      await hooks.activate?.({ alst, extension });
      return null;
    }
    case "annotate":
      return (await hooks.annotate?.(p)) ?? {};
    case "decorate":
      return (await hooks.decorate?.(p)) ?? {};
    case "lookup":
      return (await hooks.lookup?.(p)) ?? [];
    case "command": {
      const fn = hooks.commands?.[p.id as string];
      if (!fn) throw new RpcFailure("no_such_command", `no command '${String(p.id)}'`);
      return (await fn(p.args)) ?? null;
    }
    case "event":
      await hooks.onEvent?.(p.event);
      return null;
    case "settingsChanged":
      await hooks.onSettingsChanged?.(p.settings);
      return null;
    case "panelMessage":
      return (await hooks.onPanelMessage?.(p.panelId as string, p.msg)) ?? null;
    default:
      throw new RpcFailure("no_such_method", `unknown method ${method}`);
  }
}
