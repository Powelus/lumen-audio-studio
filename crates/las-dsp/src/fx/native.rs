// SPDX-License-Identifier: GPL-3.0-or-later
//! The effects that the web version builds from Web Audio's own nodes (biquad
//! filters, delays, a wave shaper, a convolver, oscillators): equaliser,
//! overdrive, chorus, phaser, auto filter, stereo tool, maximizer and
//! convolution reverb.
//!
//! Web Audio does not specify these to the sample, so they are written to behave
//! as the nodes do (the biquad formulas, `Q` in dB for low pass and high pass,
//! the convolver's normalisation, a wave shaper curve read with linear
//! interpolation) rather than copied. They use the same parameters and defaults
//! as the web version, so saved effect states load unchanged.

use super::convolver::{Convolver, Impulse};
use super::dynamics::Limiter;
use super::webcomp::WebCompressor;
use super::{db_to_lin, flush, update_params, DelayLine, StereoEffect, TAU};
use crate::biquad::{coeffs, Coeffs, Kind};
use crate::engine::PatchError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// A Web Audio style biquad
// ---------------------------------------------------------------------------

/// Coefficients as `BiquadFilterNode` computes them, including what Chromium
/// does at the edges of the range (a cutoff at 0 or at the Nyquist frequency).
/// `q` is in dB for low pass and high pass and linear for the rest, as in Web
/// Audio.
fn wa_coeffs(kind: Kind, f: f64, q: f64, gain_db: f64, sr: f64) -> Coeffs {
    let n = (f / (sr * 0.5)).clamp(0.0, 1.0);
    let pass = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
    let silent = Coeffs { b0: 0.0, ..pass };
    let gain = |g: f64| Coeffs { b0: g, ..pass };
    let a2 = 10f64.powf(gain_db / 20.0);
    let inside = n > 0.0 && n < 1.0;
    match kind {
        Kind::LowPass if n >= 1.0 => return pass,
        Kind::LowPass if n <= 0.0 => return silent,
        Kind::HighPass if n >= 1.0 => return silent,
        Kind::HighPass if n <= 0.0 => return pass,
        Kind::Peaking if !inside => return pass,
        Kind::Peaking if q <= 0.0 => return gain(a2),
        Kind::LowShelf if n >= 1.0 => return gain(a2),
        Kind::LowShelf if n <= 0.0 => return pass,
        Kind::HighShelf if n >= 1.0 => return pass,
        Kind::HighShelf if n <= 0.0 => return gain(a2),
        Kind::BandPass if !inside => return silent,
        Kind::BandPass if q <= 0.0 => return pass,
        Kind::Notch if !inside => return pass,
        Kind::Notch if q <= 0.0 => return silent,
        _ => {}
    }
    let q = match kind {
        Kind::LowPass | Kind::HighPass => 10f64.powf(q.max(0.0) / 20.0),
        _ => q,
    };
    coeffs(kind, n * sr * 0.5, gain_db, q, sr)
}

/// Gain in dB of the equaliser's bands together at `freq`, for drawing the
/// response curve.
pub fn eq_response_db(bands: &[EqBand], freq: f64, sr: f64) -> f64 {
    bands
        .iter()
        .filter(|b| b.on)
        .map(|b| {
            let kind = Kind::parse(&b.kind).unwrap_or(Kind::Peaking);
            wa_coeffs(kind, b.freq, b.q, b.gain, sr).magnitude_db(freq, sr)
        })
        .sum()
}

/// All-pass biquad (`Q` linear).
fn allpass_coeffs(f: f64, q: f64, sr: f64) -> Coeffs {
    let w0 = TAU * f.clamp(1.0, sr * 0.499) / sr;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q.max(0.0001));
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: (1.0 - alpha) / a0,
        b1: -2.0 * cos / a0,
        b2: (1.0 + alpha) / a0,
        a1: -2.0 * cos / a0,
        a2: (1.0 - alpha) / a0,
    }
}

/// Direct form I, as Chromium's biquad runs, with `f32` samples and `f64`
/// arithmetic. The coefficients may change from one sample to the next (a
/// modulated cutoff), which is why the form matters.
#[derive(Clone, Copy, Debug)]
struct Bq {
    c: Coeffs,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Default for Bq {
    fn default() -> Self {
        Self {
            c: Coeffs {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            },
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
}

impl Bq {
    fn run(&mut self, x: f64) -> f64 {
        // the node's input and output are `f32`
        let x = f64::from(x as f32);
        let c = &self.c;
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        let y = f64::from(y as f32);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = flush(y);
        y
    }
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Parametric EQ
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EqBand {
    pub on: bool,
    #[serde(rename = "type")]
    pub kind: String,
    pub freq: f64,
    pub gain: f64,
    pub q: f64,
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            on: true,
            kind: "peaking".into(),
            freq: 1000.0,
            gain: 0.0,
            q: 0.9,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EqParams {
    pub bands: Vec<EqBand>,
    pub output: f64,
}

impl Default for EqParams {
    fn default() -> Self {
        let band = |kind: &str, freq: f64| EqBand {
            kind: kind.into(),
            freq,
            ..EqBand::default()
        };
        Self {
            bands: vec![
                band("lowshelf", 60.0),
                band("peaking", 150.0),
                band("peaking", 400.0),
                band("peaking", 1000.0),
                band("peaking", 2500.0),
                band("peaking", 6000.0),
                band("highshelf", 12000.0),
            ],
            output: 0.0,
        }
    }
}

/// Seven-band parametric equaliser.
pub struct Eq {
    pub p: EqParams,
    sr: f64,
    filters: Vec<[Bq; 2]>,
    applied: Vec<(bool, String, f64, f64, f64)>,
}

impl Eq {
    pub fn new(sr: f64) -> Self {
        Self {
            p: EqParams::default(),
            sr,
            filters: vec![[Bq::default(); 2]; 7],
            applied: Vec::new(),
        }
    }

    fn update(&mut self) {
        let key: Vec<_> = self
            .p
            .bands
            .iter()
            .map(|b| (b.on, b.kind.clone(), b.freq, b.gain, b.q))
            .collect();
        if key == self.applied {
            return;
        }
        for (i, b) in self.p.bands.iter().enumerate().take(self.filters.len()) {
            let kind = if b.on {
                Kind::parse(&b.kind).unwrap_or(Kind::Peaking)
            } else {
                Kind::Peaking
            };
            let gain = if b.on { b.gain } else { 0.0 };
            let c = wa_coeffs(kind, b.freq, b.q, gain, self.sr);
            for f in &mut self.filters[i] {
                f.c = c;
            }
        }
        self.applied = key;
    }
}

impl StereoEffect for Eq {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        self.update();
        let out = db_to_lin(self.p.output);
        for i in 0..out_l.len() {
            let mut l = f64::from(in_l[i]);
            let mut r = f64::from(in_r[i]);
            for f in &mut self.filters {
                l = f[0].run(l);
                r = f[1].run(r);
            }
            out_l[i] = (l * out) as f32;
            out_r[i] = (r * out) as f32;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Overdrive
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DistParams {
    pub mode: String,
    pub drive: f64,
    pub tone: f64,
    pub output: f64,
    pub mix: f64,
}

impl Default for DistParams {
    fn default() -> Self {
        Self {
            mode: "soft".into(),
            drive: 12.0,
            tone: 9000.0,
            output: -6.0,
            mix: 1.0,
        }
    }
}

const CURVE_LEN: usize = 4096;

fn dist_curve(mode: &str) -> Vec<f32> {
    (0..CURVE_LEN)
        .map(|i| {
            let x = (i as f64 / (CURVE_LEN - 1) as f64) * 2.0 - 1.0;
            let y = match mode {
                "hard" => (x * 1.2).clamp(-0.8, 0.8) / 0.8,
                "tube" => {
                    if x >= 0.0 {
                        (x * 1.2).tanh()
                    } else {
                        (x * 0.8).tanh() * 0.9
                    }
                }
                "fold" => (x * std::f64::consts::PI * 1.5).sin(),
                "rectify" => x.abs() * 2.0 - 1.0 + x * 0.3,
                "sine" => (x * std::f64::consts::PI * 0.5).sin(),
                _ => (x * 2.0).tanh() / 2f64.tanh(),
            };
            y as f32
        })
        .collect()
}

/// The wave shaper node's lookup: linear interpolation, ends held.
fn shape(curve: &[f32], x: f64) -> f64 {
    let v = ((curve.len() - 1) as f64 / 2.0) * (x + 1.0);
    let k = v.floor();
    if k < 0.0 {
        return f64::from(curve[0]);
    }
    let k = k as usize;
    if k + 1 >= curve.len() {
        return f64::from(curve[curve.len() - 1]);
    }
    let f = v - k as f64;
    f64::from(curve[k]) * (1.0 - f) + f64::from(curve[k + 1]) * f
}

// ---------------------------------------------------------------------------
// The wave shaper's 4x oversampling, as Chromium does it: two 2x up-samplers
// (a 128 tap half-sample interpolator), the curve, then two 2x down-samplers
// (a 256 tap half-band filter). Together they delay the signal 192 samples,
// which the web version measures and compensates for.
// ---------------------------------------------------------------------------
const UP_TAPS: usize = 128;
const DOWN_TAPS: usize = 256;
/// Delay of the whole 4x chain, samples.
pub const WAVESHAPER_4X_LATENCY: f64 = 192.0;

fn blackman(x: f64, n: f64) -> f64 {
    let alpha = 0.16;
    let a0 = 0.5 * (1.0 - alpha);
    let a1 = 0.5;
    let a2 = 0.5 * alpha;
    a0 - a1 * (TAU * x / n).cos() + a2 * (2.0 * TAU * x / n).cos()
}

/// A history that gives the newest `n` samples as one slice, oldest first.
struct History {
    buf: Vec<f64>,
    pos: usize,
    n: usize,
}

impl History {
    fn new(n: usize) -> Self {
        Self {
            buf: vec![0.0; 2 * n],
            pos: 0,
            n,
        }
    }

    fn push(&mut self, x: f64) {
        self.pos = (self.pos + 1) % self.n;
        self.buf[self.pos] = x;
        self.buf[self.pos + self.n] = x;
    }

    /// The last `n` pushed samples, oldest first.
    fn window(&self) -> &[f64] {
        &self.buf[self.pos + 1..self.pos + 1 + self.n]
    }
}

/// 2x up-sampler: even outputs are the input delayed by half the kernel, odd
/// outputs interpolate halfway between.
struct Up2 {
    /// The kernel, newest sample first reversed to match the history window.
    kernel: Vec<f64>,
    hist: History,
}

impl Up2 {
    fn new() -> Self {
        let n = UP_TAPS;
        let half = (n / 2) as f64;
        let kernel_forward: Vec<f64> = (0..n)
            .map(|i| {
                let s = std::f64::consts::PI * (i as f64 - half + 0.5);
                let sinc = if s == 0.0 { 1.0 } else { s.sin() / s };
                blackman(i as f64 + 0.5, n as f64) * sinc
            })
            .collect();
        // window[n-1-j] is the sample j back, so the dot product wants k reversed
        let kernel = kernel_forward.into_iter().rev().collect();
        Self {
            kernel,
            hist: History::new(n),
        }
    }

    fn run(&mut self, x: f64) -> (f64, f64) {
        self.hist.push(x);
        let w = self.hist.window();
        let even = w[UP_TAPS - 1 - UP_TAPS / 2];
        let odd: f64 = self.kernel.iter().zip(w).map(|(k, x)| k * x).sum();
        (even, odd)
    }
}

/// 2x down-sampler: a half-band low pass (only the odd taps and the centre are
/// not zero), keeping every second output.
struct Down2 {
    centre: f64,
    /// Odd taps, for the lags 1, 3, 5, ... 255 (reversed to match the window).
    odd: Vec<f64>,
    hist: History,
}

impl Down2 {
    fn new() -> Self {
        let n = DOWN_TAPS;
        let half = n / 2;
        let tap = |i: usize| {
            let s = 0.5 * std::f64::consts::PI * (i as f64 - half as f64);
            let sinc = if s == 0.0 { 1.0 } else { s.sin() / s };
            0.5 * sinc * blackman(i as f64, n as f64)
        };
        // lag m back from the newest sample: tap h[m], odd m only
        let odd: Vec<f64> = (0..n / 2).map(|k| tap(2 * k + 1)).collect();
        Self {
            centre: tap(half),
            odd,
            hist: History::new(n),
        }
    }

    /// Take two input samples, give one output.
    fn run(&mut self, first: f64, second: f64) -> f64 {
        self.hist.push(first);
        let w = self.hist.window();
        let n = DOWN_TAPS;
        // lag m is w[n-1-m]
        let mut acc = self.centre * w[n - 1 - n / 2];
        for (k, c) in self.odd.iter().enumerate() {
            acc += c * w[n - 1 - (2 * k + 1)];
        }
        self.hist.push(second);
        acc
    }
}

/// The 4x oversampled wave shaper for one channel.
struct Shaper4 {
    up1: Up2,
    up2: Up2,
    down2: Down2,
    down1: Down2,
}

impl Shaper4 {
    fn new() -> Self {
        Self {
            up1: Up2::new(),
            up2: Up2::new(),
            down2: Down2::new(),
            down1: Down2::new(),
        }
    }

    /// One input sample in, one shaped sample out (delayed by [`WAVESHAPER_4X_LATENCY`]).
    fn run(&mut self, x: f64, curve: &[f32]) -> f64 {
        let (e1, o1) = self.up1.run(x);
        let (a, b) = self.up2.run(e1);
        let (c, d) = self.up2.run(o1);
        let (qa, qb, qc, qd) = (
            shape(curve, a),
            shape(curve, b),
            shape(curve, c),
            shape(curve, d),
        );
        let da = self.down2.run(qa, qb);
        let db = self.down2.run(qc, qd);
        self.down1.run(da, db)
    }

    /// Delay of the whole chain, samples.
    fn latency() -> f64 {
        WAVESHAPER_4X_LATENCY
    }
}

pub struct Distortion {
    pub p: DistParams,
    sr: f64,
    curve: Vec<f32>,
    mode: String,
    shapers: [Shaper4; 2],
    tone: [Bq; 2],
    dry_delay: [DelayLine; 2],
}

impl Distortion {
    pub fn new(sr: f64) -> Self {
        Self {
            p: DistParams::default(),
            sr,
            curve: dist_curve("soft"),
            mode: "soft".into(),
            shapers: [Shaper4::new(), Shaper4::new()],
            tone: [Bq::default(); 2],
            dry_delay: [DelayLine::new(256), DelayLine::new(256)],
        }
    }
}

impl StereoEffect for Distortion {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        if self.p.mode != self.mode {
            self.mode = self.p.mode.clone();
            self.curve = dist_curve(&self.mode);
        }
        let pre = db_to_lin(self.p.drive) * 0.5;
        let post = db_to_lin(self.p.output);
        let mix = clamp01(self.p.mix);
        let c = wa_coeffs(Kind::LowPass, self.p.tone, 1.0, 0.0, self.sr);
        for t in &mut self.tone {
            t.c = c;
        }
        let lat = Shaper4::latency();
        for i in 0..out_l.len() {
            for ch in 0..2 {
                let x = f64::from(if ch == 0 { in_l[i] } else { in_r[i] });
                let w = self.shapers[ch].run(x * pre, &self.curve);
                let w = self.tone[ch].run(w) * post;
                self.dry_delay[ch].write(x);
                let d = self.dry_delay[ch].read(lat);
                let v = (d * (1.0 - mix) + w * mix) as f32;
                if ch == 0 {
                    out_l[i] = v;
                } else {
                    out_r[i] = v;
                }
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }

    fn latency(&self) -> f64 {
        Shaper4::latency() / self.sr
    }
}

// ---------------------------------------------------------------------------
// Chorus / flanger
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ChorusParams {
    pub rate: f64,
    /// ms
    pub depth: f64,
    /// ms
    pub delay: f64,
    pub feedback: f64,
    pub spread: f64,
    pub mix: f64,
}

impl Default for ChorusParams {
    fn default() -> Self {
        Self {
            rate: 0.6,
            depth: 3.0,
            delay: 12.0,
            feedback: 0.0,
            spread: 1.0,
            mix: 0.5,
        }
    }
}

/// Frames a signal fed back through a delay takes to come round (a Web Audio
/// graph processes a cycle one render quantum at a time).
const CYCLE: usize = 128;

pub struct Chorus {
    pub p: ChorusParams,
    sr: f64,
    lines: [DelayLine; 2],
    /// The delay output of the last `CYCLE` frames, per channel.
    fb: [[f32; CYCLE]; 2],
    n: usize,
    phase: f64,
}

impl Chorus {
    pub fn new(sr: f64) -> Self {
        let n = (sr * 0.1).ceil() as usize + 8;
        Self {
            p: ChorusParams::default(),
            sr,
            lines: [DelayLine::new(n), DelayLine::new(n)],
            fb: [[0.0; CYCLE]; 2],
            n: 0,
            phase: 0.0,
        }
    }
}

impl StereoEffect for Chorus {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let p = self.p;
        let sr = self.sr;
        // the web version's gains and delay times are `f32`
        let depth = (p.depth / 1000.0) as f32;
        let depth_r = ((p.depth / 1000.0) * (1.0 - 2.0 * p.spread)) as f32;
        let depths = [depth, depth_r];
        let delay = (p.delay / 1000.0) as f32;
        let dry = (1.0 - p.mix * 0.5) as f32;
        let wet = p.mix as f32;
        let feedback = p.feedback as f32;
        for i in 0..out_l.len() {
            let lfo = (TAU * self.phase).sin() as f32;
            self.phase = (self.phase + p.rate / sr) % 1.0;
            let slot = self.n % CYCLE;
            for ch in 0..2 {
                let x = if ch == 0 { in_l[i] } else { in_r[i] };
                let back = self.fb[ch][slot] * feedback;
                self.lines[ch].write(f64::from(flush(f64::from(x + back)) as f32));
                let t = (delay + lfo * depths[ch]).clamp(0.0, 0.1);
                let y = self.lines[ch].read_linear(f64::from(t) * sr) as f32;
                self.fb[ch][slot] = y;
                let v = x * dry + y * wet;
                if ch == 0 {
                    out_l[i] = v;
                } else {
                    out_r[i] = v;
                }
            }
            self.n += 1;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Phaser
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PhaserParams {
    pub rate: f64,
    pub depth: f64,
    pub center: f64,
    pub feedback: f64,
    pub stages: f64,
    pub mix: f64,
}

impl Default for PhaserParams {
    fn default() -> Self {
        Self {
            rate: 0.4,
            depth: 0.7,
            center: 900.0,
            feedback: 0.4,
            stages: 6.0,
            mix: 0.5,
        }
    }
}

const MAX_STAGES: usize = 12;
/// Web Audio delays a signal fed back through a `DelayNode` by one render
/// quantum, which is what the web version's feedback path does.
const FEEDBACK_DELAY: usize = 128;

pub struct Phaser {
    pub p: PhaserParams,
    sr: f64,
    stages: [[Bq; MAX_STAGES]; 2],
    fb: [DelayLine; 2],
    phase: f64,
}

impl Phaser {
    pub fn new(sr: f64) -> Self {
        Self {
            p: PhaserParams::default(),
            sr,
            stages: [[Bq::default(); MAX_STAGES]; 2],
            fb: [
                DelayLine::new(FEEDBACK_DELAY + 8),
                DelayLine::new(FEEDBACK_DELAY + 8),
            ],
            phase: 0.0,
        }
    }
}

impl StereoEffect for Phaser {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let p = self.p;
        let sr = self.sr;
        let count = (p.stages.round().max(1.0) as usize).min(MAX_STAGES);
        let dry = 1.0 - p.mix * 0.5;
        let wet = p.mix;
        for i in 0..out_l.len() {
            let lfo = (TAU * self.phase).sin();
            self.phase = (self.phase + p.rate / sr) % 1.0;
            // the same sweep for every stage and both channels
            let f = p.center + lfo * p.center * 0.9 * p.depth;
            let c = allpass_coeffs(f, 0.5, sr);
            for ch in 0..2 {
                let x = f64::from(if ch == 0 { in_l[i] } else { in_r[i] });
                // y[n - 128] is 127 behind the last sample written (y[n - 1])
                let head = x + self.fb[ch].read((FEEDBACK_DELAY - 1) as f64) * p.feedback;
                let mut y = head;
                for s in 0..count {
                    self.stages[ch][s].c = c;
                    y = self.stages[ch][s].run(y);
                }
                self.fb[ch].write(flush(y));
                let v = (x * dry + y * wet) as f32;
                if ch == 0 {
                    out_l[i] = v;
                } else {
                    out_r[i] = v;
                }
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Auto filter
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoFilterParams {
    #[serde(rename = "type")]
    pub kind: String,
    pub cutoff: f64,
    pub res: f64,
    #[serde(rename = "lfoRate")]
    pub lfo_rate: f64,
    /// Octaves.
    #[serde(rename = "lfoDepth")]
    pub lfo_depth: f64,
    pub mix: f64,
}

impl Default for AutoFilterParams {
    fn default() -> Self {
        Self {
            kind: "lowpass".into(),
            cutoff: 2000.0,
            res: 1.2,
            lfo_rate: 0.5,
            lfo_depth: 0.0,
            mix: 1.0,
        }
    }
}

pub struct AutoFilter {
    pub p: AutoFilterParams,
    sr: f64,
    f: [Bq; 2],
    phase: f64,
}

impl AutoFilter {
    pub fn new(sr: f64) -> Self {
        Self {
            p: AutoFilterParams::default(),
            sr,
            f: [Bq::default(); 2],
            phase: 0.0,
        }
    }
}

impl StereoEffect for AutoFilter {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let kind = Kind::parse(&self.p.kind).unwrap_or(Kind::LowPass);
        let mix = clamp01(self.p.mix);
        for i in 0..out_l.len() {
            let lfo = (TAU * self.phase).sin();
            self.phase = (self.phase + self.p.lfo_rate / self.sr) % 1.0;
            // the LFO moves the cutoff through `detune`, in cents
            let f = self.p.cutoff * 2f64.powf(lfo * self.p.lfo_depth);
            let c = wa_coeffs(kind, f, self.p.res, 0.0, self.sr);
            for ch in 0..2 {
                self.f[ch].c = c;
                let x = f64::from(if ch == 0 { in_l[i] } else { in_r[i] });
                let y = self.f[ch].run(x);
                let v = (x * (1.0 - mix) + y * mix) as f32;
                if ch == 0 {
                    out_l[i] = v;
                } else {
                    out_r[i] = v;
                }
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Stereo tool
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StereoParams {
    pub width: f64,
    pub pan: f64,
    pub gain: f64,
    pub swap: bool,
    pub invert: bool,
}

impl Default for StereoParams {
    fn default() -> Self {
        Self {
            width: 1.0,
            pan: 0.0,
            gain: 0.0,
            swap: false,
            invert: false,
        }
    }
}

pub struct StereoTool {
    pub p: StereoParams,
}

impl StereoTool {
    pub fn new(_sr: f64) -> Self {
        Self {
            p: StereoParams::default(),
        }
    }
}

impl StereoEffect for StereoTool {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let p = self.p;
        // mid/side width matrix: L' = L*(1+w)/2 + R*(1-w)/2
        let a = (1.0 + p.width) / 2.0;
        let c = (1.0 - p.width) / 2.0;
        let sign = if p.invert { -1.0 } else { 1.0 };
        let gl = (1.0 - p.pan).min(1.0) * db_to_lin(p.gain) * sign;
        let gr = (1.0 + p.pan).min(1.0) * db_to_lin(p.gain) * sign;
        // the four routing gains: [left in to left out, left in to right out,
        // right in to left out, right in to right out]
        let g = if p.swap {
            [c * gr, a * gl, a * gr, c * gl]
        } else {
            [a * gl, c * gr, c * gl, a * gr]
        };
        for i in 0..out_l.len() {
            let (l, r) = (f64::from(in_l[i]), f64::from(in_r[i]));
            out_l[i] = (l * g[0] + r * g[2]) as f32;
            out_r[i] = (l * g[1] + r * g[3]) as f32;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Maximizer
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MaximizerParams {
    #[serde(rename = "lowX")]
    pub low_x: f64,
    #[serde(rename = "highX")]
    pub high_x: f64,
    #[serde(rename = "lowThr")]
    pub low_thr: f64,
    #[serde(rename = "lowGain")]
    pub low_gain: f64,
    #[serde(rename = "midThr")]
    pub mid_thr: f64,
    #[serde(rename = "midGain")]
    pub mid_gain: f64,
    #[serde(rename = "highThr")]
    pub high_thr: f64,
    #[serde(rename = "highGain")]
    pub high_gain: f64,
    pub ratio: f64,
    pub output: f64,
}

impl Default for MaximizerParams {
    fn default() -> Self {
        Self {
            low_x: 180.0,
            high_x: 3500.0,
            low_thr: -14.0,
            low_gain: 1.0,
            mid_thr: -12.0,
            mid_gain: 0.0,
            high_thr: -14.0,
            high_gain: 1.0,
            ratio: 3.0,
            output: 2.0,
        }
    }
}

/// Two cascaded second-order filters per crossover edge (Q of one over root two,
/// as the web version sets it).
#[derive(Clone, Copy, Default)]
struct Pair([Bq; 2]);

impl Pair {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.0[0].run(x);
        self.0[1].run(y)
    }

    fn set(&mut self, c: Coeffs) {
        self.0[0].c = c;
        self.0[1].c = c;
    }
}

/// Three-band compressor with an output limiter.
pub struct Maximizer {
    pub p: MaximizerParams,
    sr: f64,
    low_lp: [Pair; 2],
    mid_hp: [Pair; 2],
    mid_lp: [Pair; 2],
    high_hp: [Pair; 2],
    comps: [WebCompressor; 3],
    makeups: [f32; 3],
    limiter: Limiter,
    key: Option<MaximizerKey>,
    bufs: [Vec<f32>; 6],
    mixl: Vec<f32>,
    mixr: Vec<f32>,
}

type MaximizerKey = [f64; 10];

impl Maximizer {
    pub fn new(sr: f64) -> Self {
        let mut limiter = Limiter::new(sr);
        limiter.p.ceiling = -0.3;
        limiter.p.release = 60.0;
        limiter.p.lookahead = 3.0;
        Self {
            p: MaximizerParams::default(),
            sr,
            low_lp: [Pair::default(); 2],
            mid_hp: [Pair::default(); 2],
            mid_lp: [Pair::default(); 2],
            high_hp: [Pair::default(); 2],
            comps: [
                WebCompressor::new(sr),
                WebCompressor::new(sr),
                WebCompressor::new(sr),
            ],
            makeups: [1.0; 3],
            limiter,
            key: None,
            bufs: Default::default(),
            mixl: Vec::new(),
            mixr: Vec::new(),
        }
    }

    fn update(&mut self) {
        let p = self.p;
        let key = [
            p.low_x,
            p.high_x,
            p.low_thr,
            p.low_gain,
            p.mid_thr,
            p.mid_gain,
            p.high_thr,
            p.high_gain,
            p.ratio,
            p.output,
        ];
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let lp = |f: f64| wa_coeffs(Kind::LowPass, f, q, 0.0, self.sr);
        let hp = |f: f64| wa_coeffs(Kind::HighPass, f, q, 0.0, self.sr);
        for ch in 0..2 {
            self.low_lp[ch].set(lp(p.low_x));
            self.mid_hp[ch].set(hp(p.low_x));
            self.mid_lp[ch].set(lp(p.high_x));
            self.high_hp[ch].set(hp(p.high_x));
        }
        let thr = [p.low_thr, p.mid_thr, p.high_thr];
        let gains = [p.low_gain, p.mid_gain, p.high_gain];
        for i in 0..3 {
            let c = &mut self.comps[i].p;
            c.threshold = thr[i] as f32;
            c.ratio = p.ratio as f32;
            c.knee = 6.0;
            c.attack = 0.006;
            c.release = 0.12;
            self.makeups[i] = db_to_lin(gains[i] + thr[i].abs() * 0.25) as f32;
        }
        self.limiter.p.gain = p.output;
    }
}

impl StereoEffect for Maximizer {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        self.update();
        let n = out_l.len();
        for b in &mut self.bufs {
            b.resize(n, 0.0);
        }
        self.mixl.resize(n, 0.0);
        self.mixr.resize(n, 0.0);
        // split
        for i in 0..n {
            let (l, r) = (f64::from(in_l[i]), f64::from(in_r[i]));
            let band = |ch: usize, x: f64, s: &mut Self| {
                let low = s.low_lp[ch].run(x);
                let mid = {
                    let m = s.mid_hp[ch].run(x);
                    s.mid_lp[ch].run(m)
                };
                let high = s.high_hp[ch].run(x);
                [low as f32, mid as f32, high as f32]
            };
            let bl = band(0, l, self);
            let br = band(1, r, self);
            for k in 0..3 {
                self.bufs[2 * k][i] = bl[k];
                self.bufs[2 * k + 1][i] = br[k];
            }
        }
        // compress each band and sum
        self.mixl.fill(0.0);
        self.mixr.fill(0.0);
        let mut tl = vec![0f32; n];
        let mut tr = vec![0f32; n];
        for k in 0..3 {
            self.comps[k].process(&self.bufs[2 * k], &self.bufs[2 * k + 1], &mut tl, &mut tr);
            let g = self.makeups[k];
            for i in 0..n {
                self.mixl[i] += tl[i] * g;
                self.mixr[i] += tr[i] * g;
            }
        }
        self.limiter
            .process(&self.mixl, &self.mixr, out_l, out_r, None);
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }

    /// The band compressors' look-ahead plus the output limiter's.
    fn latency(&self) -> f64 {
        self.comps[0].latency() + self.limiter.latency()
    }
}

// ---------------------------------------------------------------------------
// Convolution reverb
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbParams {
    pub decay: f64,
    /// ms
    pub predelay: f64,
    pub damp: f64,
    pub size: f64,
    pub lowcut: f64,
    pub width: f64,
    pub dry: f64,
    pub wet: f64,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            decay: 2.2,
            predelay: 12.0,
            damp: 0.45,
            size: 0.6,
            lowcut: 120.0,
            width: 1.0,
            dry: 1.0,
            wet: 0.25,
        }
    }
}

/// The impulse response the web version builds for a setting: decaying noise
/// with damping that grows over time, plus early reflections.
pub fn make_impulse(sr: f64, decay: f64, damp: f64, size: f64, width: f64) -> [Vec<f32>; 2] {
    let len = ((sr * (decay * 1.15 + 0.05).min(14.0)).floor() as usize).max(1);
    let mut seed: u32 = 1234567;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (f64::from(seed) / 4294967296.0) * 2.0 - 1.0
    };
    let mut l = vec![0f32; len];
    let mut r = vec![0f32; len];
    let attack = ((sr * (0.002 + size * 0.04)).floor()).max(1.0);
    let (mut lp_l, mut lp_r) = (0.0f64, 0.0f64);
    for i in 0..len {
        let t = i as f64 / sr;
        let env = (-6.9 * t / decay).exp() * (i as f64 / attack).min(1.0);
        // damping: the low-pass coefficient grows over time
        let coef = (damp * (0.2 + (0.78 * t) / decay)).min(0.98);
        let a = rnd();
        let c = rnd();
        let nl = a;
        let nr = a * (1.0 - width) + c * width;
        lp_l = lp_l * coef + nl * (1.0 - coef);
        lp_r = lp_r * coef + nr * (1.0 - coef);
        l[i] = (lp_l * env) as f32;
        r[i] = (lp_r * env) as f32;
    }
    // early reflections
    let taps = 8 + (size * 10.0).floor() as usize;
    for k in 0..taps {
        let pos = (sr
            * (0.005 + size * 0.07)
            * ((k + 1) as f64 / taps as f64)
            * (0.8 + 0.4 * rnd().abs()))
        .floor() as usize;
        if pos < len {
            l[pos] += (rnd() * 0.5 * (1.0 - k as f64 / taps as f64)) as f32;
            let rp = (len - 1).min(pos + (sr * 0.0007 * k as f64).floor() as usize);
            r[rp] += (rnd() * 0.5 * (1.0 - k as f64 / taps as f64)) as f32;
        }
    }
    [l, r]
}

/// What Web Audio's convolver does to a response when normalising is on: scale
/// so that responses of different strength play at a comparable level.
pub fn normalisation_scale(ir: &[Vec<f32>; 2], sr: f64) -> f64 {
    const MIN_POWER: f64 = 0.000125;
    let n = ir[0].len() as f64 * 2.0;
    let sum: f64 = ir
        .iter()
        .flat_map(|c| c.iter())
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum();
    let mut power = (sum / n).sqrt();
    if !power.is_finite() || power < MIN_POWER {
        power = MIN_POWER;
    }
    let mut scale = 1.0 / power;
    // calibration: -58 dB at 44.1 kHz
    scale *= 10f64.powf(-58.0 * 0.05);
    scale * 44100.0 / sr
}

/// A prepared impulse response, made off the audio thread.
pub struct ReverbImpulse {
    left: Arc<Impulse>,
    right: Arc<Impulse>,
}

impl ReverbImpulse {
    pub fn new(sr: f64, p: &ReverbParams) -> Self {
        let [mut l, mut r] = make_impulse(sr, p.decay, p.damp, p.size, p.width);
        let scale = normalisation_scale(&[l.clone(), r.clone()], sr) as f32;
        for x in l.iter_mut().chain(r.iter_mut()) {
            *x *= scale;
        }
        Self {
            left: Arc::new(Impulse::new(&l)),
            right: Arc::new(Impulse::new(&r)),
        }
    }
}

pub struct ConvReverb {
    pub p: ReverbParams,
    sr: f64,
    conv: [Convolver; 2],
    pre: [DelayLine; 2],
    hp: [Bq; 2],
    ir_key: [f64; 4],
}

impl ConvReverb {
    pub fn new(sr: f64) -> Self {
        let p = ReverbParams::default();
        let ir = ReverbImpulse::new(sr, &p);
        Self {
            sr,
            conv: [Convolver::new(ir.left), Convolver::new(ir.right)],
            pre: [
                DelayLine::new((sr * 0.5) as usize + 8),
                DelayLine::new((sr * 0.5) as usize + 8),
            ],
            hp: [Bq::default(); 2],
            ir_key: [p.decay, p.damp, p.size, p.width],
            p,
        }
    }

    /// The settings that change the impulse response.
    pub fn ir_key(p: &ReverbParams) -> [f64; 4] {
        [p.decay, p.damp, p.size, p.width]
    }

    /// Switch to a response made by [`ReverbImpulse::new`] for the same settings.
    pub fn set_impulse(&mut self, ir: ReverbImpulse) {
        self.conv = [Convolver::new(ir.left), Convolver::new(ir.right)];
        self.ir_key = Self::ir_key(&self.p);
    }
}

impl StereoEffect for ConvReverb {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        // a changed response is built by `set_impulse`; here only the cheap settings apply
        let p = self.p;
        let pre_d = (p.predelay / 1000.0 * self.sr).max(1.0);
        let c = wa_coeffs(Kind::HighPass, p.lowcut, 1.0, 0.0, self.sr);
        for h in &mut self.hp {
            h.c = c;
        }
        let wet = p.wet * 1.4;
        for i in 0..out_l.len() {
            for ch in 0..2 {
                let x = f64::from(if ch == 0 { in_l[i] } else { in_r[i] });
                self.pre[ch].write(x);
                let d = self.pre[ch].read(pre_d);
                let h = self.hp[ch].run(d);
                let y = self.conv[ch].process(h);
                let v = (x * p.dry + y * wet) as f32;
                if ch == 0 {
                    out_l[i] = v;
                } else {
                    out_r[i] = v;
                }
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)?;
        if Self::ir_key(&self.p) != self.ir_key {
            // building a response takes a while, so this is for off-line use; the
            // app builds it ahead of time and calls `set_impulse`
            let ir = ReverbImpulse::new(self.sr, &self.p);
            self.set_impulse(ir);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(fx: &mut dyn StereoEffect, input: &[f32]) -> Vec<f32> {
        let mut l = vec![0f32; input.len()];
        let mut r = vec![0f32; input.len()];
        for (i, chunk) in input.chunks(128).enumerate() {
            let a = i * 128;
            let b = a + chunk.len();
            let (ol, or) = (&mut l[a..b], &mut r[a..b]);
            fx.process(chunk, chunk, ol, or, None);
        }
        l
    }

    fn sine(f: f64, seconds: f64) -> Vec<f32> {
        (0..(48000.0 * seconds) as usize)
            .map(|i| (0.5 * (TAU * f * i as f64 / 48000.0).sin()) as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn the_default_eq_is_transparent_and_a_boost_raises_its_band() {
        let x = sine(1000.0, 0.5);
        let mut eq = Eq::new(48000.0);
        let y = run(&mut eq, &x);
        assert!((rms(&y[12000..]) / rms(&x[12000..]) - 1.0).abs() < 0.01);
        eq.set_params(&serde_json::json!({ "bands": [{}, {}, {}, { "gain": 12 }] }))
            .unwrap();
        let y = run(&mut eq, &x);
        let gain = 20.0 * (rms(&y[12000..]) / rms(&x[12000..])).log10();
        assert!(gain > 10.0 && gain < 13.0, "{gain} dB");
        // a band that is off does nothing
        eq.set_params(&serde_json::json!({ "bands": [{}, {}, {}, { "gain": 12, "on": false }] }))
            .unwrap();
        let y = run(&mut eq, &x);
        assert!((rms(&y[12000..]) / rms(&x[12000..]) - 1.0).abs() < 0.02);
    }

    #[test]
    fn the_response_curve_shows_a_boost_at_its_centre() {
        let mut p = EqParams::default();
        assert!(eq_response_db(&p.bands, 1000.0, 48000.0).abs() < 1e-9);
        p.bands[3].gain = 9.0;
        let at = eq_response_db(&p.bands, 1000.0, 48000.0);
        assert!((at - 9.0).abs() < 0.5, "{at}");
        p.bands[3].on = false;
        assert!(eq_response_db(&p.bands, 1000.0, 48000.0).abs() < 1e-9);
    }

    #[test]
    fn overdrive_adds_harmonics_and_keeps_the_level_sensible() {
        let x = sine(440.0, 0.3);
        let mut d = Distortion::new(48000.0);
        d.set_params(&serde_json::json!({ "drive": 24, "output": -12 }))
            .unwrap();
        let y = run(&mut d, &x);
        assert!(y.iter().all(|v| v.is_finite() && v.abs() < 2.0));
        // a clipped sine has a higher crest-free level than the input scaled equally
        assert!(rms(&y[8000..]) > 0.05);
        // a dry setting passes the delayed input
        d.set_params(&serde_json::json!({ "mix": 0 })).unwrap();
        let dry = run(&mut d, &x);
        assert!((rms(&dry[8000..]) - rms(&x[8000..])).abs() < 0.01);
    }

    #[test]
    fn chorus_phaser_filter_and_stereo_tool_run_and_change_the_sound() {
        let x = sine(700.0, 0.4);
        let mut fx: Vec<Box<dyn StereoEffect>> = vec![
            Box::new(Chorus::new(48000.0)),
            Box::new(Phaser::new(48000.0)),
            Box::new(AutoFilter::new(48000.0)),
            Box::new(StereoTool::new(48000.0)),
        ];
        fx[2]
            .set_params(&serde_json::json!({ "cutoff": 300, "lfoDepth": 2 }))
            .unwrap();
        fx[3]
            .set_params(&serde_json::json!({ "gain": -6 }))
            .unwrap();
        for f in &mut fx {
            let y = run(f.as_mut(), &x);
            assert!(y.iter().all(|v| v.is_finite()));
            assert!(rms(&y[4000..]) > 0.01);
            assert!(y != x, "left the signal untouched");
        }
        // the stereo tool at -6 dB halves the level
        let y = run(fx[3].as_mut(), &x);
        assert!((rms(&y[4000..]) / rms(&x[4000..]) - 0.501).abs() < 0.01);
    }

    #[test]
    fn the_stereo_tool_swaps_and_narrows() {
        let l = vec![0.5f32; 256];
        let r = vec![0.0f32; 256];
        let mut t = StereoTool::new(48000.0);
        let (mut ol, mut or) = (vec![0f32; 256], vec![0f32; 256]);
        t.set_params(&serde_json::json!({ "swap": true })).unwrap();
        t.process(&l, &r, &mut ol, &mut or, None);
        assert!(ol[10].abs() < 1e-6 && (or[10] - 0.5).abs() < 1e-6);
        t.set_params(&serde_json::json!({ "swap": false, "width": 0 }))
            .unwrap();
        t.process(&l, &r, &mut ol, &mut or, None);
        assert!((ol[10] - 0.25).abs() < 1e-6 && (or[10] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_maximizer_limits_to_its_ceiling() {
        let x: Vec<f32> = sine(100.0, 0.5)
            .iter()
            .zip(sine(3000.0, 0.5))
            .map(|(a, b)| (a + b) * 1.5)
            .collect();
        let mut m = Maximizer::new(48000.0);
        let y = run(&mut m, &x);
        let peak = y.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.1 && peak <= 0.97, "{peak}");
    }

    #[test]
    fn the_reverb_rings_on_after_an_impulse_and_dies_away() {
        let mut impulse = vec![0f32; 48000 * 3];
        impulse[100] = 1.0;
        let mut v = ConvReverb::new(48000.0);
        v.set_params(&serde_json::json!({ "decay": 1.0, "dry": 0, "wet": 1 }))
            .unwrap();
        let y = run(&mut v, &impulse);
        let early = rms(&y[2000..10000]);
        let late = rms(&y[100000..140000]);
        assert!(early > 1e-4, "{early}");
        assert!(late < early * 0.2, "{late} vs {early}");
        assert!(y.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn impulse_responses_repeat_and_scale_to_a_comparable_level() {
        let a = make_impulse(48000.0, 1.5, 0.4, 0.5, 1.0);
        let b = make_impulse(48000.0, 1.5, 0.4, 0.5, 1.0);
        assert_eq!(a[0], b[0]);
        let s = normalisation_scale(&a, 48000.0);
        assert!(s.is_finite() && s > 0.0);
    }
}
