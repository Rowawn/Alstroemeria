import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ExtensionHost, TIMEOUTS, type HostDeps, type UiBridge } from "./host";
import type { Port } from "./rpc";
import type { ExtInfo, ResolvedView, Token } from "./types";

/** An in-process stand-in for a worker: `handler` plays the extension. */
type Handler = (method: string, params: any, workerCallsHost: (method: string, params?: unknown) => Promise<unknown>) => unknown;

function fakeWorkerFactory(handlers: Record<string, Handler>, created: string[]) {
  return {
    create(id: string): Port & { terminate(): void } {
      created.push(id);
      let nextId = 1;
      const pending = new Map<number, (m: any) => void>();
      let dead = false;
      const w: Port & { terminate(): void } = {
        onmessage: null,
        terminate() {
          dead = true;
        },
        postMessage(msg: any) {
          if (dead) return;
          if (typeof msg.method !== "string") return pending.get(msg.id)?.(msg);
          const callHost = (method: string, params?: unknown) =>
            new Promise((resolve, reject) => {
              const rid = nextId++;
              pending.set(rid, (m) => (m.error ? reject(new Error(m.error.message)) : resolve(m.result)));
              queueMicrotask(() => w.onmessage?.({ data: { id: rid, method, params } }));
            });
          queueMicrotask(async () => {
            try {
              const result = await handlers[id](msg.method, msg.params, callHost);
              if (!dead) w.onmessage?.({ data: { id: msg.id, result: result ?? null } });
            } catch (e) {
              if (!dead) w.onmessage?.({ data: { id: msg.id, error: { code: "error", message: (e as Error).message } } });
            }
          });
        },
      };
      return w;
    },
  };
}

const info = (id: string, extra: Partial<ExtInfo> = {}): ExtInfo => ({
  id, name: id.toUpperCase(), version: "1.0.0", kind: "addon", description: null, author: null, license: "MIT", source: "installed",
  runtime: "js", entry: "main.js", targets: ["*"], language: null, provides: [], requires: [], optional: [], permissions: [], platforms: null,
  contributes: { decorators: [], panels: [], commands: [], lookupProviders: [], screens: [], termActions: [] }, unsupported: [], enabled: true, languages: [], settings: {}, ...extra,
});

const view = (active: ExtInfo[], extra: Partial<ResolvedView> = {}): ResolvedView => ({
  language: "es", active, annotatorOrder: active.map((e) => e.id), decoratorOrder: active.filter((e) => e.contributes.decorators.length).map((e) => e.id),
  disabled: [], providerChoices: [], fingerprint: "fp1", ...extra,
});

const tokens = (): Token[] =>
  ["hola", " ", "mundo"].map((text, i) => ({ i, text, isWord: i !== 1, norm: text, start: i, end: i + 1, sentence: 0, ann: {} }));

describe("ExtensionHost", () => {
  let toasts: string[];
  let created: string[];
  let calls: [string, string, unknown][];
  let opened: [string, string, unknown][];
  let picked: [string, string][];
  let factory: ReturnType<typeof fakeWorkerFactory>;
  let handlers: Record<string, Handler>;
  let host: ExtensionHost;

  beforeEach(() => {
    vi.useFakeTimers();
    toasts = [];
    created = [];
    calls = [];
    opened = [];
    picked = [];
    handlers = {};
    factory = fakeWorkerFactory(handlers, created);
    const ui: UiBridge = {
      toast: (m) => void toasts.push(m),
      openPanel: () => {}, openTerm: () => {}, copyText: () => {}, readerCurrent: () => null,
      openScreen: (e, sid, payload) => void opened.push([e, sid, payload]),
      openPart: () => {},
      pickFile: async (e, accept) => (picked.push([e, accept]), { uploadId: "up1", name: "deck.apkg" }),
    };
    const deps: HostDeps = {
      createWorker: (id) => factory.create(id),
      entryUrl: (id, path) => `alstext://${id}/${path}`,
      extCall: async (id, method, params) => (calls.push([id, method, params]), { ok: true }),
      ui,
      now: () => 42,
    };
    host = new ExtensionHost(deps);
  });
  afterEach(() => vi.useRealTimers());

  it("activates once, runs annotators in order, and merges only provided capabilities", async () => {
    const a = info("a", { provides: ["es.x"] });
    const b = info("b", { provides: ["es.y"], requires: ["es.x"] });
    const seen: unknown[] = [];
    handlers.a = (m, p) => (m === "annotate" ? { 0: { "es.x": 1, "es.other": 9 } } : null);
    handlers.b = (m, p) => {
      if (m === "annotate") seen.push({ ...p.tokens[0].ann });
      return m === "annotate" ? { 0: { "es.y": 2 } } : null;
    };
    const resolved = view([a, b]);
    const out = await host.annotate("part1", "es", tokens(), resolved);
    expect(out[0].ann).toEqual({ "es.x": 1, "es.y": 2 });
    expect(seen).toEqual([{ "es.x": 1 }]); // b saw a's annotations
    expect(created).toEqual(["a", "b"]);
    await host.annotate("part1", "es", tokens(), resolved); // cached: no new calls, no new workers
    expect(created).toHaveLength(2);
    await host.annotate("part2", "es", tokens(), resolved);
    expect(created).toHaveLength(2); // workers are reused
    expect(host.status("a")).toBe("ready");
  });

  it("a fingerprint change invalidates the annotation cache", async () => {
    let n = 0;
    handlers.a = (m) => (m === "annotate" ? { 0: { "es.x": ++n } } : null);
    const a = info("a", { provides: ["es.x"] });
    expect((await host.annotate("p", "es", tokens(), view([a])))[0].ann["es.x"]).toBe(1);
    expect((await host.annotate("p", "es", tokens(), view([a])))[0].ann["es.x"]).toBe(1);
    expect((await host.annotate("p", "es", tokens(), view([a], { fingerprint: "fp2" })))[0].ann["es.x"]).toBe(2);
  });

  it("a slow annotator times out after 2s, is marked errored, the reader still gets tokens and a toast names it", async () => {
    handlers.slow = (m) => (m === "annotate" ? new Promise(() => {}) : null);
    const slow = info("slow", { provides: ["es.x"] });
    const p = host.annotate("p", "es", tokens(), view([slow]));
    await vi.advanceTimersByTimeAsync(TIMEOUTS.annotate + 10);
    const out = await p;
    expect(out).toHaveLength(3);
    expect(out[0].ann).toEqual({});
    expect(host.status("slow")).toBe("errored");
    expect(host.error("slow")?.message).toContain("annotate timed out after 2s");
    expect(toasts).toEqual([expect.stringContaining("SLOW stopped: annotate timed out after 2s")]);
    // Errored extensions aren't restarted for the rest of the run...
    await host.annotate("p2", "es", tokens(), view([slow]));
    expect(created).toEqual(["slow"]);
    // ...until the user retries.
    host.clearError("slow");
    handlers.slow = (m) => (m === "annotate" ? { 0: { "es.x": 1 } } : null);
    expect((await host.annotate("p3", "es", tokens(), view([slow])))[0].ann).toEqual({ "es.x": 1 });
    expect(created).toEqual(["slow", "slow"]);
  });

  it("a hook that throws errors the extension but others keep working", async () => {
    handlers.bad = (m) => {
      if (m === "annotate") throw new Error("kaboom");
      return null;
    };
    handlers.good = (m) => (m === "annotate" ? { 0: { "es.g": 1 } } : null);
    const out = await host.annotate("p", "es", tokens(), view([info("bad", { provides: ["es.b"] }), info("good", { provides: ["es.g"] })]));
    expect(out[0].ann).toEqual({ "es.g": 1 });
    expect(host.status("bad")).toBe("errored");
    expect(host.error("bad")?.message).toContain("kaboom");
    expect(host.status("good")).toBe("ready");
  });

  describe("lookup providers (SPEC §16.8)", () => {
    const input = { language: "ja", unit: { text: "食べた", key: "食べる", lemma: "食べる", status: 0 }, tokens: [], sentence: "食べた。" };
    const lp = (id: string, extra: Partial<ExtInfo> = {}) =>
      info(id, { contributes: { decorators: [], panels: [], commands: [], lookupProviders: [{ id: "l", title: "L" }], screens: [], termActions: [] }, ...extra });

    it("collects sections in decorator order and drops malformed output", async () => {
      handlers.p1 = (m) => (m === "lookup" ? [{ title: "One", items: [{ kind: "text", text: "a" }] }] : null);
      handlers.p2 = (m) =>
        m === "lookup"
          ? [
              { title: "Two", items: [{ kind: "link", label: "x", url: "http://insecure.example" }, { kind: "pitch", reading: "はし", downstep: 2 }] },
              { title: "", items: [{ kind: "text", text: "no title" }] },
              { title: "Empty", items: [{ kind: "hologram" }] },
            ]
          : null;
      const sections = await host.lookup("ja", input, view([lp("p1"), lp("p2")], { decoratorOrder: ["p2", "p1"] }));
      expect(sections.map((s) => s.title)).toEqual(["Two", "One"]);
      expect(sections[0].items).toEqual([{ kind: "pitch", reading: "はし", downstep: 2 }]); // http link dropped
    });

    it("times out after 2s, skips that provider and marks it errored", async () => {
      handlers.slow = (m) => (m === "lookup" ? new Promise(() => {}) : null);
      handlers.fast = (m) => (m === "lookup" ? [{ title: "Fast", items: [{ kind: "kv", label: "k", text: "v" }] }] : null);
      const p = host.lookup("ja", input, view([lp("slow"), lp("fast")]));
      await vi.advanceTimersByTimeAsync(TIMEOUTS.lookup + 10);
      expect((await p).map((s) => s.title)).toEqual(["Fast"]);
      expect(TIMEOUTS.lookup).toBe(2000);
      expect(host.error("slow")?.message).toContain("lookup timed out after 2s");
      expect(host.status("fast")).toBe("ready");
    });

    it("ignores extensions without lookup providers and providers with no lookup export", async () => {
      handlers.none = () => null;
      handlers.noexport = (m) => (m === "lookup" ? [] : null);
      expect(await host.lookup("ja", input, view([info("none"), lp("noexport")]))).toEqual([]);
      expect(created).toEqual(["noexport"]);
    });
  });

  it("activation that never finishes errors after 5s", async () => {
    handlers.hang = () => new Promise(() => {});
    const p = host.warm("es", view([info("hang")]));
    await vi.advanceTimersByTimeAsync(TIMEOUTS.activate + 10);
    await p;
    expect(host.error("hang")?.message).toContain("activate timed out after 5s");
  });

  it("decorators run in parallel and are layered in decorator order", async () => {
    const mk = (role: string) => () => ({ 0: [{ kind: "color", role }, { kind: "tooltip", text: role }] });
    handlers.d1 = (m) => (m === "decorate" ? mk("verb")() : null);
    handlers.d2 = (m) => (m === "decorate" ? mk("noun")() : null);
    const dec = { decorators: [{ id: "d", priority: 50 }], panels: [], commands: [], lookupProviders: [], screens: [], termActions: [] };
    const d1 = info("d1", { contributes: dec });
    const d2 = info("d2", { contributes: dec });
    const units = [{ text: "hola", isWord: true, key: "hola", wordCount: 1, status: 0, termId: null, sentence: 0, start: 0, end: 4, tokenRange: [0, 0] as [number, number] }];
    const merged = await host.decorate("es", units, tokens(), view([d1, d2], { decoratorOrder: ["d2", "d1"] }));
    expect(merged[0].color).toBe("noun");
    expect(merged[0].tooltip).toBe("noun\nverb");
  });

  it("routes alst.* calls to Rust tagged with the worker's own extension id, ignoring any id the extension supplies", async () => {
    handlers.a = async (m, _p, callHost) => {
      if (m === "annotate") {
        await callHost("alst.terms.get", { language: "es", extId: "someone.else", extension_id: "x" });
        await callHost("alst.ui.toast", { message: "hi", kind: "error" });
      }
      return {};
    };
    await host.annotate("p", "es", tokens(), view([info("a")]));
    expect(calls).toEqual([["a", "terms.get", { language: "es", extId: "someone.else", extension_id: "x" }]]);
    expect(toasts).toContain("hi");
  });

  it("only forwards core events to extensions with events:read, and their own events always", async () => {
    const got: Record<string, string[]> = { reader: [], plain: [] };
    handlers.reader = (m, p) => (m === "event" ? void got.reader.push(p.event.type) : null);
    handlers.plain = (m, p) => (m === "event" ? void got.plain.push(p.event.type) : null);
    const reader = info("reader", { permissions: [{ id: "events:read", text: "", warning: false }] });
    await host.warm("es", view([reader, info("plain")]));
    const ev = (type: string) => ({ id: "1", ts: 1, type, language: "es", subjectType: null, subjectId: null, payload: {} });
    for (const t of ["part.read", "plain/mine", "reader/mine", "other/theirs"]) host.deliverEvent(ev(t));
    await vi.advanceTimersByTimeAsync(10);
    expect(got.reader).toEqual(["part.read", "reader/mine"]);
    expect(got.plain).toEqual(["plain/mine"]);
  });

  it("retain() terminates workers that are no longer needed", async () => {
    handlers.a = () => null;
    await host.warm("es", view([info("a")]));
    expect(host.status("a")).toBe("ready");
    host.retain(new Set());
    expect(host.status("a")).toBe("idle");
  });

  describe("screens, term actions and alst.srs (SPEC §11.1)", () => {
    const mod = (extra: Partial<ExtInfo["contributes"]> = {}, permissions: string[] = []) =>
      info("mod", {
        permissions: permissions.map((id) => ({ id, text: id, warning: false })),
        contributes: {
          decorators: [], panels: [], lookupProviders: [],
          commands: [{ id: "mine", title: "Mine", context: "term" }],
          screens: [{ id: "review", title: "Review", entry: "screen.html" }],
          termActions: [{ id: "mine-action", title: "Mine sentence", command: "mine" }],
          ...extra,
        },
      });

    it("openScreen shows only a screen the extension declares, with its payload", async () => {
      const m = mod();
      await host.serveApi(m, "alst.ui.openScreen", { screenId: "review", payload: { view: "mine" } });
      expect(opened).toEqual([["mod", "review", { view: "mine" }]]);
      expect(() => host.serveApi(m, "alst.ui.openScreen", { screenId: "other" })).toThrow(/no screen/);
      expect(opened).toHaveLength(1);
      // Another extension's screen is not reachable by name.
      expect(() => host.serveApi(info("evil"), "alst.ui.openScreen", { screenId: "review" })).toThrow(/no screen/);
    });

    it("pickFile is asked on behalf of the calling extension only", async () => {
      expect(await host.serveApi(mod(), "alst.ui.pickFile", { accept: ".apkg,.colpkg" })).toEqual({ uploadId: "up1", name: "deck.apkg" });
      expect(picked).toEqual([["mod", ".apkg,.colpkg"]]);
    });

    it("srs calls go to Rust tagged with the extension id, whatever the caller says", async () => {
      await host.serveApi(mod({}, ["srs:write"]), "alst.srs.answer", { cardId: "c1", rating: 3, extId: "someone.else" });
      expect(calls).toEqual([["mod", "srs.answer", { cardId: "c1", rating: 3, extId: "someone.else" }]]);
    });

    it("a refused srs call reaches the extension as permission_denied", async () => {
      host = new ExtensionHost({
        createWorker: (id) => factory.create(id),
        entryUrl: () => "",
        extCall: async () => {
          throw "permission denied: 'mod' needs the srs:write permission";
        },
        ui: { toast: () => {}, openPanel: () => {}, openTerm: () => {}, copyText: () => {}, readerCurrent: () => null, openScreen: () => {}, openPart: () => {}, pickFile: async () => null },
        now: () => 1,
      });
      await expect(host.serveApi(mod(), "alst.srs.answer", {})).rejects.toMatchObject({ code: "permission_denied" });
    });

    it("a term action calls its command with the term, sentence, part and cue", async () => {
      const m = mod();
      let got: any;
      handlers.mod = (method, p) => {
        if (method === "command") got = p;
        return null;
      };
      const args = {
        term: "gato", language: "es", sentence: { text: "El gato duerme.", start: 10, end: 25 }, partId: "p1", workId: "w1",
        cue: { start: 1000, end: 2500, mediaHash: "ab" },
      };
      await host.runTermAction(m, "es", "mine-action", args);
      expect(got).toEqual({ language: "es", id: "mine", args });
      await expect(host.runTermAction(m, "es", "nope", args)).rejects.toThrow(/no term action/);
    });
  });
});
