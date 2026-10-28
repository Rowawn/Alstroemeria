import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import { errorText, megabytes, needsLangData } from "./langdata-state.svelte";

describe("language data errors", () => {
  it("recognises the core's marker", () => {
    expect(needsLangData("langdata:ja-ipadic")).toBe("ja-ipadic");
    expect(needsLangData(new Error("langdata:ko-kodic"))).toBeNull();
    expect(needsLangData("something else")).toBeNull();
  });
  it("explains it where no card fits", () => {
    expect(errorText("langdata:ja-ipadic")).toContain("Languages tab");
    expect(errorText("boom")).toBe("boom");
  });
  it("formats sizes", () => {
    expect(megabytes(10 * (1 << 20))).toBe("10 MB");
  });
});
