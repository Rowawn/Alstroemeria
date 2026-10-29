import { describe, expect, it } from "vitest";
import type { PackPlan, PackPlanItem } from "./api";
import { Review, selectable } from "./pack-review.svelte";

const item = (id: string, status: PackPlanItem["status"]): PackPlanItem => ({
  id, kind: "article", title: id, tags: [], how: "fetch/article", domain: "example.com", license: null, attribution: null,
  words: null, minutes: null, transcript: null, status, checked: status === "ready",
});
const plan = (items: PackPlanItem[], over: Partial<PackPlan> = {}): PackPlan => ({
  id: "p", title: "T", language: "es", topic: null, level: "B1", description: null, createdBy: null, createdAt: null,
  items, vocabTotal: 2, vocab: { new: 2, fill: 0, existing: 0 }, languageInstalled: true, ...over,
});

describe("pack review state", () => {
  it("checks ready items only and refuses to tick the others", () => {
    const r = new Review(plan([item("a", "ready"), item("b", "needs-desktop"), item("c", "already-imported"), item("d", "license-warning")]));
    expect(r.chosen).toEqual(["a"]);
    r.toggle("b");
    r.toggle("c");
    expect(r.chosen).toEqual(["a"]);
    r.toggle("d"); // a licence warning may be ticked on purpose
    expect(r.chosen).toEqual(["a", "d"]);
    expect(selectable(item("b", "needs-desktop"))).toBe(false);
    r.setAll(false);
    expect(r.chosen).toEqual([]);
    r.setAll(true);
    expect(r.chosen).toEqual(["a"]); // the warning item stays unchecked
  });

  it("can import when something is chosen or vocabulary alone is wanted", () => {
    const r = new Review(plan([item("a", "already-imported")]));
    expect(r.canImport).toBe(true); // seed vocab only
    r.seedVocab = false;
    expect(r.canImport).toBe(false);
    expect(new Review(plan([item("a", "ready")], { languageInstalled: false })).canImport).toBe(false);
  });

  it("tracks progress, failures and retries only the failed items", () => {
    const r = new Review(plan([item("a", "ready"), item("b", "ready"), item("c", "ready")]));
    r.start(r.chosen);
    expect(r.phase).toBe("running");
    r.onProgress({ handle: "h", itemId: "a", index: 1, total: 3, state: "started", error: null, workId: null });
    expect(r.itemState.a).toBe("running");
    r.onProgress({ handle: "h", itemId: "a", index: 1, total: 3, state: "done", error: null, workId: "w1" });
    r.onProgress({ handle: "h", itemId: "b", index: 2, total: 3, state: "failed", error: "fetch failed", workId: null });
    r.finish({ imported: [["a", "w1"]], skipped: [], failed: [{ itemId: "b", error: "fetch failed" }], vocab: null, cancelled: true });
    expect(r.phase).toBe("done");
    expect(r.itemState).toEqual({ a: "done", b: "failed" }); // c was never reached
    expect(r.errors.b).toBe("fetch failed");
    expect(r.prepareRetry()).toEqual(["b"]);
    expect(r.phase).toBe("review");
    expect(r.chosen).toEqual(["b"]);
  });
});
