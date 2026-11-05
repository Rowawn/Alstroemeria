// Alstroemeria extension API v1.0 (SPEC §6.9.7). MIT licensed.
//
// An extension is a folder with an `extension.json` manifest and an entry module (`runtime.entry`).
// The entry runs in a dedicated Web Worker: no DOM, no direct network. Everything it can do goes through the
// global `alst` object (also passed to `activate`), and every call is checked against the manifest's permissions.

// ---- data the host hands to hooks ---------------------------------------------------------------------

export interface Token {
  /** Index within the part; stable across hooks. */
  i: number;
  text: string;
  isWord: boolean;
  /** Normalized lookup form (lowercase, language substitutions). */
  norm: string;
  lemma?: string;
  /** UTF-16 offsets into the part text. */
  start: number;
  end: number;
  /** Index of the sentence this token belongs to. */
  sentence: number;
  /** Annotations added so far, keyed by capability (`"es.morph"`). */
  ann: Record<string, unknown>;
}

/** What the reader renders: a word, a multi-word term the user saved, or non-word text. */
export interface Unit {
  text: string;
  isWord: boolean;
  /** Term key (`text_lc`) for words. */
  key: string | null;
  wordCount: number;
  /** 0 = new, 1–5 learning, 98 ignored, 99 known; null for non-words. */
  status: number | null;
  termId: string | null;
  sentence: number;
  start: number;
  end: number;
  /** First and last token index covered, inclusive. */
  tokenRange: [number, number];
}

export type Role =
  | "noun" | "verb" | "adjective" | "adverb" | "particle" | "function" | "name"
  | "accent-high" | "accent-low" | "accent-drop"
  | "accent-heiban" | "accent-atamadaka" | "accent-nakadaka" | "accent-odaka"
  | "info" | "warn"
  | "c1" | "c2" | "c3" | "c4" | "c5" | "c6";

/** Render hints are a closed vocabulary; colors are semantic roles that themes map to real colors. */
export type RenderHint =
  | { kind: "ruby"; text: string }
  /** Furigana over parts of the unit (SPEC §16.9). The `base`s must concatenate to the unit's text, otherwise the hint is ignored. */
  | { kind: "ruby"; segments: { base: string; rt?: string }[] }
  | { kind: "color"; role: Role }
  | { kind: "underline"; role: Role; style?: "solid" | "dotted" | "wavy" | "double" }
  | { kind: "badge"; text: string; role?: Role }
  | { kind: "tooltip"; text: string };

export interface Term {
  id: string;
  language: string;
  text: string;
  textLc: string;
  lemma: string | null;
  status: number;
  translation: string | null;
  reading: string | null;
  notes: string | null;
  wordCount: number;
  parents: { id: string; text: string }[];
}

export interface CoreEvent {
  id: string;
  /** Unix ms. */
  ts: number;
  /** `term.status`, `term.lookup`, `part.read`, or `<extension-id>/<type>`. */
  type: string;
  language: string | null;
  subjectType: string | null;
  subjectId: string | null;
  payload: unknown;
}

export interface DictResult {
  dictId: number;
  dictTitle: string;
  term: string;
  reading: string;
  tags: string;
  score: number;
  glossary: string[];
}

/** One row of an imported Yomitan `term_meta_bank` (SPEC §16.7). */
export interface MetaResult {
  dictId: number;
  dictTitle: string;
  term: string;
  reading: string;
  mode: "freq" | "pitch";
  /** freq: `{ value, display? }`; pitch: `{ reading, pitches: [{ position, nasal?, devoice? }] }`. */
  data: any;
}

/** What a lookup provider returns for the term editor (SPEC §16.8). Plain text only. */
export type LookupItem =
  | { kind: "text"; text: string; role?: Role }
  | { kind: "kv"; label: string; text: string }
  /** Drawn by the app as a pitch-accent graph. */
  | { kind: "pitch"; reading: string; downstep: number }
  /** https only; opens in the system browser. */
  | { kind: "link"; label: string; url: string };
export interface LookupSection {
  title: string;
  items: LookupItem[];
}
export interface LookupInput {
  language: string;
  unit: { text: string; key: string; lemma?: string; status: number };
  tokens: Token[];
  sentence: string;
}

export interface SrsDeck { id: string; name: string; kind: "classic" | "concept"; language: string | null; configId: string | null; new: number; learning: number; review: number; total: number; suspended: number }
export interface SrsCard {
  cardId: string;
  rendered: { front: string; back: string; css: string; kind: "classic" | "concept"; deck: string; noteId: string | null; example: { partId: string; start: number; end: number; mined: boolean; text: string } | null };
  state: { queue: string; due: number; stability: number | null; difficulty: number | null; step: number; reps: number; lapses: number; lastReview: number | null };
  next: { rating: number; queue: string; due: number; inMs: number }[];
  suspended: boolean;
  media: Record<string, string>;
}
export interface SrsBrowseRow { cardId: string; noteId: string | null; deckId: string; deck: string; queue: string; due: number; suspended: boolean; stability: number | null; title: string; tags: string[]; meaning: string | null; direction: string | null }
export interface SrsStats { forecast: number[]; reviewsPerDay: number[]; retention: number | null; retentionSample: number; cards: { new: number; learning: number; review: number; suspended: number } }

/** What the host passes to the command of a `contributes.termActions` entry (SPEC §11.1). */
export interface TermActionArgs {
  term: string;
  language: string;
  sentence: { text: string; start: number; end: number };
  partId: string;
  workId: string;
  cue?: { start: number; end: number; mediaHash: string };
}

// ---- the `alst` object ---------------------------------------------------------------------------------

export interface Alst {
  /** `language` is the language of the most recent hook call. */
  extension: { id: string; version: string; language: string };
  settings: {
    /** Settings validated against `contributes.settings`, with schema defaults filled in. */
    get(): Promise<Record<string, unknown>>;
  };
  /** Synced key/value store private to this extension. Values ≤ 64 KB, ≤ 1,000 keys. */
  storage: {
    get(key: string): Promise<unknown>;
    set(key: string, value: unknown): Promise<void>;
    delete(key: string): Promise<boolean>;
    keys(): Promise<string[]>;
  };
  terms: {
    /** Needs `terms:read`. */
    get(q: { language: string; ids?: string[]; texts?: string[] }): Promise<Term[]>;
    /** Needs `terms:read`. Substring match on the term or its translation; at most 100 results. */
    search(q: { language: string; query?: string; status?: number }): Promise<Term[]>;
    /** Needs `terms:write`. */
    update(id: string, patch: { status?: number; translation?: string; notes?: string; reading?: string }): Promise<Term>;
  };
  events: {
    /** The type is prefixed with your extension id: `emit("seen")` is stored as `my.ext/seen`. */
    emit(type: string, payload?: unknown): Promise<void>;
    /** Your own events always; core events need `events:read`. `type` may end in `*`. Newest first. */
    query(q?: { type?: string; since?: number; limit?: number }): Promise<CoreEvent[]>;
  };
  dict: {
    /** Needs `dict:read`. */
    lookup(language: string, query: string): Promise<DictResult[]>;
    /** Needs `dict:read`. Frequency or pitch rows for `term` (or up to 500 `terms` in one call) from imported dictionaries (SPEC §16.7). */
    meta(language: string, q: { term?: string; terms?: string[]; reading?: string; mode: "freq" | "pitch" }): Promise<MetaResult[]>;
  };
  net: {
    /** Needs `net:<host>`; HTTPS only, ≤ 10 MB, 20 s, no cookies, redirects are re-checked. */
    fetch(
      url: string,
      init?: { method?: string; headers?: Record<string, string>; body?: string },
    ): Promise<{ status: number; headers: Record<string, string>; text: string }>;
  };
  ui: {
    toast(message: string, opts?: { kind?: "info" | "error" }): Promise<void>;
    /** Open one of this extension's own panels. */
    openPanel(panelId: string): Promise<void>;
    /** Open the term editor in the reader. */
    openTerm(language: string, text: string): Promise<void>;
    copyText(text: string): Promise<void>;
    /** Show one of your `contributes.screens` over whatever is open; the screen gets `payload` as its `open` event. */
    openScreen(screenId: string, payload?: unknown): Promise<void>;
    /** Ask the user for a file; resolves with an upload id to pass to host calls such as `srs.importApkg`, or null. */
    pickFile(accept?: string): Promise<{ uploadId: string; name: string } | null>;
    /** Open a part in the reader. */
    openPart(partId: string, start?: number): Promise<void>;
  };
  /** Spaced repetition (SPEC §11). Reads need `srs:read`; changes need `srs:write` (which includes reading). */
  srs: {
    settings(): Promise<{ cardCreation: "opt-in" | "automatic"; statusLink: "linked" | "reviews-only"; productionCards: boolean; defaultDeckKind: "classic" | "concept" }>;
    decks(p?: { tz?: number }): Promise<SrsDeck[]>;
    queue(p?: { deckId?: string; limit?: number; tz?: number }): Promise<{ cardId: string; deckId: string; queue: string; due: number }[]>;
    /** The card rendered for review, its state, when each answer would bring it back, and the media it uses (name → hash). */
    card(p: { cardId: string; tz?: number }): Promise<SrsCard>;
    browse(p?: { text?: string; deckId?: string; tag?: string; state?: string; suspended?: boolean; limit?: number; offset?: number }): Promise<SrsBrowseRow[]>;
    note(p: { noteId: string }): Promise<{ id: string; noteTypeId: string; noteType: string; fieldNames: string[]; fields: string[]; tags: string[]; deckId: string; guid: string | null; cards: { id: string; ord: number; queue: string }[] }>;
    noteTypes(): Promise<{ id: string; name: string; kind: string; fields: string[] }[]>;
    stats(p?: { tz?: number }): Promise<SrsStats>;
    learningCount(p: { language: string }): Promise<{ count: number }>;
    deckConfig(p: { deckId: string }): Promise<Record<string, unknown>>;
    /** Rebuild the example sentences of this language's concept cards if the library changed. */
    refreshPool(p: { language: string }): Promise<{ rows: number | null; upToDate: boolean }>;
    // srs:write
    answer(p: { cardId: string; rating: 1 | 2 | 3 | 4; durationMs?: number; example?: { partId: string; start: number }; tz?: number }): Promise<{ reviewId: string; state: Record<string, unknown> }>;
    undo(p: { cardId: string }): Promise<{ undone: boolean }>;
    /** `termId`, or `language` + `text` (the term is found by its key, or created at status 1). */
    addTerm(p: { termId?: string; language?: string; text?: string; onlyIfAutomatic?: boolean }): Promise<{ termId: string; created: string[] }>;
    addAllLearning(p: { language: string }): Promise<{ terms: number }>;
    onTermStatus(p: { termId: string; to: number }): Promise<void>;
    mine(p: { termId?: string; language?: string; text?: string; sentence: string; target: string; meaning?: string; sentenceMeaning?: string; audio?: string; sourceLabel?: string; source?: unknown; tags?: string[]; deckId?: string }): Promise<{ noteId: string; cardIds: string[] }>;
    editNote(p: { noteId: string; fields?: string[]; tags?: string[] }): Promise<void>;
    suspend(p: { cardIds: string[]; suspended?: boolean }): Promise<void>;
    moveCards(p: { cardIds: string[]; deckId: string }): Promise<void>;
    deleteCards(p: { cardIds: string[] }): Promise<void>;
    /** Suspend or release every card of a deck that the user suspended (status-link suspensions are left alone). */
    suspendDeck(p: { deckId: string; suspended?: boolean }): Promise<void>;
    /** Delete a deck with its cards and notes. */
    deleteDeck(p: { deckId: string }): Promise<void>;
    createDeck(p: { name: string; kind?: "classic" | "concept"; language?: string }): Promise<{ id: string }>;
    setDeckConfig(p: { configId: string; newPerDay?: number; reviewsPerDay?: number; desiredRetention?: number; learningSteps?: string[]; relearningSteps?: string[]; maxIntervalDays?: number; burySiblings?: boolean; dayStartsHour?: number }): Promise<void>;
    setSetting(p: { key: "srs.cardCreation" | "srs.statusLink" | "srs.defaultDeckKind" | "srs.productionCards"; value: string | boolean }): Promise<void>;
    /** Import a legacy-layout `.apkg`/`.colpkg` the user chose with `ui.pickFile`. */
    importApkg(p: { uploadId: string; language?: string; tz?: number }): Promise<Record<string, number>>;
    /** Needs only `srs:read`. Writes the file to the user's Downloads folder (or the app's exports folder). */
    exportApkg(p: { deckIds: string[]; tz?: number }): Promise<{ path: string; report: Record<string, number> }>;
  };
  reader: {
    current(): Promise<{ workId: string; partId: string; language: string } | null>;
  };
}

declare global {
  const alst: Alst;
}

// ---- what your entry module exports -------------------------------------------------------------------
// Use named exports, or `export default { ... }` with the same members. Every member is optional.

export interface ActivateContext {
  alst: Alst;
  extension: Alst["extension"];
}

export interface ExtensionModule {
  /** Called once per worker start; 5 s limit. */
  activate?(ctx: ActivateContext): void | Promise<void>;
  /**
   * Add annotations for a part's tokens (2 s limit). Return `{ [tokenIndex]: { [capability]: value } }`;
   * only capabilities listed in your manifest's `provides` are kept. Runs once per part per set of active extensions.
   */
  annotate?(input: { language: string; tokens: Token[] }): Record<number, Record<string, unknown>> | Promise<Record<number, Record<string, unknown>>>;
  /**
   * Return render hints per unit index (2 s limit). Runs again whenever term statuses change, so it is also a
   * good place to compute statistics for a panel. Needs a `contributes.decorators` entry.
   */
  decorate?(input: { language: string; units: Unit[]; tokens: Token[] }): Record<number, RenderHint[]> | Promise<Record<number, RenderHint[]>>;
  /**
   * Sections for the term editor, asked for when it opens (2 s limit). Needs a `contributes.lookupProviders` entry.
   * Shown under the dictionary results, in decorator order.
   */
  lookup?(input: LookupInput): LookupSection[] | Promise<LookupSection[]>;
  /** Handlers for `contributes.commands`, by command id (30 s limit). */
  commands?: Record<string, (args: unknown) => unknown>;
  /** Core events (with `events:read`) and your own events. */
  onEvent?(event: CoreEvent): void | Promise<void>;
  onSettingsChanged?(settings: Record<string, unknown>): void | Promise<void>;
  /** Messages from your panels (`alst.send(msg)` in `panel.js`); the return value is the reply. */
  onPanelMessage?(panelId: string, msg: unknown): unknown;
}
