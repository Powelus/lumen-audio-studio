// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
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
pub mod modular;
mod nebula_gen;
pub mod onset;
pub mod param;
pub mod patch;
pub mod pitch;
pub mod presets;
pub mod stretch;
pub mod synth;
pub mod wavetable;

/// `x.floor()` without the library call that x86-64 builds without SSE4.1 make for it
/// (it showed up in profiles of delay lines and oscillators). The same value as
/// `floor` for every finite `x` within ±2^62, which covers every phase and index here.
#[inline(always)]
pub fn floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

#[cfg(test)]
mod floor_tests {
    #[test]
    fn floor_matches_the_library() {
        for x in [
            0.0,
            0.5,
            1.0,
            1.5,
            -0.5,
            -1.0,
            -1.5,
            2.999_999,
            -2.000_001,
            1e9 + 0.25,
            -1e9 - 0.25,
        ] {
            assert_eq!(super::floor(x), x.floor(), "{x}");
        }
    }
}
