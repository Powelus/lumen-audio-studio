# Changelog

Each version that lands on `main` is published as a GitHub Release. Versions with a `-` (for example `-alpha.1`) are pre-releases.

## 0.2.0-alpha.1 - 2026-10-15

First release of the Rust rewrite. Early: the command line tool only, no interface and no live audio yet.

- `las render-facet`: renders a Facet (four-operator FM) patch and a list of notes to a 32-bit float stereo WAV.
- Facet engine ported from the TypeScript version: eight algorithms, operator feedback, mono, legato and glide modes, voice stealing, two LFOs (including sync, sample and hold, smooth), a modulation matrix, and a per-voice filter.
- The Rust output matches the TypeScript engine sample for sample on five reference renders (see `docs/PARITY.md`).
- Two example patches in `examples/`.
- Licensed GPL-3.0-or-later.
