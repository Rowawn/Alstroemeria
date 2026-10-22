# Alstroemeria

Local-first language-immersion app (in the tradition of Lute/LingQ). Tauri 2 + Rust core + Svelte 5 UI.
Targets Windows, macOS, Linux, Android; iOS later. Peer-to-peer LAN sync. Everything is an extension.

- Architecture and decisions: `docs/SPEC.md` (numbered sections; read only the ones your task needs).
- Milestones and current scope: `docs/ROADMAP.md`. Stay within the current milestone.

## Rules
- `src-tauri/` is a thin shell; logic lives in `crates/*`, which must not depend on Tauri.
- Writes to synced tables go only through `alst-core::store` (SPEC §4.1, §4.4).
- New tables follow the sync conventions in SPEC §4.1. Schema changes are new numbered migrations; never edit an applied one.
- Extensions use only the public host API (SPEC §6). First-party modules too.
- Don't add crates, dependencies or features ahead of their milestone.
- If an implementation needs a spec change, update `docs/SPEC.md` in the same commit.
- License: core is MPL-2.0, `extensions/*` are MIT (SPEC §14). No GPL-family dependencies or copied code.

## Commands
- `npm run tauri dev` — desktop dev
- `npm run tauri android dev` — Android dev
- `cargo test --workspace` — Rust tests

## Working economically
- Prefer scaffolding tools (`create-tauri-app`, `cargo new`) over hand-writing boilerplate.
- Run tests instead of re-reading code to check work.
- Keep sessions to one milestone step; summarize state in the roadmap before ending.
