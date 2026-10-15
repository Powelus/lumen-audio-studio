// SPDX-License-Identifier: GPL-3.0-or-later
//! Loudness and peak measurement following ITU-R BS.1770-4 and EBU R128:
//! K-weighting, momentary (400 ms), short-term (3 s) and gated integrated
//! loudness, loudness range (EBU Tech 3342) and 4x oversampled true peak.
//!
//! Ported from `src/audio/dsp/loudness.ts` of the web version.

// The index loops follow the original line by line, which keeps the port easy to check.
#![allow(clippy::needless_range_loop)]

use std::f64::consts::PI;
use std::sync::OnceLock;

/// Direct form I biquad (state per channel).
#[derive(Clone, Debug)]
struct Biquad {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    fn new(c: &Coefs) -> Self {
        Self {
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            b0: c.b0,
            b1: c.b1,
            b2: c.b2,
            a1: c.a1,
            a2: c.a2,
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // flush denormals in the recursive path
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        y
    }
}

/// Coefficients of one biquad stage.
#[derive(Clone, Copy, Debug)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// The two K-weighting stages (high shelf, then RLB high pass) for a sample rate.
pub fn k_weighting_coefficients(fs: f64) -> (Coefs, Coefs) {
    // pre-filter (shelving), from the BS.1770 reference design
    let mut f0 = 1681.974450955533;
    let g = 3.999843853973347;
    let mut q = 0.7071752369554196;
    let mut k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let mut a0 = 1.0 + k / q + k * k;
    let shelf = Coefs {
        b0: (vh + (vb * k) / q + k * k) / a0,
        b1: (2.0 * (k * k - vh)) / a0,
        b2: (vh - (vb * k) / q + k * k) / a0,
        a1: (2.0 * (k * k - 1.0)) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    // RLB high pass
    f0 = 38.13547087602444;
    q = 0.5003270373238773;
    k = (PI * f0 / fs).tan();
    a0 = 1.0 + k / q + k * k;
    let hp = Coefs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: (2.0 * (k * k - 1.0)) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    (shelf, hp)
}

/// 4x oversampling interpolator for true-peak detection (windowed sinc, 12 taps per phase).
const TP_PHASES: usize = 4;
const TP_TAPS: usize = 12;

fn tp_coef() -> &'static [[f64; TP_TAPS]; TP_PHASES] {
    static CELL: OnceLock<[[f64; TP_TAPS]; TP_PHASES]> = OnceLock::new();
    CELL.get_or_init(|| {
        let total = (TP_PHASES * TP_TAPS) as f64;
        let mut out = [[0.0; TP_TAPS]; TP_PHASES];
        for (ph, c) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (k, ck) in c.iter_mut().enumerate() {
                // position of tap k for phase ph, in input samples, centred on the filter
                let m = (k * TP_PHASES + ph) as f64;
                let t = (m - (total - 1.0) / 2.0) / TP_PHASES as f64;
                let sinc = if t == 0.0 {
                    1.0
                } else {
                    (PI * t).sin() / (PI * t)
                };
                let w = 0.42 - 0.5 * (2.0 * PI * (m + 0.5) / total).cos()
                    + 0.08 * (4.0 * PI * (m + 0.5) / total).cos();
                *ck = sinc * w;
                sum += *ck;
            }
            for ck in c.iter_mut() {
                *ck /= sum;
            }
        }
        out
    })
}

/// What a measurement came to.
#[derive(Clone, Copy, Debug)]
pub struct LoudnessSummary {
    /// Integrated (gated) loudness, LUFS. Negative infinity when nothing exceeds the gate.
    pub integrated: f64,
    /// Loudness range, LU.
    pub range: f64,
    /// Highest momentary and short-term loudness seen, LUFS.
    pub max_momentary: f64,
    pub max_short_term: f64,
    /// dBFS and dBTP.
    pub sample_peak: f64,
    pub true_peak: f64,
}

fn to_lufs(mean_square: f64) -> f64 {
    if mean_square > 0.0 {
        -0.691 + 10.0 * mean_square.log10()
    } else {
        f64::NEG_INFINITY
    }
}

fn to_db(x: f64) -> f64 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// Streaming loudness meter. Feed audio with [`LoudnessMeter::process`]; read
/// values at any time. Energy is kept per 100 ms sub-block, so momentary,
/// short-term and integrated loudness share one history.
pub struct LoudnessMeter {
    sample_rate: f64,
    channels: usize,
    filters: Vec<(Biquad, Biquad)>,
    sub_len: usize,
    sub_pos: usize,
    sub_acc: f64,
    /// Mean square (summed over channels) of each completed 100 ms sub-block.
    subs: Vec<f64>,
    short_terms: Vec<f64>,
    max_m: f64,
    max_s: f64,
    peak: f64,
    tpeak: f64,
    tp_hist: Vec<[f64; TP_TAPS]>,
    tp_pos: usize,
}

impl LoudnessMeter {
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        let (shelf, hp) = k_weighting_coefficients(sample_rate);
        Self {
            sample_rate,
            channels,
            filters: (0..channels)
                .map(|_| (Biquad::new(&shelf), Biquad::new(&hp)))
                .collect(),
            sub_len: (sample_rate * 0.1).round() as usize,
            sub_pos: 0,
            sub_acc: 0.0,
            subs: Vec::new(),
            short_terms: Vec::new(),
            max_m: f64::NEG_INFINITY,
            max_s: f64::NEG_INFINITY,
            peak: 0.0,
            tpeak: 0.0,
            tp_hist: vec![[0.0; TP_TAPS]; channels],
            tp_pos: 0,
        }
    }

    /// Start a new measurement.
    pub fn reset(&mut self) {
        *self = Self::new(self.sample_rate, self.channels);
    }

    /// Process `n` frames (planar channels).
    pub fn process(&mut self, input: &[&[f32]], n: usize) {
        let chs = self.channels.min(input.len());
        let coef = tp_coef();
        for i in 0..n {
            let mut e = 0.0;
            for c in 0..chs {
                let x = f64::from(input[c][i]);
                let (shelf, hp) = &mut self.filters[c];
                let y = hp.run(shelf.run(x));
                e += y * y;
                let ax = x.abs();
                if ax > self.peak {
                    self.peak = ax;
                }
                // true peak: push into the history and evaluate the 4 interpolation phases
                let h = &mut self.tp_hist[c];
                h[self.tp_pos] = x;
                for ph in coef.iter() {
                    let mut acc = 0.0;
                    for (k, ck) in ph.iter().enumerate() {
                        acc += ck * h[(self.tp_pos + TP_TAPS - k) % TP_TAPS];
                    }
                    let a = acc.abs();
                    if a > self.tpeak {
                        self.tpeak = a;
                    }
                }
            }
            self.tp_pos = (self.tp_pos + 1) % TP_TAPS;
            self.sub_acc += e;
            self.sub_pos += 1;
            if self.sub_pos >= self.sub_len {
                self.close_sub();
            }
        }
    }

    fn close_sub(&mut self) {
        self.subs.push(self.sub_acc / self.sub_len as f64);
        self.sub_acc = 0.0;
        self.sub_pos = 0;
        let m = self.window_loudness(4);
        if self.subs.len() >= 4 && m > self.max_m {
            self.max_m = m;
        }
        if self.subs.len() >= 30 {
            let s = self.window_loudness(30);
            self.short_terms.push(s);
            if s > self.max_s {
                self.max_s = s;
            }
        }
    }

    fn window_loudness(&self, count: usize) -> f64 {
        let k = count.min(self.subs.len());
        if k == 0 {
            return f64::NEG_INFINITY;
        }
        let sum: f64 = self.subs[self.subs.len() - k..].iter().sum();
        to_lufs(sum / k as f64)
    }

    /// Momentary loudness (last 400 ms).
    pub fn momentary(&self) -> f64 {
        if self.subs.len() >= 4 {
            self.window_loudness(4)
        } else {
            f64::NEG_INFINITY
        }
    }

    /// Short-term loudness (last 3 s).
    pub fn short_term(&self) -> f64 {
        if self.subs.len() >= 30 {
            self.window_loudness(30)
        } else {
            self.window_loudness(self.subs.len())
        }
    }

    /// Gated integrated loudness over everything measured so far.
    pub fn integrated(&self) -> f64 {
        // 400 ms blocks with 75 % overlap are 4 consecutive 100 ms sub-blocks
        let blocks: Vec<f64> = (3..self.subs.len())
            .map(|i| (self.subs[i] + self.subs[i - 1] + self.subs[i - 2] + self.subs[i - 3]) / 4.0)
            .collect();
        let abs: Vec<f64> = blocks
            .into_iter()
            .filter(|ms| to_lufs(*ms) > -70.0)
            .collect();
        if abs.is_empty() {
            return f64::NEG_INFINITY;
        }
        let ungated = to_lufs(abs.iter().sum::<f64>() / abs.len() as f64);
        let rel: Vec<f64> = abs
            .into_iter()
            .filter(|ms| to_lufs(*ms) > ungated - 10.0)
            .collect();
        if rel.is_empty() {
            return f64::NEG_INFINITY;
        }
        to_lufs(rel.iter().sum::<f64>() / rel.len() as f64)
    }

    /// Loudness range (EBU Tech 3342), LU.
    pub fn range(&self) -> f64 {
        let abs: Vec<f64> = self
            .short_terms
            .iter()
            .copied()
            .filter(|l| *l > -70.0)
            .collect();
        if abs.len() < 2 {
            return 0.0;
        }
        let mean_ms = abs
            .iter()
            .fold(0.0, |a, l| a + 10f64.powf((l + 0.691) / 10.0))
            / abs.len() as f64;
        let gate = to_lufs(mean_ms) - 20.0;
        let mut vals: Vec<f64> = abs.into_iter().filter(|l| *l > gate).collect();
        if vals.len() < 2 {
            return 0.0;
        }
        vals.sort_by(|a, b| a.total_cmp(b));
        let pct = |p: f64| {
            let i = ((p / 100.0) * (vals.len() - 1) as f64 + 0.5)
                .floor()
                .max(0.0);
            vals[(i as usize).min(vals.len() - 1)]
        };
        pct(95.0) - pct(10.0)
    }

    pub fn sample_peak_db(&self) -> f64 {
        to_db(self.peak)
    }

    pub fn true_peak_db(&self) -> f64 {
        to_db(self.tpeak.max(self.peak))
    }

    pub fn summary(&self) -> LoudnessSummary {
        LoudnessSummary {
            integrated: self.integrated(),
            range: self.range(),
            max_momentary: self.max_m,
            max_short_term: self.max_s,
            sample_peak: self.sample_peak_db(),
            true_peak: self.true_peak_db(),
        }
    }
}

/// Measure a complete render (planar channels).
pub fn analyze_loudness(channels: &[&[f32]], sample_rate: f64) -> LoudnessSummary {
    let mut m = LoudnessMeter::new(sample_rate, channels.len());
    let len = channels.first().map_or(0, |c| c.len());
    let block = 4096;
    let mut i = 0;
    while i < len {
        let n = block.min(len - i);
        let parts: Vec<&[f32]> = channels.iter().map(|c| &c[i..i + n]).collect();
        m.process(&parts, n);
        i += n;
    }
    m.summary()
}

/// Stereo correlation (-1 to 1) of two channels.
pub fn correlation(l: &[f32], r: &[f32]) -> f64 {
    let (mut lr, mut ll, mut rr) = (0.0, 0.0, 0.0);
    for i in 0..l.len().min(r.len()) {
        let (a, b) = (f64::from(l[i]), f64::from(r[i]));
        lr += a * b;
        ll += a * a;
        rr += b * b;
    }
    let d = (ll * rr).sqrt();
    if d > 1e-12 {
        lr / d
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, amp: f64, seconds: f64, sr: f64) -> Vec<f32> {
        (0..(seconds * sr) as usize)
            .map(|i| (amp * (2.0 * PI * f * i as f64 / sr).sin()) as f32)
            .collect()
    }

    #[test]
    fn a_full_scale_997_hz_sine_in_both_channels_reads_minus_3_lufs() {
        // BS.1770: a 0 dBFS, 997 Hz sine in one channel reads -3.01 LUFS; in two, 0 dB more
        let s = sine(997.0, 1.0, 8.0, 48000.0);
        let m = analyze_loudness(&[&s, &s], 48000.0);
        assert!((m.integrated - 0.0).abs() < 0.1, "{}", m.integrated);
        let one = analyze_loudness(&[&s], 48000.0);
        assert!((one.integrated + 3.01).abs() < 0.1, "{}", one.integrated);
        assert!((m.sample_peak - 0.0).abs() < 1e-6);
    }

    #[test]
    fn silence_has_no_loudness_and_a_quieter_signal_reads_lower() {
        let z = vec![0f32; 48000 * 2];
        assert_eq!(
            analyze_loudness(&[&z, &z], 48000.0).integrated,
            f64::NEG_INFINITY
        );
        let loud = sine(1000.0, 0.5, 4.0, 48000.0);
        let quiet = sine(1000.0, 0.25, 4.0, 48000.0);
        let a = analyze_loudness(&[&loud, &loud], 48000.0).integrated;
        let b = analyze_loudness(&[&quiet, &quiet], 48000.0).integrated;
        assert!((a - b - 6.02).abs() < 0.1, "{a} {b}");
    }

    #[test]
    fn correlation_is_one_for_equal_and_minus_one_for_inverted_channels() {
        let s = sine(300.0, 0.5, 1.0, 48000.0);
        let inv: Vec<f32> = s.iter().map(|x| -x).collect();
        assert!((correlation(&s, &s) - 1.0).abs() < 1e-9);
        assert!((correlation(&s, &inv) + 1.0).abs() < 1e-9);
    }
}
