import { invoke } from "@tauri-apps/api/core";

export type LookupUrl = { name: string; url: string };
export type Language = { id: string; name: string; rtl: boolean; lookupUrls: LookupUrl[] };
export type Work = {
  id: string;
  language: string;
  title: string;
  kind: string;
  author: string | null;
  sourceUrl: string | null;
  createdAt: number;
  partCount: number;
};
export type PartInfo = { id: string; idx: number; title: string | null; wordCount: number };
export type Unit = {
  text: string;
  isWord: boolean;
  key: string | null;
  wordCount: number;
  status: number | null;
  termId: string | null;
  sentence: number;
  start: number;
  end: number;
};
export type PartView = { work: Work; parts: PartInfo[]; partId: string; rtl: boolean; units: Unit[] };
export type TermRef = { id: string; text: string };
export type Term = {
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
  parents: TermRef[];
};
export type DictResult = {
  dictId: number;
  dictTitle: string;
  term: string;
  reading: string;
  tags: string;
  score: number;
  glossary: string[];
};
export type TermView = { key: string; text: string; wordCount: number; term: Term | null; dict: DictResult[] };
export type TermPatch = { status?: number; translation?: string; reading?: string; notes?: string; lemma?: string };
export type DictInfo = { id: number; title: string; revision: string | null; language: string; enabled: boolean; entryCount: number };

export const STATUSES = [0, 1, 2, 3, 4, 5, 98, 99] as const;
export const statusLabel = (s: number) =>
  s === 0 ? "New" : s === 98 ? "Ignored" : s === 99 ? "Known" : `Learning ${s}`;

const bytesOf = async (file: File) => new Uint8Array(await file.arrayBuffer());

export const api = {
  languages: () => invoke<Language[]>("list_languages"),
  works: () => invoke<Work[]>("list_works"),
  parts: (workId: string) => invoke<PartInfo[]>("list_parts", { workId }),
  createWork: (language: string, title: string, text: string) =>
    invoke<Work>("create_work", { language, title, text }),
  importEpub: async (language: string, file: File) =>
    invoke<Work>("import_epub", await bytesOf(file), {
      headers: { "x-language": language, "x-filename": encodeURIComponent(file.name) },
    }),
  importUrl: (language: string, url: string) => invoke<Work>("import_url", { language, url }),
  partView: (partId: string) => invoke<PartView>("get_part_view", { partId }),
  openTerm: (language: string, text: string, logLookup: boolean) =>
    invoke<TermView>("open_term", { language, text, logLookup }),
  saveTerm: (language: string, text: string, patch: TermPatch, parents?: string[]) =>
    invoke<Term>("save_term", { language, text, patch, parents: parents ?? null }),
  finishPart: (partId: string, markKnown: boolean, durationMs: number) =>
    invoke<number>("finish_part", { partId, markKnown, durationMs }),
  dictionaries: () => invoke<DictInfo[]>("list_dictionaries"),
  importDictionary: async (language: string, file: File) =>
    invoke<DictInfo>("import_dictionary", await bytesOf(file), { headers: { "x-language": language } }),
  deleteDictionary: (id: number) => invoke<void>("delete_dictionary", { id }),
  setDictionaryEnabled: (id: number, enabled: boolean) => invoke<void>("set_dictionary_enabled", { id, enabled }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  exportTsv: (language: string, minStatus: number, maxStatus: number) =>
    invoke<{ tsv: string; count: number }>("export_tsv", { language, minStatus, maxStatus }),
  saveExport: (filename: string, content: string) => invoke<string>("save_export", { filename, content }),
};

/** Fill a lookup URL template (`###` placeholder, SPEC §7). */
export const lookupHref = (template: string, term: string) => template.replace("###", encodeURIComponent(term));
