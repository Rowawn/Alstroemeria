// Unknown-word density (SPEC §6.9.9). `decorate` runs whenever the reader's term statuses change, so it is
// used here to collect statistics (it returns no render hints); the panel asks for them with `alst.send`.

const HISTORY_LIMIT = 50;
let latest = { language: null, total: 0, unknown: 0, words: [] };

export function decorate({ language, units }) {
  const counts = new Map();
  let total = 0;
  let unknown = 0;
  for (const u of units) {
    if (!u.isWord) continue;
    total++;
    if (u.status === 0) {
      unknown++;
      counts.set(u.key, (counts.get(u.key) ?? 0) + 1);
    }
  }
  const words = [...counts].map(([key, count]) => ({ key, count })).sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
  latest = { language, total, unknown, words };
  return {};
}

export async function onEvent(event) {
  if (event.type !== "part.read" || !event.language) return;
  const key = `history:${event.language}`;
  const history = (await alst.storage.get(key)) ?? [];
  history.push({ ts: event.ts, words: event.payload.words_read, unknown: event.payload.unknown_count });
  await alst.storage.set(key, history.slice(-HISTORY_LIMIT));
}

export async function onPanelMessage(_panelId, msg) {
  if (msg?.type !== "stats") return null;
  const history = latest.language ? ((await alst.storage.get(`history:${latest.language}`)) ?? []) : [];
  return { language: latest.language, total: latest.total, unknown: latest.unknown, top: latest.words.slice(0, 10), history: history.slice(-5) };
}

export const commands = {
  async "copy-unknown"() {
    if (!latest.words.length) {
      await alst.ui.toast("No unknown words in this part.");
      return;
    }
    await alst.ui.copyText(latest.words.map((w) => w.key).join("\n"));
    await alst.ui.toast(`Copied ${latest.words.length} unknown words.`);
  },
};
