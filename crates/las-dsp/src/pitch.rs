// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! Pitch detection for the tuner: the YIN method (de Cheveigne and Kawahara), which
//! finds the period of a monophonic signal from its difference function.

/// What a tuner shows for a detected pitch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Frequency, Hz.
    pub hz: f64,
    /// The nearest MIDI key.
    pub key: i32,
    /// How far from that key, cents (-50 to 50).
    pub cents: f64,
    /// How sure the detector is, 0 to 1.
    pub clarity: f64,
}

/// The fundamental frequency of `x`, between `min_hz` and `max_hz`, or `None` when the
/// signal is silent or has no clear period.
pub fn detect(x: &[f32], sr: f64, min_hz: f64, max_hz: f64) -> Option<(f64, f64)> {
    let max_lag = ((sr / min_hz) as usize).min(x.len() / 2);
    let min_lag = ((sr / max_hz) as usize).max(2);
    if max_lag <= min_lag + 2 {
        return None;
    }
    let energy: f64 = x.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    if energy / (x.len() as f64) < 1e-7 {
        return None;
    }
    let w = x.len() - max_lag;
    // difference function and its cumulative mean normalised form
    let mut d = vec![0.0f64; max_lag + 1];
    for (tau, slot) in d.iter_mut().enumerate().skip(1) {
        let mut s = 0.0;
        for i in 0..w {
            let v = f64::from(x[i]) - f64::from(x[i + tau]);
            s += v * v;
        }
        *slot = s;
    }
    let mut cmnd = vec![1.0f64; max_lag + 1];
    let mut run = 0.0;
    for tau in 1..=max_lag {
        run += d[tau];
        cmnd[tau] = if run > 0.0 {
            d[tau] * tau as f64 / run
        } else {
            1.0
        };
    }
    let threshold = 0.15;
    let mut tau = min_lag;
    let mut found = None;
    while tau < max_lag {
        if cmnd[tau] < threshold {
            while tau + 1 < max_lag && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            found = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = found.or_else(|| {
        // no dip under the threshold: the deepest one, if it is clear enough
        let best = (min_lag..max_lag).min_by(|&a, &b| cmnd[a].total_cmp(&cmnd[b]))?;
        (cmnd[best] < 0.35).then_some(best)
    })?;
    // parabolic interpolation around the dip
    let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-12 {
        (0.5 * (a - c) / den).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let period = tau as f64 + shift;
    Some((sr / period, (1.0 - b).clamp(0.0, 1.0)))
}

/// A pitch as a key and cents, for a tuning reference of `a4` Hz.
pub fn reading(hz: f64, clarity: f64, a4: f64) -> Reading {
    let midi = 69.0 + 12.0 * (hz / a4).log2();
    let key = midi.round() as i32;
    Reading {
        hz,
        key,
        cents: (midi - key as f64) * 100.0,
        clarity,
    }
}

/// The name of a MIDI key, in sharps, with the octave (60 is C4).
pub fn key_name(key: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[key.rem_euclid(12) as usize],
        key.div_euclid(12) - 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, sr: f64, n: usize, harmonics: bool) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                let mut v = (std::f64::consts::TAU * hz * t).sin();
                if harmonics {
                    v += 0.5 * (std::f64::consts::TAU * 2.0 * hz * t).sin()
                        + 0.3 * (std::f64::consts::TAU * 3.0 * hz * t).sin();
                }
                (v * 0.4) as f32
            })
            .collect()
    }

    #[test]
    fn sines_and_rich_tones_are_found_within_a_few_cents() {
        let sr = 48000.0;
        for hz in [55.0, 110.0, 220.0, 440.0, 466.16, 1000.0] {
            for rich in [false, true] {
                let x = tone(hz, sr, 4096, rich);
                let (f, clarity) = detect(&x, sr, 40.0, 2000.0).expect("a pitch");
                let cents = 1200.0 * (f / hz).log2();
                assert!(cents.abs() < 5.0, "{hz} Hz read as {f} Hz");
                assert!(clarity > 0.8);
            }
        }
    }

    #[test]
    fn silence_and_noise_have_no_pitch() {
        assert!(detect(&vec![0.0; 4096], 48000.0, 40.0, 2000.0).is_none());
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..4096)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / 16777216.0 - 0.5
            })
            .collect();
        assert!(detect(&noise, 48000.0, 40.0, 2000.0).is_none());
    }

    #[test]
    fn readings_name_the_note_and_the_cents() {
        let r = reading(440.0, 1.0, 440.0);
        assert_eq!((r.key, key_name(r.key).as_str()), (69, "A4"));
        assert!(r.cents.abs() < 1e-9);
        let r = reading(261.63 * 2f64.powf(20.0 / 1200.0), 1.0, 440.0);
        assert_eq!(key_name(r.key), "C4");
        assert!((r.cents - 20.0).abs() < 0.5);
        assert_eq!(key_name(-1), "B-2");
    }
}
