// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! Tape emulation, frequency shifter and pitch shifter.
//! Ported from `src/audio/dsp/fx/modulation2.ts`.

use super::{db_to_lin, flush, update_params, Biq, BiqKind, DelayLine, StereoEffect, TAU};
use crate::engine::PatchError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Tape
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TapeParams {
    /// Input drive, dB.
    pub drive: f64,
    /// Asymmetry (even harmonics), 0 to 1.
    pub bias: f64,
    /// Low-frequency head bump, dB.
    pub bump: f64,
    /// High-frequency roll-off, Hz.
    pub tone: f64,
    /// Slow pitch drift depth, 0 to 1.
    pub wow: f64,
    /// Fast pitch flutter depth, 0 to 1.
    pub flutter: f64,
    pub output: f64,
    pub mix: f64,
}

impl Default for TapeParams {
    fn default() -> Self {
        Self {
            drive: 6.0,
            bias: 0.2,
            bump: 2.0,
            tone: 14000.0,
            wow: 0.15,
            flutter: 0.1,
            output: 0.0,
            mix: 1.0,
        }
    }
}

/// Fixed latency of the tape effect (the centre of its modulated delay), seconds.
pub const TAPE_LATENCY: f64 = 0.004;

/// Tape-style processing: pre-emphasis, asymmetric soft saturation (2x
/// oversampled), de-emphasis, head bump, high-frequency loss and wow and flutter
/// through a modulated delay with a fixed 4 ms centre.
pub struct Tape {
    pub p: TapeParams,
    sr: f64,
    dl: [DelayLine; 2],
    dry_lines: [DelayLine; 2],
    pre: [Biq; 2],
    de: [Biq; 2],
    bump_f: [Biq; 2],
    tone_f: [Biq; 2],
    aa: [Biq; 2],
    last: [f64; 2],
    wow_ph: f64,
    fl_ph: f64,
    drift: f64,
    key: Option<(f64, f64)>,
    seed: u32,
}

impl Tape {
    pub fn new(sr: f64) -> Self {
        let n = (sr * 0.02).ceil() as usize;
        let two = || [Biq::default(), Biq::default()];
        Self {
            p: TapeParams::default(),
            sr,
            dl: [DelayLine::new(n), DelayLine::new(n)],
            dry_lines: [DelayLine::new(n), DelayLine::new(n)],
            pre: two(),
            de: two(),
            bump_f: two(),
            tone_f: two(),
            aa: two(),
            last: [0.0; 2],
            wow_ph: 0.0,
            fl_ph: 0.0,
            drift: 0.0,
            key: None,
            seed: 22222,
        }
    }

    fn update(&mut self) {
        let key = (self.p.bump, self.p.tone);
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        for c in 0..2 {
            self.pre[c].set(BiqKind::HighShelf, 3000.0, 0.7, 6.0, self.sr);
            self.de[c].set(BiqKind::HighShelf, 3000.0, 0.7, -6.0, self.sr);
            self.bump_f[c].set(BiqKind::Peak, 70.0, 0.9, self.p.bump, self.sr);
            self.tone_f[c].set(BiqKind::LowPass, self.p.tone, 0.6, 0.0, self.sr);
            // anti-alias for the 2x oversampled saturator (runs at 2*sr)
            self.aa[c].set(BiqKind::LowPass, self.sr * 0.45, 0.7, 0.0, self.sr * 2.0);
        }
    }

    /// Asymmetric tanh, DC-corrected so silence stays silence.
    fn sat(x: f64, k: f64, bias: f64) -> f64 {
        ((k * (x + bias)).tanh() - (k * bias).tanh()) / k
    }
}

impl StereoEffect for Tape {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        self.update();
        let p = self.p;
        let sr = self.sr;
        let k = db_to_lin(p.drive).max(0.05);
        let bias = p.bias.clamp(0.0, 1.0) * 0.3;
        let out = db_to_lin(p.output);
        let mix = p.mix.clamp(0.0, 1.0);
        let centre = TAPE_LATENCY * sr;
        let wow_depth = p.wow.clamp(0.0, 1.0) * 0.0025 * sr;
        let fl_depth = p.flutter.clamp(0.0, 1.0) * 0.00015 * sr;
        for i in 0..out_l.len() {
            // wow: slow, slightly irregular; flutter: faster and small
            self.wow_ph += (0.6 + self.drift * 0.2) / sr;
            if self.wow_ph >= 1.0 {
                self.wow_ph -= 1.0;
                // the original multiplies in f64 and keeps the low 32 bits
                let x = f64::from(self.seed) * 1103515245.0 + 12345.0;
                self.seed = (x % 4294967296.0) as u32;
                self.drift = (f64::from(self.seed) / 4294967296.0) * 2.0 - 1.0;
            }
            self.fl_ph = (self.fl_ph + 7.3 / sr) % 1.0;
            let m = (TAU * self.wow_ph).sin() * wow_depth + (TAU * self.fl_ph).sin() * fl_depth;
            for c in 0..2 {
                let x = f64::from(if c == 0 { in_l[i] } else { in_r[i] });
                let mut y = self.pre[c].run(x);
                // 2x oversampled saturation: linear interpolation up, filtered, decimated
                let mid = (self.last[c] + y) * 0.5;
                self.last[c] = y;
                self.aa[c].run(Self::sat(mid, k, bias));
                y = self.aa[c].run(Self::sat(y, k, bias));
                y = self.de[c].run(y);
                y = self.bump_f[c].run(y);
                y = self.tone_f[c].run(y);
                self.dl[c].write(y);
                let wet = self.dl[c].read(centre + m) * out;
                // the dry path gets the same fixed delay so mixing never comb-filters
                self.dry_lines[c].write(x);
                let v = wet * mix + self.dry_lines[c].read(centre) * (1.0 - mix);
                if c == 0 {
                    out_l[i] = v as f32;
                } else {
                    out_r[i] = v as f32;
                }
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }

    fn latency(&self) -> f64 {
        (TAPE_LATENCY * self.sr).round() / self.sr
    }
}

// ---------------------------------------------------------------------------
// Frequency shifter
// ---------------------------------------------------------------------------
const HIL_A: [f64; 4] = [0.6923878, 0.9360654322959, 0.988229522686, 0.9987488452737];
const HIL_B: [f64; 4] = [
    0.4021921162426,
    0.856171088242,
    0.9722909545651,
    0.9952884791278,
];

/// 90-degree phase splitter (two allpass chains, Olli Niemitalo's coefficients).
#[derive(Default)]
struct Hilbert {
    sa: [[f64; 4]; 4],
    sb: [[f64; 4]; 4],
    a_delay: f64,
    re: f64,
    im: f64,
}

impl Hilbert {
    fn chain(mut x: f64, coefs: &[f64; 4], st: &mut [[f64; 4]; 4]) -> f64 {
        for k in 0..4 {
            let a2 = coefs[k] * coefs[k];
            let s = &mut st[k];
            // y[n] = a^2 (x[n] + y[n-2]) - x[n-2]
            let y = a2 * (x + s[3]) - s[1];
            s[1] = s[0];
            s[0] = x;
            s[3] = s[2];
            s[2] = flush(y);
            x = y;
        }
        x
    }

    /// In-phase and quadrature outputs land in `re` and `im`. The in-phase path
    /// carries a one-sample delay.
    fn run(&mut self, x: f64) {
        self.re = self.a_delay;
        self.a_delay = Self::chain(x, &HIL_A, &mut self.sa);
        self.im = Self::chain(x, &HIL_B, &mut self.sb);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FreqShiftParams {
    /// Shift in Hz (negative is down).
    pub shift: f64,
    /// Stereo spread: the right channel shifts by -shift * spread.
    pub spread: f64,
    pub feedback: f64,
    pub mix: f64,
}

impl Default for FreqShiftParams {
    fn default() -> Self {
        Self {
            shift: 0.0,
            spread: 0.0,
            feedback: 0.0,
            mix: 1.0,
        }
    }
}

/// Single-sideband frequency shifter (Bode style).
pub struct FrequencyShifter {
    pub p: FreqShiftParams,
    sr: f64,
    h: [Hilbert; 2],
    ph: [f64; 2],
    fb: [f64; 2],
}

impl FrequencyShifter {
    pub fn new(sr: f64) -> Self {
        Self {
            p: FreqShiftParams::default(),
            sr,
            h: [Hilbert::default(), Hilbert::default()],
            ph: [0.0; 2],
            fb: [0.0; 2],
        }
    }
}

impl StereoEffect for FrequencyShifter {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let p = self.p;
        let shifts = [p.shift, p.shift * (1.0 - 2.0 * p.spread.clamp(0.0, 1.0))];
        let fb_amt = p.feedback.clamp(0.0, 0.95);
        let mix = p.mix.clamp(0.0, 1.0);
        for c in 0..2 {
            let inc = shifts[c] / self.sr;
            let (inp, out) = if c == 0 {
                (in_l, &mut *out_l)
            } else {
                (in_r, &mut *out_r)
            };
            for i in 0..out.len() {
                let x = f64::from(inp[i]);
                let h = &mut self.h[c];
                h.run(x + self.fb[c] * fb_amt);
                let a = TAU * self.ph[c];
                // upper sideband for positive shifts
                let y = h.re * a.cos() + h.im * a.sin();
                self.ph[c] += inc;
                self.ph[c] -= self.ph[c].floor();
                self.fb[c] = flush(y);
                out[i] = (x * (1.0 - mix) + y * mix) as f32;
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Pitch shifter
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PitchShiftParams {
    /// Semitones, -24 to 24.
    pub semitones: f64,
    pub cents: f64,
    /// Grain window, ms.
    pub window: f64,
    pub mix: f64,
}

impl Default for PitchShiftParams {
    fn default() -> Self {
        Self {
            semitones: 0.0,
            cents: 0.0,
            window: 60.0,
            mix: 1.0,
        }
    }
}

/// Average delay of the pitch shifter at a setting, seconds: the fixed minimum
/// read delay plus, when shifting, half a grain times the ratio difference.
pub fn pitch_latency(p: &PitchShiftParams, sr: f64) -> f64 {
    let ratio = 2f64.powf((p.semitones.clamp(-24.0, 24.0) + p.cents / 100.0) / 12.0);
    let l = p.window.clamp(10.0, 200.0) * 0.001 * sr;
    let dmin = (sr * 0.003).ceil() + 160.0;
    // whole samples, so delay matching elsewhere needs no interpolation
    crate::engine::js_round(
        dmin + (ratio - 1.0).max(0.0) * l * 0.5 + (1.0 - ratio).max(0.0) * l * 0.5,
    ) / sr
}

#[derive(Clone, Copy, Default)]
struct Tap {
    age: f64,
    delay: f64,
    len: f64,
}

/// Granular (SOLA style) pitch shifter. Two overlapping grains read the input at
/// the pitch ratio with sin^2 cross-fades. When a grain restarts, its start
/// point is chosen (within one pitch period) to best match the waveform of the
/// grain it fades into.
pub struct PitchShifter {
    pub p: PitchShiftParams,
    sr: f64,
    dl: [DelayLine; 2],
    mono: DelayLine,
    taps: [Tap; 2],
    started: bool,
    /// Minimum read delay: leaves room for the correlation look-ahead.
    dmin: f64,
    search: f64,
}

impl PitchShifter {
    pub fn new(sr: f64) -> Self {
        let size = (sr * 1.4).ceil() as usize;
        Self {
            p: PitchShiftParams::default(),
            sr,
            dl: [DelayLine::new(size), DelayLine::new(size)],
            mono: DelayLine::new(size),
            taps: [Tap {
                age: 0.0,
                delay: 0.0,
                len: 1.0,
            }; 2],
            started: false,
            dmin: (sr * 0.003).ceil() + 160.0,
            search: (sr * 0.016).ceil(),
        }
    }

    fn ratio(&self) -> f64 {
        2f64.powf((self.p.semitones.clamp(-24.0, 24.0) + self.p.cents / 100.0) / 12.0)
    }

    fn grain(&self) -> f64 {
        self.p.window.clamp(10.0, 200.0) * 0.001 * self.sr
    }

    fn restart(&mut self, k: usize, ratio: f64, l: f64) {
        let other = self.taps[1 - k];
        let base = self.dmin + (ratio - 1.0).max(0.0) * l;
        let mut best = 0.0;
        if self.started {
            // pick the start offset whose waveform best continues the other grain
            let m = 96.0;
            let mut best_score = f64::NEG_INFINITY;
            let mut d = 0.0;
            while d <= self.search {
                let (mut dot, mut e) = (0.0, 0.0);
                let mut j = 0.0;
                while j < m {
                    let a = self.mono.read(base + d - j);
                    let b = self.mono.read(other.delay - j);
                    dot += a * b;
                    e += a * a;
                    j += 2.0;
                }
                let score = if e > 1e-12 { dot / e.sqrt() } else { 0.0 };
                if score > best_score {
                    best_score = score;
                    best = d;
                }
                d += 2.0;
            }
        }
        let t = &mut self.taps[k];
        t.age = 0.0;
        t.len = l;
        t.delay = base + best;
    }
}

impl StereoEffect for PitchShifter {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let ratio = self.ratio();
        let l = self.grain();
        let speed = 1.0 - ratio;
        let mix = self.p.mix.clamp(0.0, 1.0);
        // the dry signal is delayed by the average wet delay so the mix stays aligned
        let dry_d = pitch_latency(&self.p, self.sr) * self.sr;
        if !self.started {
            self.restart(0, ratio, l);
            self.restart(1, ratio, l);
            self.taps[1].age = l / 2.0;
            self.taps[1].delay += speed * (l / 2.0);
        }
        for i in 0..out_l.len() {
            let xl = f64::from(in_l[i]);
            let xr = f64::from(in_r[i]);
            self.dl[0].write(xl);
            self.dl[1].write(xr);
            self.mono.write(xl + xr);
            self.started = true;
            let (mut yl, mut yr) = (0.0, 0.0);
            for k in 0..2 {
                if self.taps[k].age >= self.taps[k].len {
                    self.restart(k, ratio, l);
                }
                let t = self.taps[k];
                let s = (std::f64::consts::PI * t.age / t.len).sin();
                let w = s * s;
                yl += w * self.dl[0].read(t.delay);
                yr += w * self.dl[1].read(t.delay);
                self.taps[k].delay += speed;
                self.taps[k].age += 1.0;
            }
            if mix < 1.0 {
                out_l[i] = (self.dl[0].read(dry_d) * (1.0 - mix) + yl * mix) as f32;
                out_r[i] = (self.dl[1].read(dry_d) * (1.0 - mix) + yr * mix) as f32;
            } else {
                out_l[i] = yl as f32;
                out_r[i] = yr as f32;
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }

    fn latency(&self) -> f64 {
        pitch_latency(&self.p, self.sr)
    }
}
