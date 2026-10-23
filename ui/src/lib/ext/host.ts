// The UI-side extension host: owns one worker per active extension, routes `alst.*` calls to Rust with the
// worker's extension id, runs annotators and decorators, and tracks errors (SPEC §6.9.5, §6.9.6).

import { mergeAnnotations, mergeHints, type DecoratorOutput, type MergedHints } from "./hints";
import { RpcChannel, RpcFailure, type Port } from "./rpc";
import type { CoreEvent, ExtInfo, ResolvedView, Token, UnitLike } from "./types";

export const TIMEOUTS = { activate: 5000, annotate: 2000, decorate: 2000, command: 30000, event: 5000, panel: 10000 } as const;

export type ExtStatus = "idle" | "starting" | "ready" | "errored";
export type ExtError = { message: string; at: number };

/** What the host needs from the outside world; replaced by fakes in tests. */
export type HostDeps = {
  /** Creates the worker for an extension. */
  createWorker(extId: string): Port & { terminate(): void };
  /** `alstext://` URL (or the Windows/Android http form) of a file in an extension. */
  entryUrl(extId: string, path: string): string;
  /** Rust host-API call. */
  extCall(extId: string, method: string, params: unknown): Promise<unknown>;
  ui: UiBridge;
  now(): number;
};

export type UiBridge = {
  toast(message: string, kind?: "info" | "error"): void;
  openPanel(extId: string, panelId: string): void;
  openTerm(language: string, text: string): void;
  copyText(text: string): Promise<void> | void;
  readerCurrent(): { workId: string; partId: string; language: string } | null;
};

type Running = { info: ExtInfo; worker: Port & { terminate(): void }; rpc: RpcChannel; ready: Promise<void> };

export class ExtensionHost {
  private running = new Map<string, Running>();
  private errors = new Map<string, ExtError>();
  private listeners = new Set<() => void>();
  private annotated = new Map<string, Token[]>();

  constructor(private deps: HostDeps) {}

  /** Subscribe to status changes (errors, starts, terminations). Returns an unsubscribe function. */
  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private changed() {
    this.listeners.forEach((fn) => fn());
  }

  status(id: string): ExtStatus {
    if (this.errors.has(id)) return "errored";
    return this.running.has(id) ? "ready" : "idle";
  }

  error(id: string): ExtError | undefined {
    return this.errors.get(id);
  }

  /** Forget an extension's error so it can start again (the Extensions screen's "Retry"). */
  clearError(id: string) {
    if (this.errors.delete(id)) this.changed();
  }

  /** Mark an extension errored for this run, terminate its worker and tell the user. */
  private fail(info: ExtInfo, message: string) {
    this.errors.set(info.id, { message, at: this.deps.now() });
    this.terminate(info.id);
    this.deps.ui.toast(`${info.name} stopped: ${message}`, "error");
    this.changed();
  }

  terminate(id: string) {
    const r = this.running.get(id);
    if (!r) return;
    this.running.delete(id);
    r.rpc.close("extension stopped");
    r.worker.terminate();
    this.annotated.clear();
    this.changed();
  }

  /** Terminate workers of extensions that are no longer in `keep`. */
  retain(keep: Set<string>) {
    for (const id of [...this.running.keys()]) if (!keep.has(id)) this.terminate(id);
  }

  /** Start the worker for `info` if needed; resolves when `activate` has finished. */
  private async ensure(info: ExtInfo, language: string): Promise<Running | null> {
    if (this.errors.has(info.id)) return null;
    let r = this.running.get(info.id);
    if (!r) {
      const worker = this.deps.createWorker(info.id);
      const rpc = new RpcChannel(worker, (method, params) => this.serveApi(info, method, params));
      const entry = info.entry;
      const ready = (async () => {
        if (!entry) throw new RpcFailure("no_entry", "extension has no runtime.entry");
        await rpc.call(
          "activate",
          { entry: this.deps.entryUrl(info.id, entry), extension: { id: info.id, version: info.version }, language },
          TIMEOUTS.activate,
        );
      })();
      r = { info, worker, rpc, ready };
      this.running.set(info.id, r);
      this.changed();
    }
    try {
      await r.ready;
      return r;
    } catch (e) {
      if (this.running.get(info.id) === r) this.fail(info, e instanceof Error ? e.message : String(e));
      return null;
    }
  }

  /** Call a hook in an extension's worker. Timeouts and thrown errors mark it errored and return `null`. */
  async callHook(info: ExtInfo, language: string, method: string, params: unknown, timeoutMs: number): Promise<{ value: unknown } | null> {
    const r = await this.ensure(info, language);
    if (!r) return null;
    try {
      return { value: await r.rpc.call(method, params, timeoutMs) };
    } catch (e) {
      if (this.running.get(info.id) === r) this.fail(info, describe(method, e));
      return null;
    }
  }

  // ---- reader pipeline ---------------------------------------------------------------------

  /**
   * Run annotators in order. Results are cached per part + fingerprint for the session; an extension that
   * errors is skipped (and not cached, so a retry can add its annotations later).
   */
  async annotate(partKey: string, language: string, tokens: Token[], resolved: ResolvedView): Promise<Token[]> {
    const key = `${partKey}|${resolved.fingerprint}`;
    const cached = this.annotated.get(key);
    if (cached) return cached;
    const out: Token[] = tokens.map((t) => ({ ...t, ann: { ...t.ann } }));
    let complete = true;
    for (const id of resolved.annotatorOrder) {
      const info = resolved.active.find((e) => e.id === id);
      if (!info || info.runtime === "declarative") continue;
      const res = await this.callHook(info, language, "annotate", { language, tokens: out }, TIMEOUTS.annotate);
      if (!res) {
        complete = false;
        continue;
      }
      const dropped = mergeAnnotations(out, res.value, info.provides);
      if (dropped.length) console.warn(`[${info.id}] annotate returned capabilities it doesn't provide, ignored: ${dropped.join(", ")}`);
    }
    if (complete) this.annotated.set(key, out);
    return out;
  }

  /** Run decorators in parallel and layer their hints in decorator order. */
  async decorate(language: string, units: UnitLike[], tokens: Token[], resolved: ResolvedView): Promise<MergedHints[]> {
    const outputs = await Promise.all(
      resolved.decoratorOrder.map(async (id): Promise<DecoratorOutput | null> => {
        const info = resolved.active.find((e) => e.id === id);
        if (!info || info.runtime === "declarative") return null;
        const res = await this.callHook(info, language, "decorate", { language, units, tokens }, TIMEOUTS.decorate);
        return res && res.value && typeof res.value === "object" ? { id, hints: res.value as Record<number, unknown> } : null;
      }),
    );
    return mergeHints(units.length, outputs.filter((o): o is DecoratorOutput => o !== null));
  }

  // ---- events, commands, panels ------------------------------------------------------------

  /** Deliver a core or extension event to running workers allowed to see it. */
  deliverEvent(ev: CoreEvent) {
    for (const r of this.running.values()) {
      const own = ev.type.startsWith(`${r.info.id}/`);
      const isExt = ev.type.includes("/");
      const mayRead = own || (!isExt && r.info.permissions.some((p) => p.id === "events:read"));
      if (!mayRead) continue;
      r.rpc.call("event", { event: ev }, TIMEOUTS.event).catch((e) => {
        if (this.running.get(r.info.id) === r) this.fail(r.info, describe("event", e));
      });
    }
  }

  /** Make sure workers for these extensions are running (used when a reader opens, so listeners exist early). */
  async warm(language: string, resolved: ResolvedView) {
    await Promise.all(
      resolved.active.filter((e) => e.runtime === "js" || e.runtime === "wasm").map((e) => this.ensure(e, language)),
    );
  }

  async runCommand(info: ExtInfo, language: string, commandId: string, args: unknown): Promise<unknown> {
    const res = await this.callHook(info, language, "command", { language, id: commandId, args }, TIMEOUTS.command);
    return res?.value ?? null;
  }

  async panelMessage(info: ExtInfo, language: string, panelId: string, msg: unknown): Promise<unknown> {
    const res = await this.callHook(info, language, "panelMessage", { language, panelId, msg }, TIMEOUTS.panel);
    return res?.value ?? null;
  }

  settingsChanged(id: string, settings: unknown) {
    const r = this.running.get(id);
    r?.rpc.call("settingsChanged", { settings }, TIMEOUTS.event).catch(() => {});
  }

  // ---- host API for extensions -------------------------------------------------------------

  /** Handle a call from an extension (worker or panel) as `info`. The id comes from here, never from the caller. */
  serveApi(info: ExtInfo, method: string, params: unknown): Promise<unknown> | unknown {
    if (!method.startsWith("alst.")) throw new RpcFailure("no_such_method", `unknown method ${method}`);
    const name = method.slice("alst.".length);
    const p = (params ?? {}) as Record<string, unknown>;
    const ui = this.deps.ui;
    switch (name) {
      case "ui.toast":
        ui.toast(String(p.message ?? "").slice(0, 300), p.kind === "error" ? "error" : "info");
        return null;
      case "ui.openPanel": {
        const id = String(p.panelId ?? "");
        if (!info.contributes.panels.some((x) => x.id === id)) throw new RpcFailure("no_such_panel", `no panel '${id}'`);
        ui.openPanel(info.id, id);
        return null;
      }
      case "ui.openTerm":
        ui.openTerm(String(p.language ?? ""), String(p.text ?? ""));
        return null;
      case "ui.copyText":
        return Promise.resolve(ui.copyText(String(p.text ?? ""))).then(() => null);
      case "reader.current":
        return ui.readerCurrent();
      default:
        return this.deps.extCall(info.id, name, params ?? {}).catch((e) => {
          throw new RpcFailure(codeOf(String(e)), String(e));
        });
    }
  }
}

/** Rust errors arrive as plain strings; map the permission ones to a stable code for extensions. */
function codeOf(message: string): string {
  return message.startsWith("permission denied") ? "permission_denied" : "error";
}

/** Timeouts already name the method; other failures get it as a prefix. */
function describe(method: string, e: unknown): string {
  if (e instanceof RpcFailure && e.code === "timeout") return e.message;
  return `${method}: ${e instanceof Error ? e.message : String(e)}`;
}
