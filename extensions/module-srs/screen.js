// The SPA behind the module's "Review" tab (SPEC §11.9): decks and review, browse, stats, files, settings, and the
// sentence-mining dialog. It talks to the host only through `alst.srs.*` (panel.js).
import { alst } from "./panel.js";

const app = document.getElementById("app");
const tz = -new Date().getTimezoneOffset();
const $ = (tag, props = {}, ...kids) => {
  const { on, ...rest } = props;
  const e = Object.assign(document.createElement(tag), rest);
  for (const [k, v] of Object.entries(on ?? {})) e.addEventListener(k, v);
  e.append(...kids.flat().filter((k) => k != null && k !== false));
  return e;
};
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const srs = alst.srs;
const call = (p) => p.catch((e) => { state.error = e.message; render(); throw e; });

const state = { view: "decks", error: "", decks: null, session: null, browse: null, notice: "" };

function fmtIn(ms) {
  const m = ms / 60000;
  if (m < 1) return "<1m";
  if (m < 60) return `${Math.round(m)}m`;
  if (m < 60 * 24) return `${Math.round(m / 60)}h`;
  const d = m / 1440;
  return d < 30 ? `${Math.round(d)}d` : d < 365 ? `${(d / 30).toFixed(1)}mo` : `${(d / 365).toFixed(1)}y`;
}

function nav() {
  const tabs = [["decks", "Decks"], ["browse", "Browse"], ["stats", "Stats"], ["files", "Import / export"], ["settings", "Settings"]];
  return $("nav", {}, ...tabs.map(([id, title]) => $("button", { className: state.view === id || (id === "decks" && state.view === "review") ? "on" : "", on: { click: () => go(id) } }, title)));
}

function go(view) {
  state.view = view;
  state.error = "";
  state.notice = "";
  if (view !== "review") state.session = null;
  render();
}

async function render() {
  const body = { decks: viewDecks, review: viewReview, browse: viewBrowse, stats: viewStats, files: viewFiles, settings: viewSettings }[state.view];
  const content = await body();
  app.replaceChildren(...[nav(), state.error ? $("p", { className: "error" }, state.error) : null, state.notice ? $("p", {}, state.notice) : null, state.editor ? state.editorBox : null, content].filter(Boolean));
}

// ---- decks -------------------------------------------------------------------------------------

async function viewDecks() {
  state.decks = await srs.decks({ tz });
  if (!state.decks.length) return $("p", { className: "muted" }, "No decks yet. Add a word to review from the reader, or import an Anki deck under Import / export.");
  const act = (fn) => async (e) => { e.stopPropagation(); try { await fn(); } catch (err) { state.error = err.message; } render(); };
  return $("table", {},
    $("tr", {}, $("th", {}, "Deck"), $("th", { className: "n" }, "New"), $("th", { className: "n" }, "Learn"), $("th", { className: "n" }, "Due"), $("th", {})),
    ...state.decks.map((d) => {
      const off = d.total > 0 && d.suspended >= d.total;
      return $("tr", { className: "click", on: { click: () => startReview(d) } },
        $("td", {}, d.name, d.kind === "concept" ? $("small", { className: "muted" }, " · words") : "", off ? $("small", { className: "muted" }, " · suspended") : ""),
        $("td", { className: "n new" }, d.new), $("td", { className: "n lrn" }, d.learning), $("td", { className: "n due" }, d.review),
        $("td", { className: "acts" }, ...(state.confirmDelete === d.id
          ? [$("span", { className: "muted" }, "Delete? "),
             $("button", { className: "danger", on: { click: act(async () => { state.confirmDelete = null; await srs.deleteDeck({ deckId: d.id }); }) } }, "Yes, delete"),
             $("button", { on: { click: act(async () => { state.confirmDelete = null; }) } }, "Cancel")]
          : [$("button", { title: off ? "Put the deck's cards back in review" : "Keep the deck but stop reviewing it", on: { click: act(() => srs.suspendDeck({ deckId: d.id, suspended: !off })) } }, off ? "Unsuspend" : "Suspend"),
             $("button", { title: "Delete the deck and its cards", on: { click: act(async () => { state.confirmDelete = d.id; }) } }, "Delete")])));
    }));
}

// ---- review ------------------------------------------------------------------------------------

async function startReview(deck) {
  state.session = { deck, card: null, shown: false, last: null, shownAt: 0, done: false };
  state.view = "review";
  await nextCard();
}

async function nextCard() {
  const s = state.session;
  const q = await call(srs.queue({ deckId: s.deck.id, limit: 1, tz }));
  if (!q.length) {
    s.card = null;
    s.done = true;
  } else {
    s.card = await call(srs.card({ cardId: q[0].cardId, tz }));
    s.done = false;
  }
  s.shown = false;
  s.shownAt = Date.now();
  render();
  // Anki's autoplay: the question's sound plays when the card appears.
  const m = s.card?.rendered.front.match(/data-sound="([^"]+)"/);
  if (m) playSound(m[1].replace(/&amp;/g, "&"));
}

function cardDoc(card, side) {
  const r = card.rendered;
  const fix = (html) => html.replace(/src="([^"]+)"/g, (m, n) => (card.media[n] ? `src="${alst.mediaUrl(card.media[n])}"` : m));
  const cs = getComputedStyle(document.documentElement);
  const vars = ["fg", "bg", "muted", "panel", "border", "accent"].map((n) => `--${n}:${cs.getPropertyValue(`--${n}`)}`).join(";");
  const html = fix(side === "front" ? r.front : r.back);
  return `<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><style>
:root{${vars}}html,body{margin:0}body{min-height:100vh;box-sizing:border-box;display:flex;flex-direction:column;padding:14px;background:var(--panel);color:var(--fg);font:20px system-ui,sans-serif;text-align:center}
img{max-width:100%}hr{border:0;border-top:1px solid var(--border)}.alst-actions{display:grid;gap:8px;margin-top:auto;padding-top:18px}.alst-btn{display:block;box-sizing:border-box;width:100%;text-align:center;text-decoration:none;color:var(--fg);background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:10px 12px;font:15px system-ui,sans-serif}${r.css}</style>
<div class="card">${html}</div><script>
(function(){var links=[].slice.call(document.querySelectorAll('a.alst-sound,a.alst-open'));if(!links.length)return;
var bar=document.createElement('div');bar.className='alst-actions';var n=0;
links.forEach(function(a){if(a.classList.contains('alst-sound')){n++;a.textContent='\u25B6 Play audio'+(n>1?' '+n:'')}a.classList.add('alst-btn');bar.appendChild(a)});
document.body.appendChild(bar)})();
document.addEventListener('click',function(e){var a=e.target.closest('a');if(!a)return;
if(a.classList.contains('alst-sound')){e.preventDefault();parent.postMessage({type:'alst-card',action:'sound',name:a.dataset.sound},'*')}
else if(a.classList.contains('alst-open')){e.preventDefault();parent.postMessage({type:'alst-card',action:'open',part:a.dataset.part,start:+a.dataset.start},'*')}
else if(a.getAttribute('href')==='#'&&!a.onclick){e.preventDefault()}});
document.addEventListener('keydown',function(e){parent.postMessage({type:'alst-card',action:'key',key:e.key},'*')});
<\/script>`;
}

let cardFrame = null;
window.addEventListener("message", (e) => {
  const m = e.data;
  if (!cardFrame || e.source !== cardFrame.contentWindow || !m || m.type !== "alst-card") return;
  if (m.action === "sound") playSound(String(m.name));
  else if (m.action === "key") handleKey(String(m.key));
  else if (m.action === "open") alst.ui.openPart(String(m.part), Number(m.start) || 0);
});

let audio = null;
function playSound(name) {
  const card = state.session?.card;
  if (!card) return;
  let url;
  const clip = name.match(/^alstclip:([0-9a-f]{64}):(\d+):(\d+)$/);
  if (clip) url = `${alst.mediaUrl(clip[1])}#t=${clip[2] / 1000},${clip[3] / 1000}`;
  else if (card.media[name]) url = alst.mediaUrl(card.media[name]);
  if (!url) return;
  audio?.pause();
  audio = new Audio(url);
  audio.play().catch(() => {});
}

function viewReview() {
  const s = state.session;
  if (!s) return $("p", {}, "No session.");
  const back = $("button", { on: { click: () => go("decks") } }, "← Decks");
  if (s.done) {
    return $("div", {}, back, $("h3", {}, "All done for now"), $("p", { className: "muted" }, "No more cards are due in this deck today."),
      s.last ? $("button", { on: { click: undo } }, "Undo last answer") : null);
  }
  const c = s.card;
  if (!c) return $("p", { className: "muted" }, "Loading…");
  const frame = $("iframe", { className: "card-frame", title: "Card" });
  frame.setAttribute("sandbox", "allow-scripts");
  frame.srcdoc = cardDoc(c, s.shown ? "back" : "front");
  // The card document must not keep the keyboard: Space and 1-4 are handled by this screen.
  frame.addEventListener("load", () => window.focus());
  cardFrame = frame;
  // On the answer side the sound plays when it is there (Anki's autoplay).
  const bar = $("div", { className: "row" }, back, $("span", { className: "grow muted" }, `${c.rendered.deck} · ${c.state.queue}`),
    $("button", { on: { click: undo }, disabled: !s.last, title: "Undo (U)" }, "Undo"),
    $("button", { on: { click: editCard }, disabled: !c.rendered.noteId, title: "Edit note (E)" }, "Edit"),
    $("button", { on: { click: toggleSuspend }, title: "Suspend (S)" }, c.suspended ? "Unsuspend" : "Suspend"));
  const controls = s.shown
    ? $("div", { className: "rate" }, ...c.next.map((n, i) => $("button", { className: i === 2 ? "primary" : "", on: { click: () => answer(n.rating) } }, $("span", {}, ["Again", "Hard", "Good", "Easy"][i] + ` (${i + 1})`), $("small", {}, fmtIn(n.inMs)))))
    : $("button", { className: "show primary", on: { click: showAnswer } }, "Show answer (Space)");
  return $("div", {}, bar, frame, controls);
}

function showAnswer() {
  const s = state.session;
  if (!s || s.shown) return;
  s.shown = true;
  render();
  // The sentence's audio plays with the answer.
  const m = s.card.rendered.back.match(/data-sound="([^"]+)"/);
  if (m) playSound(m[1].replace(/&amp;/g, "&"));
}

async function answer(rating) {
  const s = state.session;
  const c = s.card;
  const ex = c.rendered.example;
  await call(srs.answer({ cardId: c.cardId, rating, durationMs: Date.now() - s.shownAt, example: ex ? { partId: ex.partId, start: ex.start } : undefined, tz }));
  s.last = c.cardId;
  await nextCard();
}

async function undo() {
  const s = state.session;
  if (!s?.last) return;
  const id = s.last;
  const r = await call(srs.undo({ cardId: id }));
  s.last = null;
  if (!r.undone) return nextCard();
  // Show the card again, front side.
  s.card = await call(srs.card({ cardId: id, tz }));
  s.done = false;
  s.shown = false;
  s.shownAt = Date.now();
  render();
}

async function toggleSuspend() {
  const c = state.session.card;
  await call(srs.suspend({ cardIds: [c.cardId], suspended: !c.suspended }));
  await nextCard();
}

async function editCard() {
  const c = state.session.card;
  if (!c.rendered.noteId) return;
  await openEditor(c.rendered.noteId, async () => { state.session.card = await srs.card({ cardId: c.cardId, tz }); });
}

async function openEditor(noteId, after) {
  const n = await call(srs.note({ noteId }));
  state.editor = { n, after };
  const box = $("div", { className: "dialog" }, $("strong", {}, `Edit ${n.noteType}`),
    ...n.fieldNames.map((name, i) => $("label", {}, name, $("textarea", { rows: 2, value: n.fields[i] ?? "", id: `f${i}` }))),
    $("label", {}, "Tags (space separated)", $("input", { type: "text", id: "tags", value: n.tags.join(" ") })),
    $("div", { className: "row" },
      $("button", { className: "primary", on: { click: async () => {
        const fields = n.fieldNames.map((_, i) => document.getElementById(`f${i}`).value);
        const tags = document.getElementById("tags").value.split(/\s+/).filter(Boolean);
        await call(srs.editNote({ noteId, fields, tags }));
        state.editor = null; await after?.(); render();
      } } }, "Save"),
      $("button", { on: { click: () => { state.editor = null; render(); } } }, "Cancel")));
  state.editorBox = box;
  render();
}

// ---- browse ------------------------------------------------------------------------------------

async function viewBrowse() {
  const b = (state.browse ??= { text: "", deckId: "", state: "", rows: null, picked: new Set() });
  const decks = state.decks ?? (state.decks = await srs.decks({ tz }));
  b.rows = await srs.browse({ text: b.text, deckId: b.deckId || undefined, state: b.state || undefined, limit: 200 });
  const refresh = () => render();
  const act = (fn) => async () => { if (!b.picked.size) return; await call(fn([...b.picked])); b.picked.clear(); refresh(); };
  const moveSel = $("select", {}, $("option", { value: "" }, "Move to…"), ...decks.map((d) => $("option", { value: d.id }, d.name)));
  return $("div", {},
    $("div", { className: "row" },
      $("input", { type: "search", className: "grow", placeholder: "Search cards", value: b.text, on: { change: (e) => { b.text = e.target.value; refresh(); } } }),
      $("select", { on: { change: (e) => { b.deckId = e.target.value; refresh(); } } }, $("option", { value: "" }, "All decks"), ...decks.map((d) => $("option", { value: d.id, selected: d.id === b.deckId }, d.name))),
      $("select", { on: { change: (e) => { b.state = e.target.value; refresh(); } } }, ...[["", "Any state"], ["new", "New"], ["learning", "Learning"], ["review", "Review"], ["relearning", "Relearning"]].map(([v, t]) => $("option", { value: v, selected: v === b.state }, t)))),
    $("div", { className: "row" },
      $("button", { on: { click: () => { const all = b.picked.size < b.rows.length; b.picked.clear(); if (all) b.rows.forEach((r) => b.picked.add(r.cardId)); refresh(); } } }, b.picked.size < b.rows.length ? "Select all" : "Select none"),
      $("button", { on: { click: act((cardIds) => srs.suspend({ cardIds, suspended: true })) } }, "Suspend"),
      $("button", { on: { click: act((cardIds) => srs.suspend({ cardIds, suspended: false })) } }, "Unsuspend"),
      moveSel, $("button", { on: { click: act((cardIds) => srs.moveCards({ cardIds, deckId: moveSel.value })) } }, "Move"),
      ...(b.confirm
        ? [$("button", { className: "danger", on: { click: async () => { b.confirm = false; await act((cardIds) => srs.deleteCards({ cardIds }))(); } } }, `Yes, delete ${b.picked.size}`),
           $("button", { on: { click: () => { b.confirm = false; refresh(); } } }, "Cancel")]
        : [$("button", { on: { click: () => { if (b.picked.size) { b.confirm = true; refresh(); } } } }, "Delete")])),
    $("p", { className: "muted" }, `${b.rows.length} card(s)`),
    $("table", {}, ...b.rows.map((r) => $("tr", {},
      $("td", {}, $("input", { type: "checkbox", checked: b.picked.has(r.cardId), on: { change: (e) => (e.target.checked ? b.picked.add(r.cardId) : b.picked.delete(r.cardId)) } })),
      $("td", { className: r.noteId ? "click" : "", on: { click: () => r.noteId && openEditor(r.noteId, refresh) } }, r.title || "(empty)", r.meaning ? $("small", { className: "muted" }, ` — ${r.meaning}`) : "", r.direction === "production" ? $("small", { className: "muted" }, " ↩") : ""),
      $("td", { className: "muted" }, r.deck), $("td", { className: "muted" }, r.suspended ? "suspended" : r.queue)))));
}

// ---- stats -------------------------------------------------------------------------------------

function chart(values, label) {
  const max = Math.max(1, ...values);
  return $("div", {}, $("small", { className: "muted" }, label),
    $("div", { className: "chart" }, ...values.map((v, i) => $("div", { title: `${v}` }, $("div", { className: "bar", style: `height:${Math.round((v / max) * 100)}%` })))));
}

async function viewStats() {
  const s = await call(srs.stats({ tz }));
  return $("div", {},
    $("h3", {}, "Cards"),
    $("p", { className: "counts" }, $("span", {}, `New ${s.cards.new}`), $("span", {}, `Learning ${s.cards.learning}`), $("span", {}, `Review ${s.cards.review}`), $("span", {}, `Suspended ${s.cards.suspended}`)),
    $("h3", {}, "True retention"),
    $("p", {}, s.retention == null ? "Not enough reviews yet." : `${(s.retention * 100).toFixed(1)}% of ${s.retentionSample} reviews of mature cards in the last 30 days were remembered.`),
    chart(s.forecast, "Due in the next 30 days"),
    chart(s.reviewsPerDay, "Reviews per day, last 30 days"));
}

// ---- import and export -------------------------------------------------------------------------

async function viewFiles() {
  const decks = state.decks ?? (state.decks = await srs.decks({ tz }));
  const picks = new Set();
  const out = $("p", { className: "muted" });
  return $("div", {},
    $("h3", {}, "Import from Anki"),
    $("p", { className: "muted" }, "Choose a .apkg or .colpkg exported from Anki with “Support older Anki versions” ticked. Notes, media and review history come across; importing the same deck again updates its notes and keeps your reviews."),
    $("button", { className: "primary", on: { click: async () => {
      const f = await alst.ui.pickFile(".apkg,.colpkg");
      if (!f) return;
      state.notice = `Importing ${f.name}…`; render();
      try {
        const r = await srs.importApkg({ uploadId: f.uploadId, tz });
        state.decks = null;
        state.notice = `Imported ${r.notesNew} new and ${r.notesUpdated} updated notes, ${r.cards} cards, ${r.reviews} reviews, ${r.media} media files.`;
      } catch (e) { state.notice = ""; state.error = e.message; }
      render();
    } } }, "Choose file…"),
    $("h3", {}, "Export to Anki"),
    $("p", { className: "muted" }, "Writes a .apkg Anki can import. Word decks (concept cards) become plain notes, one per word and direction, with the best example sentence; this loses the rotating sentences and per-concept scheduling."),
    ...decks.map((d) => $("label", { className: "inline" }, $("input", { type: "checkbox", on: { change: (e) => (e.target.checked ? picks.add(d.id) : picks.delete(d.id)) } }), `${d.name} (${d.total})`)),
    $("button", { on: { click: async () => {
      if (!picks.size) return;
      try { const r = await srs.exportApkg({ deckIds: [...picks], tz }); state.notice = `Saved ${r.report.cards} cards to ${r.path}`; state.error = ""; }
      catch (e) { state.error = e.message; }
      render();
    } } }, "Export selected decks"), out);
}

// ---- settings ----------------------------------------------------------------------------------

async function viewSettings() {
  const s = await srs.settings();
  const decks = state.decks ?? (state.decks = await srs.decks({ tz }));
  const set = (key) => async (e) => { await call(srs.setSetting({ key, value: e.target.type === "checkbox" ? e.target.checked : e.target.value })); render(); };
  const langs = [...new Set(decks.map((d) => d.language).filter(Boolean))];
  const lang = $("select", {}, ...langs.map((l) => $("option", { value: l }, l)));
  const counter = $("span", { className: "muted" });
  const refreshCount = async () => { counter.textContent = lang.value ? `${(await srs.learningCount({ language: lang.value })).count} learning words have no card.` : ""; };
  lang.addEventListener("change", refreshCount);
  setTimeout(refreshCount, 0);
  const cfgBox = $("div", {});
  const deckSel = $("select", {}, $("option", { value: "" }, "Deck options…"), ...decks.filter((d) => d.configId).map((d) => $("option", { value: d.id }, d.name)));
  deckSel.addEventListener("change", async () => {
    if (!deckSel.value) return cfgBox.replaceChildren();
    const c = await call(srs.deckConfig({ deckId: deckSel.value }));
    const num = (id, label, v, step = 1) => $("label", {}, label, $("input", { type: "number", id, value: v, step }));
    cfgBox.replaceChildren($("div", { className: "dialog" },
      num("c-new", "New cards per day", c.newPerDay), num("c-rev", "Reviews per day", c.reviewsPerDay),
      num("c-ret", "Desired retention (0.70–0.99)", c.desiredRetention, 0.01),
      $("label", {}, "Learning steps", $("input", { type: "text", id: "c-learn", value: c.learningSteps.join(" ") })),
      $("label", {}, "Relearning steps", $("input", { type: "text", id: "c-relearn", value: c.relearningSteps.join(" ") })),
      num("c-max", "Maximum interval (days)", c.maxIntervalDays), num("c-hour", "Day starts at (hour)", c.dayStartsHour),
      $("label", { className: "inline" }, $("input", { type: "checkbox", id: "c-bury", checked: c.burySiblings }), "Bury siblings until the next day"),
      $("button", { className: "primary", on: { click: async () => {
        const v = (id) => document.getElementById(id);
        try {
          await srs.setDeckConfig({ configId: c.id, newPerDay: +v("c-new").value, reviewsPerDay: +v("c-rev").value, desiredRetention: +v("c-ret").value,
            learningSteps: v("c-learn").value.split(/\s+/).filter(Boolean), relearningSteps: v("c-relearn").value.split(/\s+/).filter(Boolean),
            maxIntervalDays: +v("c-max").value, dayStartsHour: +v("c-hour").value, burySiblings: v("c-bury").checked });
          state.notice = "Saved."; state.error = "";
        } catch (e) { state.error = e.message; }
        render();
      } } }, "Save options")));
  });
  const radio = (key, cur, options) => options.map(([v, label]) => $("label", { className: "inline" }, $("input", { type: "radio", name: key, value: v, checked: cur === v, on: { change: set(key) } }), label));
  return $("div", {},
    $("h3", {}, "Making cards"),
    ...radio("srs.cardCreation", s.cardCreation, [["opt-in", "Only when I add a word (“Add to review”)"], ["automatic", "Automatically when a word moves to status 1–5"]]),
    $("label", { className: "inline" }, $("input", { type: "checkbox", checked: s.productionCards, on: { change: set("srs.productionCards") } }), "Also make production cards (meaning → word)"),
    $("p", { className: "muted" }, "New word cards are:"),
    ...radio("srs.defaultDeckKind", s.defaultDeckKind, [["concept", "Word cards with rotating example sentences"], ["classic", "Plain cards (word → meaning)"]]),
    $("div", { className: "row" }, lang, $("button", { on: { click: async () => { const r = await call(srs.addAllLearning({ language: lang.value })); state.notice = `Added cards for ${r.terms} words.`; render(); } } }, "Add all learning words"), counter),
    $("h3", {}, "Word status and reviews"),
    ...radio("srs.statusLink", s.statusLink, [["linked", "Linked: marking a word known or ignored suspends its cards; reviews update its status"], ["reviews-only", "Reviews update status only; changing a status never touches cards"]]),
    $("h3", {}, "Deck options"), deckSel, cfgBox);
}

// ---- keyboard ----------------------------------------------------------------------------------

function handleKey(key) {
  if (state.view !== "review" || !state.session) return;
  const s = state.session;
  if (key === " " || key === "Enter") { if (!s.shown) showAnswer(); else answer(3); }
  else if (s.shown && ["1", "2", "3", "4"].includes(key)) answer(+key);
  else if (key === "u") undo();
  else if (key === "e") editCard();
  else if (key === "s" && s.card) toggleSuspend();
}

document.addEventListener("keydown", (e) => {
  if (/INPUT|TEXTAREA|SELECT/.test(e.target.tagName) || state.view !== "review") return;
  if (e.key === " " || e.key === "Enter") e.preventDefault();
  handleKey(e.key);
});


alst.on("refresh", () => { if (state.view === "decks") render(); });

render().catch((e) => { app.textContent = e.message; });
