import { describe, expect, it, vi } from "vitest";

const handlers = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
    handlers.set(name, cb);
    return () => {};
  }),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => ({ deviceName: "Mac", peers: [] })) }));

describe("sync state", () => {
  it("counts applied batches, shows the pairing code and clears it when paired", async () => {
    const { sync } = await import("./sync.svelte");
    sync.start();
    await Promise.resolve();
    const tick = sync.tick;
    handlers.get("sync://applied")!({ payload: ["terms"] });
    expect(sync.tick).toBe(tick + 1);

    handlers.get("sync://pair-code")!({ payload: { peerName: "Phone", code: "123456" } });
    expect(sync.code?.code).toBe("123456");
    handlers.get("sync://paired")!({ payload: { siteId: "s", name: "Phone", publicKey: "k" } });
    expect(sync.code).toBeNull();
    expect(sync.paired?.name).toBe("Phone");
    // A failed pairing arrives as an Err result and leaves nothing paired.
    handlers.get("sync://paired")!({ payload: { Err: "declined" } });
    expect(sync.paired).toBeNull();
  });

  it("takes the pushed overview when the service reports state", async () => {
    const { sync } = await import("./sync.svelte");
    sync.start();
    handlers.get("sync://state")!({ payload: { deviceName: "Phone", peers: [] } });
    expect(sync.overview?.deviceName).toBe("Phone");
  });
});
