import { describe, expect, it } from "vitest";
import { cueAt, clock, groupUnits, repeatTarget, type Cue } from "./cues";

const cues: Cue[] = [
  { start: 1000, end: 2000, a: 0, b: 5 },
  { start: 2500, end: 4000, a: 6, b: 12 },
  { start: 4000, end: 4500, a: 13, b: 16 },
];

describe("cueAt", () => {
  it("finds the last cue that has started", () => {
    expect(cueAt(cues, 0)).toBe(-1);
    expect(cueAt(cues, 1000)).toBe(0);
    expect(cueAt(cues, 2200)).toBe(0); // in the gap the previous cue stays current
    expect(cueAt(cues, 2500)).toBe(1);
    expect(cueAt(cues, 4000)).toBe(2);
    expect(cueAt(cues, 99999)).toBe(2);
    expect(cueAt([], 5)).toBe(-1);
  });
});

describe("groupUnits", () => {
  it("assigns units (including the newline between cues) to the cue they start in", () => {
    const units = [0, 3, 5, 6, 10, 12, 13].map((start) => ({ start }));
    expect(groupUnits(units, cues)).toEqual([[0, 1, 2], [3, 4, 5], [6]]);
  });
  it("skips units before the first cue", () => {
    expect(groupUnits([{ start: 0 }, { start: 20 }], [{ start: 0, end: 1, a: 10, b: 30 }])).toEqual([[1]]);
  });
});

describe("repeatTarget", () => {
  it("jumps back near the end of the locked cue only", () => {
    expect(repeatTarget(cues, 1, 3000)).toBeNull();
    expect(repeatTarget(cues, 1, 3990)).toBe(2500);
    expect(repeatTarget(cues, -1, 3990)).toBeNull();
  });
});

describe("clock", () => {
  it("formats m:ss and h:mm:ss", () => {
    expect(clock(0)).toBe("0:00");
    expect(clock(75_900)).toBe("1:15");
    expect(clock(3_725_000)).toBe("1:02:05");
  });
});
