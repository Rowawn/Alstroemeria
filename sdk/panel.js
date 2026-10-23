// Panel SDK (MIT). Include in a panel page: <script type="module" src="./panel.js"></script>
//
// Panels run in a sandboxed <iframe> with no network. This script talks to the host with postMessage:
//   import { alst } from "./panel.js";
//   const stats = await alst.send({ type: "stats" });          // → extension's onPanelMessage(panelId, msg)
//   const terms = await alst.call("terms.get", { language, texts: ["hola"] });   // any permitted alst.* call
//   alst.on("refresh", () => ...);                             // the reader's data changed
// The host also sends the app's theme, applied here as CSS variables on <html> (--bg, --fg, --accent, --role-*, ...).

let nextId = 1;
const pending = new Map();
const listeners = new Map();

window.addEventListener("message", (e) => {
  const m = e.data;
  if (!m || typeof m !== "object") return;
  if (m.type === "alst-result") {
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    m.error ? p.reject(new Error(m.error.message ?? "error")) : p.resolve(m.result);
  } else if (m.type === "alst-theme") {
    for (const [name, value] of Object.entries(m.cssVariables ?? {})) {
      document.documentElement.style.setProperty(name, String(value));
    }
    document.documentElement.dataset.alstTheme = "ready";
  } else if (m.type === "alst-event") {
    for (const cb of listeners.get(m.name) ?? []) cb();
  }
});

function request(method, params) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    parent.postMessage({ type: "alst-call", id, method, params }, "*");
  });
}

export const alst = {
  /** Call a host API method by name without the `alst.` prefix, e.g. `call("storage.get", { key })`. */
  call: request,
  /** Send a message to the extension worker's `onPanelMessage` and get its reply. */
  send: (msg) => request("panel.message", msg),
  /** Subscribe to host notifications; currently `"refresh"`. */
  on(name, cb) {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(cb);
  },
  ui: {
    openTerm: (language, text) => request("ui.openTerm", { language, text }),
    toast: (message, opts) => request("ui.toast", { message, kind: opts?.kind }),
    copyText: (text) => request("ui.copyText", { text }),
  },
};

parent.postMessage({ type: "alst-ready" }, "*");
