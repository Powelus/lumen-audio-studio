# Contributing

Thanks for helping. A few rules keep the project healthy.

- The project is GPL-3.0-or-later. By sending a contribution you agree it is
  released under that licence.
- `crates/las-dsp` is dual licensed (GPL-3.0-or-later OR a proprietary licence held
  by the copyright holder). Contributions to it are accepted only if you also grant
  the copyright holder the right to release them under that proprietary licence.
- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`
  (in `crates/las-dsp`: `GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary`).
- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` before opening a pull request. CI runs the same
  on Windows, macOS and Linux.
- DSP ported from the web version must come with a parity test: numbers or
  rendered audio produced by the original (see `docs/PARITY.md`).
- No allocation, locks or blocking calls in code that runs on the audio thread.
