# Changelog

Each version that lands on `main` is published as a GitHub Release. Versions with a `-` (for example `-alpha.1`) are pre-releases.

## 0.2.0-alpha.6 - 2026-10-15

The first effects, and the loudness meter.

- Effects ported from the web version: compressor (with external sidechain), look-ahead limiter, bit crusher, noise gate, transient shaper, tape, frequency shifter, pitch shifter, hall reverb, echo, step FX and the tempo-synced volume shaper. Output matches the original sample for sample on 19 reference renders.
- Loudness meter (BS.1770-4 and EBU R128): K-weighting, momentary, short-term and gated integrated loudness, loudness range, sample and true peak, stereo correlation. It reads the same numbers as the original.
- Effect settings load from the JSON the web version saved: only the keys present change.

## 0.2.0-alpha.5 - 2026-10-15

Factory presets and a preset browser.

- All 444 factory presets of the web version (Polaris 44, Vortex 36, Monolith 30, Zenith 34, TriOsc 16, Facet 33, Mist 21, Nebula 230) are included as JSON banks. Every Polaris-family, Facet and Mist preset is tested to load and play. Nebula's banks are included but Nebula itself is not ported yet.
- The app has a preset list per instrument: previous and next buttons, a search box and categories.
- Save your own presets (Save as) and delete them again. They are kept in your configuration folder, one file per instrument.

## 0.2.0-alpha.4 - 2026-10-15

Polaris, the wavetable synth, and the engine behind Vortex, Monolith, Zenith, Nebula and TriOsc.

- Wavetable synth engine ported from the web version: four wavetable oscillators with unison and ten warp modes, sub oscillator, noise, two filters in twelve types (low, high and band pass in 12 and 24 dB, notch, peak, ladder, formant, comb), four envelopes, four LFOs, a 21-source modulation matrix, mono, legato and glide. Output matches the original sample for sample on eleven reference renders covering every warp mode and filter type.
- 23 built-in band-limited, mip-mapped wavetables and the FFT behind them, checked against the original.
- `las render-synth` and `examples/synth-supersaw.json`.
- In the app: Polaris with tabs for oscillators, filters, envelopes and LFOs, a modulation matrix editor, and voice and global controls. Polaris is the default instrument.
- Wavetables and patches are prepared off the audio thread and swapped in.
- Engines share a `Playable` trait, so the command line tool and the app treat them alike.

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
