# Lumen Audio Studio

A free, open-source digital audio workstation for Windows, macOS and Linux.
Standalone only: no accounts, no licence keys, no cloud.

This repository is the Rust rewrite. The earlier web and Electron version lives
in [Powelus/pow-daw](https://github.com/Powelus/pow-daw) (last release 0.1.13).

**Status:** early. The crate `las-dsp` holds the audio DSP and is being ported
piece by piece from the TypeScript version, each piece checked against audio
that the original produced. So far: the Facet FM synth, playable live in the
native app `lumen` (on-screen piano, computer keyboard or MIDI keyboard), and
the `las` command line tool, which renders it to a WAV file. Nothing else is
ported yet.

## Download

Builds for Windows, macOS and Linux are on the
[releases page](https://github.com/Powelus/lumen-audio-studio/releases). Every
new version on `main` is published automatically (see [CHANGELOG.md](CHANGELOG.md)).

```sh
./lumen                                              # the app
las render-facet examples/facet-bell.json -o bell.wav # offline render
las render-mist examples/mist-pad.json -o pad.wav
las render-synth examples/synth-supersaw.json -o saw.wav
```

On Linux the app needs the ALSA library (`libasound2`), `libxkbcommon-x11` and
OpenGL, which most desktops already have.

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
cargo run -p las-cli -- render-facet examples/facet-bell.json -o bell.wav
cargo run --release -p las-app    # the app
```

Linux build libraries: `sudo apt install libasound2-dev pkg-config`.

To publish a release, raise `version` under `[workspace.package]` in
`Cargo.toml`, add a `## <version> - <date>` entry to `CHANGELOG.md`, and merge to
`main`. Versions with a `-` are marked as pre-releases.

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE). Anyone may use, change and share this
software; anyone who distributes a modified version must share its source under
the same licence.
