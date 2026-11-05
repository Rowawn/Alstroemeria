// module.srs against a fake `alst` (SPEC §11.1, §11.5): it uses only the public host API, and side effects run only for
// status changes made on this device.
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../../..");
const loadJs = (rel: string) => import(/* @vite-ignore */ join(root, rel));

describe("module.srs", () => {
  let calls: [string, unknown][];
  let toasts: string[];
  let opened: [string, unknown][];
  let mod: any;

  beforeEach(async () => {
    calls = [];
    toasts = [];
    opened = [];
    vi.stubGlobal("alst", {
      srs: {
        addTerm: async (p: unknown) => (calls.push(["addTerm", p]), { termId: "t1", created: calls.length === 1 ? ["c1"] : [] }),
        mine: async (p: unknown) => (calls.push(["mine", p]), { noteId: "n", cardIds: ["c"] }),
        onTermStatus: async (p: unknown) => void calls.push(["onTermStatus", p]),
      },
      terms: { get: async () => [{ translation: "cat" }] },
      ui: { toast: async (m: string) => void toasts.push(m), openScreen: async (id: string, payload: unknown) => void opened.push([id, payload]) },
    });
    mod = await loadJs("extensions/module-srs/main.js");
  });
  afterEach(() => vi.unstubAllGlobals());

  it("'Add to review' sends the term and says whether it was new", async () => {
    await mod.commands["add-to-review"]({ language: "es", term: "gato" });
    await mod.commands["add-to-review"]({ language: "es", term: "gato" });
    expect(calls[0]).toEqual(["addTerm", { language: "es", text: "gato" }]);
    expect(toasts).toEqual(["Added to review", "Already in review"]);
  });

  it("'Mine sentence' saves the sentence with the term's meaning and audio clip, then says so", async () => {
    const args = { term: "gato", language: "es", sentence: { text: "El gato <duerme>.", start: 0, end: 17 }, partId: "p", workId: "w", cue: { start: 1, end: 2, mediaHash: "ab" } };
    await mod.commands["mine-sentence"](args);
    const m: any = calls.find((c) => c[0] === "mine")![1];
    expect(m).toMatchObject({ target: "gato", meaning: "cat", sentence: "El <b>gato</b> &lt;duerme&gt;.", audio: "[sound:alstclip:ab:1:2]" });
    expect(toasts).toEqual(["Sentence added to review"]);
  });

  it("reacts to status changes made on this device only", async () => {
    await mod.onEvent({ type: "term.status", own: false, subjectId: "t1", payload: { from: 0, to: 2 } });
    await mod.onEvent({ type: "term.lookup", own: true, subjectId: "t1", payload: {} });
    expect(calls).toEqual([]);
    await mod.onEvent({ type: "term.status", own: true, subjectId: "t1", payload: { from: 0, to: 2 } });
    expect(calls).toEqual([["onTermStatus", { termId: "t1", to: 2 }]]);
  });

  it("its manifest declares the screen, the term actions and exactly the permissions it uses", () => {
    const m = JSON.parse(readFileSync(join(root, "extensions/module-srs/extension.json"), "utf8"));
    expect(m.contributes.screens.map((s: any) => s.id)).toEqual(["review"]);
    expect(m.contributes.termActions.map((a: any) => a.command).sort()).toEqual(["add-to-review", "mine-sentence"]);
    expect(m.permissions.sort()).toEqual(["events:read", "srs:write", "terms:read"]);
    expect(m.license).toBe("MIT");
  });

  it("the screen uses only alst.srs and alst.ui from the public API", () => {
    const src = readFileSync(join(root, "extensions/module-srs/screen.js"), "utf8");
    const used = new Set([...src.matchAll(/alst\.call\("([a-zA-Z.]+)"/g)].map((m) => m[1]));
    expect([...used]).toEqual([]);
    expect(src).not.toMatch(/invoke\(|__TAURI|fetch\(/);
  });
});
