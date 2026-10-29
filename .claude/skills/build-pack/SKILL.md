---
name: build-pack
description: Build an Alstroemeria content pack (.alpack), a topic-based reading and listening list for one language, with little model reading. Use when asked to make, build or create a content pack, reading list or listening list for the app.
---

# build-pack

Make a `.alpack` (SPEC §10) for a language, topic and level. Credits are shared, so **you choose and describe; scripts fetch and summarise.**
Never open a page or feed yourself when a script can summarise it.

## Input
Language (code, e.g. `es`), topic, level (CEFR). Optional counts: defaults 8 articles, 4 podcast episodes, 3 videos, 0-2 texts, hard cap 25 items;
seed vocab default 40 words. Use smaller counts when the user asks (credits).

## Steps
1. **Find candidates** with web search (2-4 searches; request more candidates than you need, about 1.5x). Do not fetch result pages yourself.
2. **Check every candidate with a script, several URLs per call** (all print short JSON):
   - Articles: `node scripts/pack/probe.mjs <url> <url> ...` → `{ok, title, words, excerpt}`. Keep pages with `ok` and a word count that suits the level
     (A2: 150-500, B1: 400-1200, B2+: 700-2500). Use the excerpt, not the page, to judge level and topic.
   - Podcasts: `node scripts/pack/rss.mjs <feed-url>` → episodes `{guid, title, minutes, hasTranscript}`. Pick only episodes with `hasTranscript` and `hasAudio`.
     If no feed has transcripts, say so and drop the podcasts rather than padding.
   - Videos: `node scripts/pack/yt.mjs <lang> <url> ...` → `{title, minutes, subs}`. Use `manual` subtitles; `auto` only if nothing else.
3. **Write a draft** to the scratchpad (not the repo) as JSON:
   ```json
   { "title": "Spanish: automotive vocabulary", "language": "es", "topic": "automotive", "level": "B1", "description": "…",
     "items": [
       { "kind": "article", "title": "…", "tags": ["engine"], "source": { "url": "https://…", "license": "unknown", "attribution": "Site name" },
         "content": { "mode": "fetch", "strategy": "article" }, "estimate": { "words": 900 } },
       { "kind": "podcast", "title": "…", "source": { "url": "<feed url>", "license": "unknown", "attribution": "…" },
         "content": { "mode": "fetch", "strategy": "rss-episode", "guid": "…" }, "estimate": { "minutes": 24, "transcript": true } },
       { "kind": "video", "title": "…", "source": { "url": "https://www.youtube.com/watch?v=…", "license": "unknown", "attribution": "Channel" },
         "content": { "mode": "fetch", "strategy": "youtube-subs" }, "estimate": { "minutes": 8 } } ],
     "vocab": [ { "text": "bujía", "translation": "spark plug", "notes": "" } ] }
   ```
   Item `id`s are generated (`a1`, `p1`, `v1`…); give your own stable ids when updating an existing pack (keep the pack `id` too, so a new version only adds new items).
4. **Seed vocab:** 20-40 topic words at the level, from topic knowledge and the probe excerpts. Give a short translation into English. Multi-word entries are fine.
   Reading is only for languages that need one (ja, ko).
5. **Build:** `node scripts/pack/build.mjs <draft.json>` validates with the app's own reader and writes `packs/<language>-<topic>.alpack`
   (`packs/` is gitignored). If it fails, fix the draft; do not hand-edit the pack.
6. **Report** a short table (kind, title, source domain, minutes or words) and the output path. Say what you dropped and why.

## Rules
- Fetch mode only for copyrighted or unknown-licence sources. Never copy page text into the pack.
- `embedded` content only with an allowlisted licence in `source.license` (CC0-1.0, CC-BY-4.0, CC-BY-SA-4.0, PD, own) and real attribution. Inline `text` in a draft item becomes an embedded Markdown file.
- No song lyrics unless openly licensed.
- Podcasts only with transcripts. Videos only with manual subtitles (auto only if nothing else).
- Every URL must have passed a script check. Stop at the caps.
- Keep tool calls few: batch URLs, no re-probing, no reading result pages.
