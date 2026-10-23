import { describe, expect, it } from "vitest";
import { mergeAnnotations, mergeHints, sanitizeHints } from "./hints";

describe("sanitizeHints", () => {
  it("keeps valid hints, drops unknown kinds and bad roles", () => {
    const out = sanitizeHints([
      { kind: "ruby", text: "かん" },
      { kind: "color", role: "verb" },
      { kind: "color", role: "hotpink" },
      { kind: "underline", role: "adverb", style: "wavy" },
      { kind: "underline", role: "adverb", style: "zigzag" },
      { kind: "pitchLine", pattern: ["H", "L"] },
      { kind: "hologram" },
      null,
      "x",
    ]);
    expect(out).toEqual([
      { kind: "ruby", text: "かん" },
      { kind: "color", role: "verb" },
      { kind: "underline", role: "adverb", style: "wavy" },
      { kind: "underline", role: "adverb", style: undefined },
    ]);
    expect(sanitizeHints("nope")).toEqual([]);
  });

  it("limits badge text to 4 characters and drops empty text", () => {
    expect(sanitizeHints([{ kind: "badge", text: "ABCDEFG", role: "info" }, { kind: "badge", text: "" }, { kind: "tooltip", text: "" }])).toEqual([
      { kind: "badge", text: "ABCD", role: "info" },
    ]);
    expect(sanitizeHints([{ kind: "badge", text: "日本語です!" }])[0]).toMatchObject({ text: "日本語で" });
  });
});

describe("mergeHints", () => {
  const h = (hints: Record<number, unknown>, id = "x") => ({ id, hints });

  it("first decorator in order wins ruby, color and underline", () => {
    const [m] = mergeHints(1, [
      h({ 0: [{ kind: "color", role: "verb" }, { kind: "underline", role: "verb" }, { kind: "ruby", text: "a" }] }, "first"),
      h({ 0: [{ kind: "color", role: "noun" }, { kind: "underline", role: "noun", style: "dotted" }, { kind: "ruby", text: "b" }] }, "second"),
    ]);
    expect(m).toMatchObject({ color: "verb", underline: { role: "verb", style: "solid" }, ruby: "a" });
  });

  it("a later decorator can fill in what an earlier one left unset", () => {
    const [m] = mergeHints(1, [h({ 0: [{ kind: "color", role: "verb" }] }), h({ 0: [{ kind: "ruby", text: "b" }] })]);
    expect(m).toMatchObject({ color: "verb", ruby: "b" });
  });

  it("badges accumulate up to two; tooltips accumulate joined by line breaks", () => {
    const [m] = mergeHints(1, [
      h({ 0: [{ kind: "badge", text: "1" }, { kind: "tooltip", text: "one" }] }),
      h({ 0: [{ kind: "badge", text: "2" }, { kind: "badge", text: "3" }, { kind: "tooltip", text: "two" }] }),
    ]);
    expect(m.badges.map((b) => b.text)).toEqual(["1", "2"]);
    expect(m.tooltip).toBe("one\ntwo");
  });

  it("ignores out-of-range or non-numeric unit indexes and non-array hint lists", () => {
    const merged = mergeHints(2, [h({ 5: [{ kind: "color", role: "noun" }], "-1": [{ kind: "color", role: "noun" }], x: [], 1: "bad" } as Record<number, unknown>)]);
    expect(merged).toHaveLength(2);
    expect(merged.every((m) => !m.color && m.badges.length === 0)).toBe(true);
  });

  it("returns empty hints when there are no decorators", () => {
    expect(mergeHints(3, [])).toEqual([{ badges: [] }, { badges: [] }, { badges: [] }]);
  });
});

describe("mergeAnnotations", () => {
  it("keeps only provided capabilities and reports the rest", () => {
    const tokens = [{ ann: {} as Record<string, unknown> }, { ann: {} as Record<string, unknown> }];
    const dropped = mergeAnnotations(tokens, { 0: { "es.morph": "gerund", "es.secret": 1 }, 1: { "es.morph": "inf" }, 9: { "es.morph": "x" } }, ["es.morph"]);
    expect(tokens[0].ann).toEqual({ "es.morph": "gerund" });
    expect(tokens[1].ann).toEqual({ "es.morph": "inf" });
    expect(dropped).toEqual(["es.secret"]);
  });

  it("tolerates garbage results", () => {
    const tokens = [{ ann: {} }];
    expect(mergeAnnotations(tokens, null, ["a.b"])).toEqual([]);
    expect(mergeAnnotations(tokens, { 0: "str" }, ["a.b"])).toEqual([]);
    expect(tokens[0].ann).toEqual({});
  });
});
