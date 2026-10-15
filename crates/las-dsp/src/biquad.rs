// SPDX-License-Identifier: GPL-3.0-or-later
//! RBJ cookbook biquad magnitude response, for filter and EQ curves.
//!
//! Ported from `src/audio/dsp/biquad.ts` of the web version.

use std::f64::consts::PI;

/// Filter shapes of the RBJ cookbook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peaking,
    LowShelf,
    HighShelf,
}

impl Kind {
    /// The names used by the project format and the web version.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "lowpass" => Self::LowPass,
            "highpass" => Self::HighPass,
            "bandpass" => Self::BandPass,
            "notch" => Self::Notch,
            "peaking" => Self::Peaking,
            "lowshelf" => Self::LowShelf,
            "highshelf" => Self::HighShelf,
            _ => return None,
        })
    }
}

/// Normalised coefficients (a0 is 1).
#[derive(Clone, Copy, Debug)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// Coefficients for `kind` at `f0` Hz with `gain_db` (peaking and shelves) and `q`.
pub fn coeffs(kind: Kind, f0: f64, gain_db: f64, q: f64, sample_rate: f64) -> Coeffs {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * f0 / sample_rate;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q.max(0.0001));
    let (b0, b1, b2, a0, a1, a2) = match kind {
        Kind::LowPass => {
            let b0 = (1.0 - cos) / 2.0;
            (b0, 1.0 - cos, b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
        }
        Kind::HighPass => {
            let b0 = (1.0 + cos) / 2.0;
            (b0, -(1.0 + cos), b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
        }
        Kind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        Kind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        Kind::Peaking => (
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        ),
        Kind::LowShelf => {
            // like WebAudio, shelves use S = 1 and ignore q
            let s = 2.0 * a.sqrt() * (sin / 2.0) * 2f64.sqrt();
            (
                a * (a + 1.0 - (a - 1.0) * cos + s),
                2.0 * a * (a - 1.0 - (a + 1.0) * cos),
                a * (a + 1.0 - (a - 1.0) * cos - s),
                a + 1.0 + (a - 1.0) * cos + s,
                -2.0 * (a - 1.0 + (a + 1.0) * cos),
                a + 1.0 + (a - 1.0) * cos - s,
            )
        }
        Kind::HighShelf => {
            let s = 2.0 * a.sqrt() * (sin / 2.0) * 2f64.sqrt();
            (
                a * (a + 1.0 + (a - 1.0) * cos + s),
                -2.0 * a * (a - 1.0 + (a + 1.0) * cos),
                a * (a + 1.0 + (a - 1.0) * cos - s),
                a + 1.0 - (a - 1.0) * cos + s,
                2.0 * (a - 1.0 - (a + 1.0) * cos),
                a + 1.0 - (a - 1.0) * cos - s,
            )
        }
    };
    Coeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

impl Coeffs {
    /// Magnitude response in dB at `freq` Hz.
    pub fn magnitude_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let mag = |c0: f64, c1: f64, c2: f64| {
            let re = c0 + c1 * (-w).cos() + c2 * (-2.0 * w).cos();
            let im = c1 * (-w).sin() + c2 * (-2.0 * w).sin();
            re.hypot(im)
        };
        let ratio = mag(self.b0, self.b1, self.b2) / mag(1.0, self.a1, self.a2);
        20.0 * ratio.max(1e-9).log10()
    }
}

/// Magnitude response in dB of one biquad, the same call shape as the web version.
pub fn biquad_db(kind: Kind, f0: f64, gain_db: f64, q: f64, freq: f64, sample_rate: f64) -> f64 {
    coeffs(kind, f0, gain_db, q, sample_rate).magnitude_db(freq, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values produced by `biquadDb` of the TypeScript version (48 kHz).
    /// (kind, f0, gain dB, q, frequency, dB)
    const FROM_TS: &[(&str, f64, f64, f64, f64, f64)] = &[
        ("lowpass", 1000.0, 0.0, 0.707, 1000.0, -3.011611724),
        ("lowpass", 1000.0, 0.0, 0.707, 8000.0, -37.797154706),
        ("lowpass", 500.0, 0.0, 4.0, 500.0, 12.041199827),
        ("highpass", 1000.0, 0.0, 0.707, 100.0, -40.025039816),
        ("bandpass", 2000.0, 0.0, 2.0, 2000.0, 0.000000000),
        ("bandpass", 2000.0, 0.0, 2.0, 500.0, -17.629736580),
        ("notch", 1000.0, 0.0, 1.0, 2000.0, -1.577970241),
        ("peaking", 1000.0, 9.0, 1.0, 1000.0, 9.000000000),
        ("peaking", 3000.0, -6.0, 2.0, 5000.0, -1.039653557),
        ("lowshelf", 200.0, 6.0, 1.0, 50.0, 5.974781863),
        ("highshelf", 5000.0, -4.0, 1.0, 12000.0, -3.945788441),
    ];

    #[test]
    fn matches_the_typescript_version() {
        for &(name, f0, gain, q, freq, want) in FROM_TS {
            let kind = Kind::parse(name).expect("known kind");
            let got = biquad_db(kind, f0, gain, q, freq, 48000.0);
            assert!((got - want).abs() < 1e-6, "{name}: got {got}, want {want}");
        }
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(Kind::parse("lowpass"), Some(Kind::LowPass));
        assert_eq!(Kind::parse("ladder"), None);
    }

    #[test]
    fn a_notch_is_deep_at_its_centre() {
        assert!(biquad_db(Kind::Notch, 1000.0, 0.0, 1.0, 1000.0, 48000.0) < -60.0);
    }
}
