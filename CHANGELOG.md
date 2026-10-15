# Changelog

Each version that lands on `main` is published as a GitHub Release. Versions with a `-` (for example `-alpha.1`) are pre-releases.

## 0.2.0-alpha.3 - 2026-10-15

Mist, the granular synth, joins Facet.

- Mist engine ported from the TypeScript version: grains from a source sample, position, scan, spray, size, density, transpose, random pitch, stereo spread, window shape, reverse, key tracking, the modulation matrix, filter, mono and voice stealing. Output and the built-in texture match the original sample for sample on five reference renders.
- `las render-mist`, with an example patch (`examples/mist-pad.json`).
- In the app: switch between Facet and Mist, Mist controls, a Pad preset, and drop a `.wav` file on the window to use it as Mist's source. Start on Mist with `lumen --mist`.
- Engine: instruments now receive the shared random stream, so seeded renders repeat exactly.

## 0.2.0-alpha.2 - 2026-10-15

The first native app: `lumen`. It plays the Facet FM synth live.

- Native window drawn on the GPU with egui. No web view, no browser.
- Live audio output on Windows, macOS and Linux, with a lock-free link between the interface and the audio thread. Patch changes are parsed off the audio thread.
- Play with the on-screen piano, the computer keyboard (A to ; for notes, Z and X for octave) or a MIDI keyboard (notes, pitch bend, mod wheel, pressure).
- Facet controls: algorithm, feedback, four operators, amp envelope, filter, mono, legato, glide. Three starting sounds.
- Meters, DSP load, voice count and dropout counter in the window.
- Engine: patches can be prepared ahead of time and swapped in without allocating.
- Linux needs the ALSA library (`libasound2`), `libxkbcommon-x11` and OpenGL.

## 0.2.0-alpha.1 - 2026-10-15

First release of the Rust rewrite. Early: the command line tool only, no interface and no live audio yet.

- `las render-facet`: renders a Facet (four-operator FM) patch and a list of notes to a 32-bit float stereo WAV.
- Facet engine ported from the TypeScript version: eight algorithms, operator feedback, mono, legato and glide modes, voice stealing, two LFOs (including sync, sample and hold, smooth), a modulation matrix, and a per-voice filter.
- The Rust output matches the TypeScript engine sample for sample on five reference renders (see `docs/PARITY.md`).
- Two example patches in `examples/`.
- Licensed GPL-3.0-or-later.
