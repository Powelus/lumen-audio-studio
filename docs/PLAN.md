# Plan

## Decisions

| Topic | Decision |
| --- | --- |
| Licence | GPL-3.0-or-later |
| Platforms | Windows, macOS, Linux. No browser version. |
| Language | Rust for the engine, the DSP and the plugin work |
| Project format | Read the `.las` files and `.lasbank` banks of the web version |
| Interface | Native Rust, drawn on the GPU with egui. No web view. Chosen for speed and memory, and so plugin windows can be embedded natively. |
| Audio | cpal for output, midir for MIDI input. The audio thread owns the engine and never waits on the interface (lock-free queues, atomics for meters). |

## Stages

Each stage ships and keeps the tests green.

1. **DSP crate.** Port the filters, wavetables, the synth voice engine (Polaris
   family), Facet (FM), Mist (granular) and the effects into `las-dsp`. Each
   piece passes parity tests against the web version.
2. **Engine and project model.** Project data model and `.las` reader and writer,
   scheduler, mixer, automation, offline render, export (WAV, FLAC, MP3, MIDI,
   stems). Command-line renderer first, so the sound can be checked without an
   interface. Done: project model, `.las` reader and writer, migration and repair
   (`las-project`), scheduler, mixer and offline render with the synths, the sampler and the
   factory samples (`las-engine`). Next: audio clips, LumaPad, automation, then
   export.
3. **Audio output and MIDI.** Native audio output and MIDI input, with a live
   engine that follows the project.
4. **Interface.** Native egui app. Started in 0.2.0-alpha.2 with a playable Facet synth, then grows to the channel rack, piano roll, playlist and mixer as the engine stages land.
5. **Plugins.** Host VST3 and CLAP plugins; export the synths and effects as
   VST3 and CLAP.

## Presets

The factory presets (about 440) are data. They are exported once from the web
version to JSON and loaded by the Rust engine, with the same validation rules.

## Performance rules

- The audio callback never allocates, locks or parses. Anything heavy (patch
  parsing, loading samples, building graphs) happens on another thread and is
  swapped in.
- The interface reads meters from atomics and sends commands through lock-free
  queues. It can stall without a glitch in the sound.
- The app shows DSP load, voice count and dropouts, so regressions are visible.
- Known gaps: replaced patches are freed on the interface thread, but the
  instrument still frees its own previous copy on the audio thread (a few small
  allocations per patch change). To fix when the instruments move to preallocated
  state.
