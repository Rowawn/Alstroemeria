// JSON-RPC-like channel over a message port (SPEC §6.9.5): `{id, method, params}` → `{id, result}` | `{id, error}`.

import type { RpcError } from "./types";

export type Port = {
  postMessage(msg: unknown): void;
  onmessage: ((e: { data: unknown }) => void) | null;
};

type Request = { id: number; method: string; params?: unknown };
type Response = { id: number; result?: unknown; error?: RpcError };

export class RpcFailure extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

export class RpcChannel {
  private next = 1;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void; timer?: ReturnType<typeof setTimeout> }>();
  private closed: RpcFailure | null = null;

  constructor(
    private port: Port,
    private onRequest?: (method: string, params: unknown) => unknown | Promise<unknown>,
  ) {
    port.onmessage = (e) => this.receive(e.data);
  }

  /** Send a request. With `timeoutMs`, rejects with code `timeout` if no reply arrives in time. */
  call(method: string, params?: unknown, timeoutMs?: number): Promise<unknown> {
    if (this.closed) return Promise.reject(this.closed);
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const entry: { resolve: (v: unknown) => void; reject: (e: Error) => void; timer?: ReturnType<typeof setTimeout> } = { resolve, reject };
      if (timeoutMs !== undefined) {
        entry.timer = setTimeout(() => {
          this.pending.delete(id);
          reject(new RpcFailure("timeout", `${method} timed out after ${timeoutMs / 1000}s`));
        }, timeoutMs);
      }
      this.pending.set(id, entry);
      this.port.postMessage({ id, method, params } satisfies Request);
    });
  }

  /** Reject everything in flight and refuse new calls (the worker was terminated). */
  close(reason: string) {
    this.closed = new RpcFailure("closed", reason);
    for (const [, p] of this.pending) {
      clearTimeout(p.timer);
      p.reject(this.closed);
    }
    this.pending.clear();
    this.port.onmessage = null;
  }

  private receive(data: unknown) {
    if (!data || typeof data !== "object") return;
    const m = data as Partial<Request & Response>;
    if (typeof m.id !== "number") return;
    if (typeof m.method === "string") {
      void this.serve(m as Request);
      return;
    }
    const p = this.pending.get(m.id);
    if (!p) return; // late reply after a timeout
    this.pending.delete(m.id);
    clearTimeout(p.timer);
    if (m.error) p.reject(new RpcFailure(m.error.code ?? "error", m.error.message ?? "error"));
    else p.resolve(m.result);
  }

  private async serve(req: Request) {
    try {
      if (!this.onRequest) throw new RpcFailure("no_handler", `no handler for ${req.method}`);
      const result = await this.onRequest(req.method, req.params);
      this.port.postMessage({ id: req.id, result: result === undefined ? null : result } satisfies Response);
    } catch (e) {
      const code = e instanceof RpcFailure ? e.code : "error";
      const message = e instanceof Error ? e.message : String(e);
      this.port.postMessage({ id: req.id, error: { code, message } } satisfies Response);
    }
  }
}
