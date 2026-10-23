// Wires the extension host to the real app: Web Workers, Tauri commands, toasts and the reader.

import { api } from "../api";
import { toast } from "../notify.svelte";
import { ExtensionHost, type UiBridge } from "./host";
// Inlined and started from a blob: URL on purpose. A worker started from a same-origin script URL takes its CSP from
// that script's HTTP response (which has none for app assets), so it could reach the network. A blob worker inherits
// the page's CSP, whose connect-src allows only IPC and alstext:// (SPEC §6.9.5).
import BootstrapWorker from "./worker-bootstrap.ts?worker&inline";

/** Windows and Android serve custom protocols as `http://<scheme>.localhost/` (SPEC §6.9.2). */
const httpForm = () => /Windows|Android/i.test(navigator.userAgent);

export function extUrl(extId: string, path: string): string {
  const p = path.split("/").map(encodeURIComponent).join("/");
  return httpForm() ? `http://alstext.localhost/${extId}/${p}` : `alstext://${extId}/${p}`;
}

/** Copy via the async clipboard API, falling back to a hidden textarea (webviews restrict the former). */
export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    if (!ok) throw new Error("clipboard is not available");
  }
}

/** The reader sets these while it is open; they default to no-ops so extensions can call them any time. */
export const readerBridge: {
  current: { workId: string; partId: string; language: string } | null;
  openPanel: (extId: string, panelId: string) => void;
  openTerm: (language: string, text: string) => void;
} = { current: null, openPanel: () => {}, openTerm: () => {} };

const ui: UiBridge = {
  toast,
  openPanel: (extId, panelId) => readerBridge.openPanel(extId, panelId),
  openTerm: (language, text) => readerBridge.openTerm(language, text),
  copyText,
  readerCurrent: () => readerBridge.current,
};

export const extHost = new ExtensionHost({
  createWorker: (extId) => {
    const w = new BootstrapWorker({ name: extId });
    let handler: ((e: { data: unknown }) => void) | null = null;
    return {
      postMessage: (m) => w.postMessage(m),
      get onmessage() {
        return handler;
      },
      set onmessage(fn) {
        handler = fn;
        w.onmessage = fn ? (e) => fn({ data: e.data }) : null;
      },
      terminate: () => w.terminate(),
    };
  },
  entryUrl: extUrl,
  extCall: api.extCall,
  ui,
  now: () => Date.now(),
});
