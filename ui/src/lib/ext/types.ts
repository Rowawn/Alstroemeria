// Shapes shared between the UI host, workers and the Rust side (SPEC §6.9).

export type Role =
  | "noun" | "verb" | "adjective" | "adverb" | "particle" | "function" | "name"
  | "accent-high" | "accent-low" | "accent-drop" | "info" | "warn"
  | "c1" | "c2" | "c3" | "c4" | "c5" | "c6";

export const ROLES: readonly Role[] = [
  "noun", "verb", "adjective", "adverb", "particle", "function", "name",
  "accent-high", "accent-low", "accent-drop", "info", "warn",
  "c1", "c2", "c3", "c4", "c5", "c6",
];

export type UnderlineStyle = "solid" | "dotted" | "wavy" | "double";

export type RenderHint =
  | { kind: "ruby"; text: string }
  | { kind: "color"; role: Role }
  | { kind: "underline"; role: Role; style?: UnderlineStyle }
  | { kind: "badge"; text: string; role?: Role }
  | { kind: "tooltip"; text: string };

export type Token = {
  i: number;
  text: string;
  isWord: boolean;
  norm: string;
  lemma?: string;
  start: number;
  end: number;
  sentence: number;
  ann: Record<string, unknown>;
};

export type UnitLike = {
  text: string;
  isWord: boolean;
  key: string | null;
  wordCount: number;
  status: number | null;
  termId: string | null;
  sentence: number;
  start: number;
  end: number;
  tokenRange: [number, number];
};

export type PermInfo = { id: string; text: string; warning: boolean };

export type Contributes = {
  decorators: { id: string; priority: number }[];
  panels: { id: string; title: string; entry: string; placement: string }[];
  commands: { id: string; title: string; context?: "reader" | "term" | "global" }[];
  settings?: { properties?: Record<string, SettingSchema> } | null;
};

export type SettingSchema = {
  type: "boolean" | "number" | "string";
  title?: string;
  description?: string;
  default?: unknown;
  enum?: string[];
};

export type ExtInfo = {
  id: string;
  name: string;
  version: string;
  kind: "language" | "addon" | "module";
  description: string | null;
  author: string | null;
  license: string;
  source: "bundled" | "installed";
  runtime: "declarative" | "js" | "wasm" | "native";
  entry: string | null;
  targets: string[];
  language: string | null;
  provides: string[];
  requires: string[];
  optional: string[];
  permissions: PermInfo[];
  platforms: string[] | null;
  contributes: Contributes;
  unsupported: string[];
  enabled: boolean;
  languages: string[];
  settings: Record<string, unknown>;
};

export type ProviderChoice = { capability: string; providers: string[]; chosen: string };

export type ResolvedView = {
  language: string;
  active: ExtInfo[];
  annotatorOrder: string[];
  decoratorOrder: string[];
  disabled: { id: string; name: string; reason: string }[];
  providerChoices: ProviderChoice[];
  fingerprint: string;
};

export type ExtensionList = {
  extensions: ExtInfo[];
  errors: { location: string; message: string }[];
  platform: string;
};

export type PackageInfo = ExtInfo & { existingVersion: string | null; fileCount: number };

export type CoreEvent = {
  id: string;
  ts: number;
  type: string;
  language: string | null;
  subjectType: string | null;
  subjectId: string | null;
  payload: unknown;
};

export type RpcError = { code: string; message: string };
