// SPDX-License-Identifier: GPL-3.0-or-later
//! DSP for Lumen Audio Studio.
//!
//! Everything here is plain Rust with no allocation in the audio path, so the
//! same code can run in the standalone engine and inside plugins. Each module
//! is ported from the TypeScript original and checked against numbers that the
//! TypeScript code produced (see `docs/PARITY.md`).

pub mod biquad;
pub mod engine;
pub mod fft;
pub mod fm;
pub mod fx;
pub mod loudness;
pub mod mist;
pub mod param;
pub mod patch;
pub mod presets;
pub mod stretch;
pub mod synth;
pub mod wavetable;
