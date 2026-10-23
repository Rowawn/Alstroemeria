// On-device verification of the media features (roadmap M4). Runs when the `selftest` file in the app data folder
// starts with `media`: `media <feed url> [<youtube url>]`. It drives the real webview: chunked upload, the
// alstmedia:// protocol (through <audio>/<video>, which need Range support to seek), codec support, the cue-synced
// reader (DOM), podcast import and yt-dlp. Results go to the app log through `selftest_report`. Not part of normal use.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { api, mediaUrl } from "./api";
import type { CoreEvent } from "./ext/types";

const report = (name: string, ok: boolean, detail = "") => invoke("selftest_report", { name, ok, detail: String(detail).slice(0, 700) });

async function check(name: string, fn: () => Promise<string | void>) {
  try {
    await report(name, true, (await fn()) ?? "");
  } catch (e) {
    await report(name, false, e instanceof Error ? e.message : String(e));
  }
}

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(what: string, fn: () => T | null | undefined | false, ms = 8000): Promise<T> {
  const t0 = Date.now();
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

/** 16-bit mono PCM WAV of a quiet sine; ~3 MB for 35 s so the upload needs several chunks. */
function makeWav(seconds: number, rate = 44100): File {
  const n = seconds * rate;
  const buf = new ArrayBuffer(44 + n * 2);
  const v = new DataView(buf);
  const str = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF");
  v.setUint32(4, 36 + n * 2, true);
  str(8, "WAVEfmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  str(36, "data");
  v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) v.setInt16(44 + i * 2, Math.round(Math.sin((2 * Math.PI * 330 * i) / rate) * 800), true);
  return new File([buf], "tono.wav", { type: "audio/wav" });
}

const stamp = (ms: number) => {
  const s = Math.floor(ms / 1000);
  const pad = (x: number, w = 2) => String(x).padStart(w, "0");
  return `00:${pad(Math.floor(s / 60))}:${pad(s % 60)},${pad(ms % 1000, 3)}`;
};

/** 17 cues of 2 s: "Frase número N." */
function makeSrt(): File {
  const out = [];
  for (let i = 0; i < 17; i++) out.push(`${i + 1}\n${stamp(i * 2000)} --> ${stamp(i * 2000 + 1900)}\nFrase número ${i + 1}.\n`);
  return new File([out.join("\n")], "tono.srt", { type: "application/x-subrip" });
}

const LRC = "[ti:Canción de prueba]\n[ar:Ana]\n[00:00.00]Uno dos\n[00:02.00]Tres cuatro\n[00:04.00]Cinco seis\n";

const cueEl = (k: number) => document.querySelector<HTMLElement>(`[data-cue="${k}"]`);
const curCue = () => {
  const el = document.querySelector<HTMLElement>(".cue.cur");
  return el ? Number(el.dataset.cue) : -1;
};
const player = () => document.querySelector<HTMLMediaElement>("audio, video");
const button = (text: string) => [...document.querySelectorAll("button")].find((b) => b.textContent?.includes(text)) as HTMLButtonElement | undefined;

/** Load a media element from the media store and report what the webview can do with it. */
async function probe(hash: string, video: boolean): Promise<string> {
  const el = document.createElement(video ? "video" : "audio");
  el.muted = true;
  el.preload = "auto";
  el.src = mediaUrl(hash);
  document.body.append(el);
  try {
    const failed = new Promise<never>((_, rej) => (el.onerror = () => rej(new Error(`media error code ${el.error?.code}: ${el.error?.message}`))));
    await Promise.race([until("metadata", () => el.readyState >= 1 && Number.isFinite(el.duration), 10000), failed]);
    await Promise.race([el.play(), failed]);
    const t0 = el.currentTime;
    await sleep(900);
    assert(el.currentTime > t0 + 0.3, `did not advance (t=${el.currentTime})`);
    el.pause();
    return `duration ${el.duration.toFixed(2)}s, advanced ${(el.currentTime - t0).toFixed(2)}s`;
  } finally {
    el.remove();
  }
}

export async function run(
  languages: { id: string }[],
  arg: string,
  ui: { openPart(id: string): void; closePart(): void },
) {
  const [, feedUrl, ytUrl] = arg.split(/\s+/);
  await report("media-selftest-start", true, navigator.userAgent);
  if (!languages.some((l) => l.id === "es")) return report("languages", false, "no Spanish language");

  const events: CoreEvent[] = [];
  const unlisten = await listen<CoreEvent>("alst:event", (e) => events.push(e.payload));

  await check("codec support (canPlayType)", async () => {
    const a = document.createElement("audio");
    const v = document.createElement("video");
    const types = [
      ["mp3", a, "audio/mpeg"], ["aac/m4a", a, 'audio/mp4; codecs="mp4a.40.2"'], ["ogg vorbis", a, 'audio/ogg; codecs="vorbis"'],
      ["opus", a, 'audio/ogg; codecs="opus"'], ["wav", a, "audio/wav"], ["flac", a, "audio/flac"], ["webm audio", a, 'audio/webm; codecs="opus"'],
      ["mp4 h264", v, 'video/mp4; codecs="avc1.42E01E"'], ["mp4 h264+aac", v, 'video/mp4; codecs="avc1.42E01E, mp4a.40.2"'],
      ["webm vp9", v, 'video/webm; codecs="vp9"'], ["webm vp8", v, 'video/webm; codecs="vp8"'], ["mp4 hevc", v, 'video/mp4; codecs="hvc1.1.6.L93.B0"'],
      ["mkv", v, "video/x-matroska"], ["mov", v, "video/quicktime"],
    ] as const;
    return types.map(([n, el, t]) => `${n}=${el.canPlayType(t) || "no"}`).join(" ");
  });

  let wavPart = "";
  let wavHash = "";
  await check("import WAV + SRT through chunked upload", async () => {
    const wav = makeWav(35);
    const steps: number[] = [];
    const w = await api.importMedia("es", "Tono de prueba", wav, makeSrt(), (f) => steps.push(f));
    assert(steps.length >= 3, `expected several chunks, got ${steps.length} (size ${wav.size})`);
    assert(w.kind === "audio" && w.partCount === 1, `kind ${w.kind}, parts ${w.partCount}`);
    wavPart = (await api.parts(w.id))[0].id;
    const v = await api.partView(wavPart);
    assert(v.media?.present && v.media.mime === "audio/wav" && !v.media.video, JSON.stringify(v.media));
    assert(v.timing?.cues.length === 17, `cues ${v.timing?.cues.length}`);
    // Cue offsets are UTF-16 offsets into the text the units reassemble.
    const text = v.units.map((u) => u.text).join("");
    const c = v.timing.cues[5];
    assert(text.slice(c.a, c.b) === "Frase número 6.", `cue 5 text '${text.slice(c.a, c.b)}'`);
    wavHash = v.media.hash;
    return `${wav.size} bytes in ${steps.length} chunks, hash ${wavHash.slice(0, 12)}`;
  });

  await check("same file again is deduplicated; LRC import is lyrics", async () => {
    const w = await api.importMedia("es", "", makeWav(35), new File([LRC], "cancion.lrc"));
    assert(w.kind === "lyrics" && w.title === "Canción de prueba" && w.author === "Ana", `${w.kind} '${w.title}' ${w.author}`);
    const v = await api.partView((await api.parts(w.id))[0].id);
    assert(v.media?.hash === wavHash, "media hash differs for identical content");
  });

  await check("bad imports are rejected", async () => {
    const e1 = await api.importMedia("es", "x", new File(["not audio"], "a.txt", { type: "text/plain" }), makeSrt()).then(() => "", (e) => String(e));
    assert(e1.includes("not audio or video"), `non-audio: '${e1}'`);
    const e2 = await api.importMedia("es", "x", makeWav(1), new File(["just words"], "a.srt")).then(() => "", (e) => String(e));
    assert(e2.includes("no cues"), `bad subtitle: '${e2}'`);
    return `${e1} | ${e2}`;
  });

  await check("media element: metadata, seek (Range) and playback from alstmedia://", async () => {
    const r = await probe(wavHash, false);
    const el = document.createElement("audio");
    el.muted = true;
    el.src = mediaUrl(wavHash);
    await until("metadata", () => el.readyState >= 1, 8000);
    assert(Math.abs(el.duration - 35) < 0.5, `duration ${el.duration}`);
    el.currentTime = 20;
    await until("seeked to 20 s", () => Math.abs(el.currentTime - 20) < 0.2 && !el.seeking, 5000);
    return `${r}; duration ${el.duration.toFixed(1)}; seek ok`;
  });

  await check("unknown and malformed media ids are errors, not crashes", async () => {
    for (const id of ["0".repeat(64), "not-a-hash"]) {
      const el = document.createElement("audio");
      el.src = mediaUrl(id);
      await until(`error for ${id.slice(0, 6)}`, () => el.error, 5000);
    }
  });

  await check("reader: cue highlight, click-to-seek, repeat, prev/next, speed, word click", async () => {
    ui.openPart(wavPart);
    const p = await until("audio element", () => player() as HTMLMediaElement | null, 8000);
    await until("cue rows", () => cueEl(16), 5000);
    p.muted = true;
    // Play from cue 3 with its play button: current cue is 3, then 4 two seconds later.
    cueEl(3)!.querySelector<HTMLButtonElement>(".cue-play")!.click();
    await until("playing at cue 3", () => !p.paused && Math.abs(p.currentTime - 6) < 1.2, 5000);
    await until("cue 3 highlighted", () => curCue() === 3, 3000);
    await until("cue 4 highlighted as time passes", () => curCue() === 4, 4000);
    // Clicking blank area of a cue row seeks (and plays).
    cueEl(8)!.click();
    await until("seek to cue 8", () => Math.abs(p.currentTime - 16) < 1.2, 3000);
    await until("cue 8 highlighted", () => curCue() === 8, 3000);
    // Clicking a word opens the term editor and does not move the player.
    const before = p.currentTime;
    cueEl(8)!.querySelector<HTMLElement>(".w")!.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    await until("term editor", () => document.querySelector(".editor"), 3000);
    assert(Math.abs(p.currentTime - before) < 1, "word click moved the player");
    // Prev / next.
    button("⏭")!.click();
    await until("next cue", () => curCue() === 9, 3000);
    button("⏮")!.click();
    await until("previous cue", () => curCue() === 8, 3000);
    // Repeat: stay on cue 10 for longer than its 2 s.
    cueEl(10)!.click();
    await until("at cue 10", () => curCue() === 10, 3000);
    button("Repeat sentence")!.click();
    let loops = 0;
    let last = p.currentTime;
    const t0 = Date.now();
    while (Date.now() - t0 < 5200) {
      await sleep(40);
      if (p.currentTime < last - 0.5) loops++;
      last = p.currentTime;
      assert(p.currentTime < 22.3 && p.currentTime > 19.7, `left the repeated cue (t=${p.currentTime.toFixed(2)})`);
    }
    assert(loops >= 2, `expected the cue to loop at least twice in 5 s, looped ${loops}`);
    assert(curCue() === 10, `current cue ${curCue()}`);
    button("Repeat sentence")!.click();
    await until("repeat off: playback continues past cue 10", () => curCue() >= 11, 5000);
    // Speed.
    const speed = document.querySelector<HTMLSelectElement>(".controls select")!;
    speed.value = "1.5";
    speed.dispatchEvent(new Event("change", { bubbles: true }));
    await until("playbackRate 1.5", () => p.playbackRate === 1.5, 2000);
    p.pause();
    return `loops=${loops}`;
  });

  await check("leaving the reader logs media.listen", async () => {
    await sleep(300);
    ui.closePart();
    const ev = await until("media.listen event", () => events.find((e) => e.type === "media.listen"), 5000);
    const seconds = (ev.payload as { seconds: number }).seconds;
    assert(seconds >= 5, `listened ${seconds}s`);
    return `${seconds}s`;
  });

  if (feedUrl) {
    let feed!: Awaited<ReturnType<typeof api.podcastFeed>>;
    await check("podcast: feed lists episodes and transcripts", async () => {
      feed = await api.podcastFeed(feedUrl);
      assert(feed.episodes.length === 4 && feed.title === "Charlas de prueba", `${feed.title}/${feed.episodes.length}`);
      assert(feed.episodes[0].transcripts.length === 1 && feed.episodes[3].transcripts.length === 0, "transcript refs");
    });
    for (const [guid, video, mime] of [["ep1", false, "audio/mp4"], ["ep2", true, "video/mp4"], ["ep3", false, "audio/wav"]] as const) {
      await check(`podcast: import ${guid} (${mime}) and play it`, async () => {
        const w = await api.podcastImport("es", feedUrl, guid);
        assert(w.author === "Charlas de prueba", `author ${w.author}`);
        const v = await api.partView((await api.parts(w.id))[0].id);
        assert(v.media?.mime === mime && v.media.video === video && v.media.present, JSON.stringify(v.media));
        assert((v.timing?.cues.length ?? 0) >= 2, "cues");
        return `${w.kind}, ${v.timing!.cues.length} cues; ${await probe(v.media.hash, video)}`;
      });
    }
    await check("podcast: episode without transcript is refused", async () => {
      const e = await api.podcastImport("es", feedUrl, "ep4").then(() => "", (x) => String(x));
      assert(e.includes("no transcript"), `got '${e}'`);
    });
  }

  await check("yt-dlp status (hidden on mobile, found on desktop)", async () => {
    const s = await api.ytdlpStatus();
    const mobile = /Android|iPhone|iPad/i.test(navigator.userAgent);
    assert(!(mobile && s.available), "must be unavailable on mobile");
    if (ytUrl && s.available) {
      const w = await api.importYoutube("es", ytUrl);
      const v = await api.partView((await api.parts(w.id))[0].id);
      assert(w.kind === "subtitle" && v.units.length > 200 && !v.media, `${w.kind} ${v.units.length} units`);
      return `available ${s.version}; imported '${w.title}' by ${w.author}, ${w.partCount} parts`;
    }
    return `available=${s.available} ${s.version ?? ""}`;
  });

  unlisten();
  await report("media-selftest-done", true);
}
