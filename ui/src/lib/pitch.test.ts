import { describe, expect, it } from "vitest";
import { highLow, morae, patternOf } from "./pitch";

describe("morae", () => {
  it("counts small kana with the previous one and ー, っ, ん separately", () => {
    expect(morae("きょう")).toEqual(["きょ", "う"]);
    expect(morae("がっこう")).toEqual(["が", "っ", "こ", "う"]);
    expect(morae("ラーメン")).toEqual(["ラ", "ー", "メ", "ン"]);
    expect(morae("しゃしん")).toHaveLength(3);
  });
});

describe("patternOf", () => {
  it("classifies by downstep", () => {
    expect(patternOf(3, 0)).toBe("heiban");
    expect(patternOf(3, 1)).toBe("atamadaka");
    expect(patternOf(3, 3)).toBe("odaka");
    expect(patternOf(4, 2)).toBe("nakadaka");
    expect(patternOf(1, 1)).toBe("atamadaka"); // a one-mora word with downstep 1 is atamadaka, not odaka
  });
});

describe("highLow", () => {
  it("draws heiban low-high-high with a high particle", () => {
    expect(highLow(3, 0)).toEqual([false, true, true, true]);
  });
  it("draws atamadaka high then low", () => {
    expect(highLow(3, 1)).toEqual([true, false, false, false]);
  });
  it("draws nakadaka and odaka", () => {
    expect(highLow(4, 2)).toEqual([false, true, false, false, false]);
    expect(highLow(3, 3)).toEqual([false, true, true, false]);
  });
});
