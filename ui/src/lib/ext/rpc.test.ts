import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RpcChannel, RpcFailure, type Port } from "./rpc";

function pair() {
  const a: Port = { postMessage: (m) => queueMicrotask(() => b.onmessage?.({ data: m })), onmessage: null };
  const b: Port = { postMessage: (m) => queueMicrotask(() => a.onmessage?.({ data: m })), onmessage: null };
  return [a, b] as const;
}

describe("RpcChannel", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("round-trips results in both directions", async () => {
    const [a, b] = pair();
    const left = new RpcChannel(a, (m, p) => `left:${m}:${JSON.stringify(p)}`);
    const right = new RpcChannel(b, (m) => `right:${m}`);
    await expect(left.call("ping")).resolves.toBe("right:ping");
    await expect(right.call("pong", { n: 1 })).resolves.toBe('left:pong:{"n":1}');
  });

  it("turns handler errors into rejections with a code", async () => {
    const [a, b] = pair();
    const left = new RpcChannel(a);
    new RpcChannel(b, () => {
      throw new RpcFailure("permission_denied", "nope");
    });
    await expect(left.call("x")).rejects.toMatchObject({ code: "permission_denied", message: "nope" });
    new RpcChannel(b, () => {
      throw new Error("boom");
    });
    await expect(left.call("x")).rejects.toMatchObject({ code: "error", message: "boom" });
  });

  it("times out a call that never gets a reply", async () => {
    const [a, b] = pair();
    const left = new RpcChannel(a);
    new RpcChannel(b, () => new Promise(() => {})); // never answers
    const p = left.call("slow", {}, 2000);
    const assertion = expect(p).rejects.toMatchObject({ code: "timeout", message: "slow timed out after 2s" });
    await vi.advanceTimersByTimeAsync(1999);
    await vi.advanceTimersByTimeAsync(2);
    await assertion;
  });

  it("ignores a late reply after a timeout and doesn't time out answered calls", async () => {
    const [a, b] = pair();
    const left = new RpcChannel(a);
    let release: (v: string) => void = () => {};
    new RpcChannel(b, (m) => (m === "late" ? new Promise((r) => (release = r)) : "ok"));
    const late = left.call("late", {}, 100);
    const lateAssertion = expect(late).rejects.toMatchObject({ code: "timeout" });
    await vi.advanceTimersByTimeAsync(101);
    await lateAssertion;
    release("too late"); // must not throw
    await vi.advanceTimersByTimeAsync(10);
    await expect(left.call("fast", {}, 100)).resolves.toBe("ok");
    await vi.advanceTimersByTimeAsync(1000); // the cleared timer doesn't fire later
  });

  it("close() rejects calls in flight and refuses new ones", async () => {
    const [a, b] = pair();
    const left = new RpcChannel(a);
    new RpcChannel(b, () => new Promise(() => {}));
    const inflight = left.call("x", {}, 5000);
    const assertion = expect(inflight).rejects.toMatchObject({ code: "closed" });
    left.close("worker terminated");
    await assertion;
    await expect(left.call("y")).rejects.toMatchObject({ code: "closed", message: "worker terminated" });
    await vi.advanceTimersByTimeAsync(10000); // no stray timeout rejection
  });
});
