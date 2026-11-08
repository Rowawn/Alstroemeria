# Changelog

All notable changes to the app are listed here (format: [Keep a Changelog](https://keepachangelog.com/)). Each
release's notes are its section of this file. The version is the app's (`scripts/bump-version.mjs`); crates keep
their own versions.

## [Unreleased]

## [0.2.0] - 2026-11-07

### Added
- Content packs (`.alpack`): import a ready-made set of texts with a review screen before anything is added; the library
  can filter by pack.
- Spaced-repetition reviews (the SRS module) with the FSRS-6 scheduler: decks, review, browse, stats and deck options.
- Concept cards for words, each review showing a different example sentence from your reading; mine a sentence from the
  reader in one tap. Reviews can update a word's status, or the other way round (a setting).
- Anki import and export of `.apkg` files exported with "Support older Anki versions", including review history.
- Reviews sync between devices and are merged when two devices reviewed offline.

## [0.1.1] - 2026-10-28

### Changed
- When a video or audio file will not play, the message now includes the browser's error code and the position it stopped at,
  so the cause can be found.

## [0.1.0] - 2026-10-27

First public release.

### Added
- Reader for texts, EPUBs and web pages with term statuses, multi-word terms and Yomitan-format dictionaries.
- Audio and video with subtitle-synced reading, podcast and YouTube import.
- Extensions (languages, add-ons, panels) with a permission model; first-party Spanish, Japanese and Korean
  language extensions, furigana, pitch accent, hanja and grammar-colour add-ons.
- Japanese and Korean word grouping with deinflection.
- Peer-to-peer sync over the local network: pairing by code or QR, automatic discovery, media and dictionary sharing.
- Japanese and Korean language data as a download (about 10 and 20 MB), or import from a file, or from a paired device.
- Updates: the desktop app offers new versions from GitHub Releases; the Android app shows when one is available.

### Notes
- macOS and Windows builds are not code-signed; see the README for the first-run steps.
