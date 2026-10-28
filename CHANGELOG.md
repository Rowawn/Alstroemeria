# Changelog

All notable changes to the app are listed here (format: [Keep a Changelog](https://keepachangelog.com/)). Each
release's notes are its section of this file. The version is the app's (`scripts/bump-version.mjs`); crates keep
their own versions.

## [Unreleased]

## [0.1.0] - Unreleased

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
