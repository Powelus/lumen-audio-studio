# Plan

## Decisions

| Topic | Decision |
| --- | --- |
| Licence | GPL-3.0-or-later |
| Platforms | Windows, macOS, Linux. No browser version. |
| Language | Rust for the engine, the DSP and the plugin work |
| Project format | Read the `.las` files and `.lasbank` banks of the web version |
| Interface | Open: a webview shell (Tauri) reusing the React interface, or a native Rust interface |

## Stages

Each stage ships and keeps the tests green.

1. **DSP crate.** Port the filters, wavetables, the synth voice engine (Polaris
   family), Facet (FM), Mist (granular) and the effects into `las-dsp`. Each
   piece passes parity tests against the web version.
2. **Engine and project model.** Project data model and `.las` reader and writer,
   scheduler, mixer, automation, offline render, export (WAV, FLAC, MP3, MIDI,
   stems). Command-line renderer first, so the sound can be checked without an
   interface.
3. **Audio output and MIDI.** Native audio output and MIDI input, with a live
   engine that follows the project.
4. **Interface.** Decided before this stage starts.
5. **Plugins.** Host VST3 and CLAP plugins; export the synths and effects as
   VST3 and CLAP.

## Presets

The factory presets (about 440) are data. They are exported once from the web
version to JSON and loaded by the Rust engine, with the same validation rules.
