# Changelog

Each version that lands on `main` is published as a GitHub Release. Versions with a `-` (for example `-alpha.1`) are pre-releases.

## 0.2.0-alpha.9 - 2026-10-15

Play a project: the scheduler, the mixer and an offline render.

- New crate `las-engine`. It schedules a project's notes (patterns, the playlist, swing, tempo automation, the arpeggiator), runs each channel's instrument (Polaris, Vortex, Monolith, Zenith, TriOsc, Nebula with its layers and macros, Facet, Mist) through its effects rack, and mixes the inserts: faders, pan, width, mute and solo, effect slots, sends, routing, sidechain (chosen source or an insert routed "sidechain only"), and plugin delay compensation.
- `las render-project song.las -o song.wav` renders the whole song, or one pattern with `--pattern`, to a 32 bit float WAV.
- Checked against the web version by rendering the same projects in headless Chromium and comparing: the instruments, the mixer, routing, sidechain and the arpeggiator match to rounding (more than 120 dB below the signal), and the effects Chromium builds from Web Audio nodes match to better than 85 dB.
- To get there the Web Audio based effects were corrected against the browser: the overdrive uses Chromium's 4x oversampling filters and their 192 sample delay, the maximizer uses a port of Chromium's dynamics compressor (look-ahead, adaptive release), biquads run in direct form I with `f32` between nodes, the chorus reads its delay line with linear interpolation and feeds back one render quantum late, and the phaser's feedback delay is 128 samples (it was 129). Cutoffs at 0 or at the Nyquist frequency behave as in the browser.
- Not yet in the render: the sampler and its factory samples, audio clips, LumaPad, and automation clips.

## 0.2.0-alpha.8 - 2026-10-15

Projects: the model and the `.las` file format.

- New crate `las-project`: the project model (channels, patterns, notes, playlist, automation, mixer with effect slots and sends, samples, markers, MIDI map) as the web version saves it. Fields the model does not know are kept, so a save and reopen loses nothing.
- Open `.las` files from the web version, encrypted or plain, with their embedded samples. Files written here open in the web version. The file is checked three ways: the authentication tag, the CRC and the length, and each failure says what is wrong.
- Migration and repair are ported: version 1 projects are updated, and bad tempo, time signature, routing loops, dangling sends, clips, automation and MIDI assignments, duplicate ids and out of range values are repaired, each reported as a note. Checked against the web version on the demo song and on a deliberately damaged project (same repairs, same notes, same order).
- `las info project.las` lists what a project holds. `las convert in.las out.las [--plain]` reads and writes it again.

## 0.2.0-alpha.7 - 2026-10-15

The effects rack.

- Eight more effects, completing the catalogue: parametric EQ (seven bands), overdrive, chorus, phaser, auto filter, stereo tool, maximizer (three-band) and convolution reverb. The web version builds these from Web Audio's own filter, delay, wave shaper and convolver nodes, which are not specified to the sample, so they are written to behave like those nodes (biquad formulas, wave shaper curve, convolver normalisation) and read the same settings.
- Every instrument has an effects rack in the app (Effects tab): add, remove and reorder effects, enable and mix each one, pick effect presets (80 from the web version), and tempo for the synced ones. Controls are drawn from a catalogue of the effects (ranges, curves, units, options), with an EQ response curve, trance gate steps, volume shaper curves and step FX lanes.
- Factory presets bring their effects. Saving a preset saves the rack with it.
- Effects and patches are swapped on the audio thread without allocating or freeing: the old ones are handed back to the interface thread to be dropped there.

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
