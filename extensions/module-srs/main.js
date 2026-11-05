// Spaced repetition module (SPEC §11). Everything goes through the public host API (`alst.srs.*`); the screens are in
// screen.html. This worker serves the term actions and reacts to status changes made on this device.

export const commands = {
  async "add-to-review"(a) {
    const r = await alst.srs.addTerm({ language: a.language, text: a.term });
    alst.ui.toast(r.created.length ? "Added to review" : "Already in review");
  },
  // One tap: the sentence is saved with the term's meaning (the editor saves its pending edits first) and its audio clip.
  async "mine-sentence"(a) {
    const [t] = await alst.terms.get({ language: a.language, texts: [a.term] }).catch(() => []);
    const sent = a.sentence.text;
    const at = sent.toLowerCase().indexOf(a.term.toLowerCase());
    const esc = (x) => x.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const marked = at >= 0 ? `${esc(sent.slice(0, at))}<b>${esc(sent.slice(at, at + a.term.length))}</b>${esc(sent.slice(at + a.term.length))}` : esc(sent);
    await alst.srs.mine({
      language: a.language, text: a.term, sentence: marked, target: a.term, meaning: t?.translation ?? "", sentenceMeaning: "",
      audio: a.cue ? `[sound:alstclip:${a.cue.mediaHash}:${a.cue.start}:${a.cue.end}]` : "",
      source: { workId: a.workId, partId: a.partId, start: a.sentence.start, end: a.sentence.end, cue: a.cue },
      tags: ["mined"],
    });
    alst.ui.toast("Sentence added to review");
  },
};

/** Status changes on this device create cards (automatic mode) and suspend or release them (linked mode). */
export async function onEvent(ev) {
  if (ev.type !== "term.status" || !ev.own || !ev.subjectId || !ev.payload) return;
  await alst.srs.onTermStatus({ termId: ev.subjectId, to: ev.payload.to });
}
