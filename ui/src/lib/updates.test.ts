import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
let feed: unknown = null;
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    calls.push(cmd);
    if (cmd === "update_info") return { version: "0.1.0", autoCheck: false, canInstall: true };
    if (cmd === "update_check") {
      if (feed instanceof Error) throw feed;
      return feed;
    }
    return undefined;
  }),
}));

import { updates } from "./updates.svelte";

beforeEach(() => {
  calls.length = 0;
  updates.available = null;
  updates.error = "";
  updates.upToDate = false;
});

describe("update checks", () => {
  it("does not check by itself when the setting is off", async () => {
    await updates.start();
    expect(calls).toEqual(["update_info"]);
  });

  it("a manual check with nothing new says so", async () => {
    feed = null;
    await updates.check(true);
    expect(updates.upToDate).toBe(true);
    expect(updates.available).toBeNull();
  });

  it("a manual check shows an error, an automatic one stays quiet", async () => {
    feed = new Error("offline");
    await updates.check(false);
    expect(updates.error).toBe("");
    await updates.check(true);
    expect(updates.error).toContain("offline");
  });

  it("a newer version is offered", async () => {
    feed = { version: "0.2.0", notes: "x", canInstall: true, pageUrl: null };
    await updates.check(false);
    expect(updates.available?.version).toBe("0.2.0");
  });
});
