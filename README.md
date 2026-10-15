# Lumen Audio Studio

A free, open-source digital audio workstation for Windows, macOS and Linux.
Standalone only: no accounts, no licence keys, no cloud.

This repository is the Rust rewrite. The earlier web and Electron version lives
in [Powelus/pow-daw](https://github.com/Powelus/pow-daw) (last release 0.1.13).

**Status:** just started. The first crate, `las-dsp`, holds the audio DSP and is
being ported piece by piece from the TypeScript version, each piece checked
against numbers that the original produced.

## Goals

- One DSP codebase for the standalone engine and for VST3/CLAP plugins.
- Native audio output with low latency.
- Host VST3 and CLAP plugins.
- Open `.las` projects and `.lasbank` preset banks from the earlier version.
- Everything the web version does: pattern and song sequencing, piano roll,
  mixer with sends and sidechain, automation, recording, audio clip editing,
  the synths (Polaris, Vortex, Monolith, Zenith, Nebula, TriOsc, Facet, Mist),
  the effects, export to WAV, FLAC, MP3, stems and MIDI.

See [docs/PLAN.md](docs/PLAN.md) for the plan.

## Building

You need a recent stable Rust toolchain.

```sh
cargo test --workspace
```

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE). Anyone may use, change and share this
software; anyone who distributes a modified version must share its source under
the same licence.
