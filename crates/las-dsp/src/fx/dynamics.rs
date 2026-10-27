// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! Compressor, limiter, noise gate, transient shaper and bit crusher.
//! Ported from `src/audio/worklets/fx.worklet.ts` and
//! `src/audio/dsp/fx/dynamics2.ts`.

use super::{db_to_lin, flush, lin_to_db, ms_coef, update_params, StereoEffect};
use crate::engine::{js_round, PatchError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Compressor with an optional external sidechain
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorParams {
    pub threshold: f64,
    pub ratio: f64,
    /// ms
    pub attack: f64,
    /// ms
    pub release: f64,
    pub knee: f64,
    pub makeup: f64,
    pub mix: f64,
    /// Detect on the external sidechain input.
    pub sidechain: bool,
    /// Detector high pass, Hz (0 is off).
    #[serde(rename = "scHpf")]
    pub sc_hpf: f64,
    /// Hear the detector signal.
    pub listen: bool,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold: -18.0,
            ratio: 4.0,
            attack: 5.0,
            release: 120.0,
            knee: 6.0,
            makeup: 0.0,
            mix: 1.0,
            sidechain: false,
            sc_hpf: 0.0,
            listen: false,
        }
    }
}

pub struct Compressor {
    pub p: CompressorParams,
    sr: f64,
    env_db: f64,
    hp_state: [f64; 2],
    hp_prev: [f64; 2],
    gr_max: f64,
}

impl Compressor {
    pub fn new(sr: f64) -> Self {
        Self {
            p: CompressorParams::default(),
            sr,
            env_db: -120.0,
            hp_state: [0.0; 2],
            hp_prev: [0.0; 2],
            gr_max: 0.0,
        }
    }

    /// The largest gain reduction (dB) since the last call, for a meter.
    pub fn take_gain_reduction(&mut self) -> f64 {
        std::mem::take(&mut self.gr_max)
    }
}

impl StereoEffect for Compressor {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
    ) {
        let p = self.p;
        let sr = self.sr;
        let (sc_l, sc_r) = match (p.sidechain, sidechain) {
            (true, Some((a, b))) => (a, b),
            _ => (in_l, in_r),
        };
        let atk = (-1.0 / (p.attack.max(0.05) * 0.001 * sr)).exp();
        let rel = (-1.0 / (p.release.max(1.0) * 0.001 * sr)).exp();
        let makeup = db_to_lin(p.makeup);
        let hp_a = if p.sc_hpf > 0.0 {
            (-2.0 * std::f64::consts::PI * p.sc_hpf / sr).exp()
        } else {
            0.0
        };
        let half_knee = p.knee / 2.0;
        let slope = 1.0 - 1.0 / p.ratio.max(1.0);
        for i in 0..out_l.len() {
            let mut dl = f64::from(sc_l[i]);
            let mut dr = f64::from(sc_r[i]);
            if hp_a > 0.0 {
                // one-pole high pass on the detector
                let yl = hp_a * (self.hp_state[0] + dl - self.hp_prev[0]);
                self.hp_prev[0] = dl;
                self.hp_state[0] = yl;
                let yr = hp_a * (self.hp_state[1] + dr - self.hp_prev[1]);
                self.hp_prev[1] = dr;
                self.hp_state[1] = yr;
                dl = yl;
                dr = yr;
            }
            let lvl = lin_to_db(dl.abs().max(dr.abs()));
            let coef = if lvl > self.env_db { atk } else { rel };
            self.env_db = lvl + (self.env_db - lvl) * coef;
            let over = self.env_db - p.threshold;
            let gr = if over <= -half_knee {
                0.0
            } else if over >= half_knee || half_knee == 0.0 {
                over * slope
            } else {
                (slope * (over + half_knee) * (over + half_knee)) / (2.0 * p.knee)
            };
            if gr > self.gr_max {
                self.gr_max = gr;
            }
            let g = db_to_lin(-gr) * makeup;
            let l = f64::from(in_l[i]);
            let r = f64::from(in_r[i]);
            if p.listen {
                out_l[i] = dl as f32;
                out_r[i] = dr as f32;
            } else {
                out_l[i] = (l * (1.0 - p.mix) + l * g * p.mix) as f32;
                out_r[i] = (r * (1.0 - p.mix) + r * g * p.mix) as f32;
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Look-ahead brickwall limiter
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LimiterParams {
    /// Input gain, dB.
    pub gain: f64,
    /// Output ceiling, dB.
    pub ceiling: f64,
    /// ms
    pub release: f64,
    /// ms
    pub lookahead: f64,
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self {
            gain: 0.0,
            ceiling: -0.3,
            release: 80.0,
            lookahead: 3.0,
        }
    }
}

pub struct Limiter {
    pub p: LimiterParams,
    sr: f64,
    buf: [Vec<f32>; 2],
    pos: usize,
    g: f64,
    // sliding window minimum (monotonic deque over needed gains)
    dq_val: Vec<f64>,
    dq_idx: Vec<f64>,
    dq_head: usize,
    dq_tail: usize,
    sample_index: f64,
    gr_max: f64,
}

impl Limiter {
    pub fn new(sr: f64) -> Self {
        let max = (sr * 0.02).ceil() as usize + 4;
        Self {
            p: LimiterParams::default(),
            sr,
            buf: [vec![0.0; max], vec![0.0; max]],
            pos: 0,
            g: 1.0,
            dq_val: vec![0.0; max * 2],
            dq_idx: vec![0.0; max * 2],
            dq_head: 0,
            dq_tail: 0,
            sample_index: 0.0,
            gr_max: 0.0,
        }
    }

    pub fn take_gain_reduction(&mut self) -> f64 {
        std::mem::take(&mut self.gr_max)
    }

    /// Look-ahead in samples at the current settings.
    pub fn lookahead_samples(&self) -> usize {
        let size = self.buf[0].len() as f64;
        js_round((self.p.lookahead / 1000.0) * self.sr).clamp(1.0, size - 2.0) as usize
    }
}

impl StereoEffect for Limiter {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let size = self.buf[0].len();
        let look = self.lookahead_samples();
        let gain = db_to_lin(self.p.gain);
        let ceil = db_to_lin(self.p.ceiling);
        let rel = (-1.0 / (self.p.release.max(1.0) * 0.001 * self.sr)).exp();
        let cap = self.dq_val.len();
        for i in 0..out_l.len() {
            let l = f64::from(in_l[i]) * gain;
            let r = f64::from(in_r[i]) * gain;
            self.buf[0][self.pos] = l as f32;
            self.buf[1][self.pos] = r as f32;
            let peak = l.abs().max(r.abs());
            let need = if peak > ceil { ceil / peak } else { 1.0 };
            let idx = self.sample_index;
            self.sample_index += 1.0;
            // push into the deque
            while self.dq_tail > self.dq_head && self.dq_val[(self.dq_tail - 1) % cap] >= need {
                self.dq_tail -= 1;
            }
            self.dq_val[self.dq_tail % cap] = need;
            self.dq_idx[self.dq_tail % cap] = idx;
            self.dq_tail += 1;
            while self.dq_idx[self.dq_head % cap] <= idx - look as f64 {
                self.dq_head += 1;
            }
            let target = self.dq_val[self.dq_head % cap];
            if target < self.g {
                self.g += (target - self.g) * 0.5;
            } else {
                self.g = target + (self.g - target) * rel;
            }
            let rp = if self.pos < look {
                self.pos + size - look
            } else {
                self.pos - look
            };
            let mut ol = f64::from(self.buf[0][rp]) * self.g;
            let mut or = f64::from(self.buf[1][rp]) * self.g;
            ol = ol.clamp(-ceil, ceil);
            or = or.clamp(-ceil, ceil);
            out_l[i] = ol as f32;
            out_r[i] = or as f32;
            self.pos = (self.pos + 1) % size;
            let gr = -lin_to_db(self.g);
            if gr > self.gr_max {
                self.gr_max = gr;
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }

    fn latency(&self) -> f64 {
        self.lookahead_samples() as f64 / self.sr
    }
}

// ---------------------------------------------------------------------------
// Bit and sample-rate reducer
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CrusherParams {
    pub bits: f64,
    pub downsample: f64,
    pub mix: f64,
}

impl Default for CrusherParams {
    fn default() -> Self {
        Self {
            bits: 8.0,
            downsample: 4.0,
            mix: 1.0,
        }
    }
}

pub struct Crusher {
    pub p: CrusherParams,
    hold: [f64; 2],
    count: u64,
}

impl Crusher {
    pub fn new(_sr: f64) -> Self {
        Self {
            p: CrusherParams::default(),
            hold: [0.0; 2],
            count: 0,
        }
    }
}

impl StereoEffect for Crusher {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let levels = 2f64.powf(self.p.bits.max(1.0)) / 2.0;
        let ds = js_round(self.p.downsample).max(1.0) as u64;
        for i in 0..out_l.len() {
            let c = self.count;
            self.count += 1;
            if c % ds == 0 {
                self.hold[0] = js_round(f64::from(in_l[i]) * levels) / levels;
                self.hold[1] = js_round(f64::from(in_r[i]) * levels) / levels;
            }
            let (dl, dr) = (f64::from(in_l[i]), f64::from(in_r[i]));
            out_l[i] = (dl * (1.0 - self.p.mix) + self.hold[0] * self.p.mix) as f32;
            out_r[i] = (dr * (1.0 - self.p.mix) + self.hold[1] * self.p.mix) as f32;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Noise gate
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GateParams {
    /// Open threshold, dBFS.
    pub threshold: f64,
    /// Close this many dB below the open threshold.
    pub hysteresis: f64,
    /// Attenuation when closed, dB.
    pub range: f64,
    pub attack: f64,
    pub hold: f64,
    pub release: f64,
    /// Detector high pass, Hz (0 is off).
    #[serde(rename = "scHpf")]
    pub sc_hpf: f64,
    pub sidechain: bool,
    pub listen: bool,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold: -40.0,
            hysteresis: 4.0,
            range: -60.0,
            attack: 0.5,
            hold: 30.0,
            release: 120.0,
            sc_hpf: 0.0,
            sidechain: false,
            listen: false,
        }
    }
}

/// Noise gate with hysteresis, hold and smooth attack and release.
pub struct NoiseGate {
    pub p: GateParams,
    sr: f64,
    env: f64,
    open: bool,
    hold_left: i64,
    gain: f64,
    hp: [f64; 2],
    hp_prev: [f64; 2],
    /// Lowest gain reached since the last read (for metering), dB.
    pub min_gain_db: f64,
}

impl NoiseGate {
    pub fn new(sr: f64) -> Self {
        let p = GateParams::default();
        Self {
            gain: db_to_lin(p.range),
            p,
            sr,
            env: 0.0,
            open: false,
            hold_left: 0,
            hp: [0.0; 2],
            hp_prev: [0.0; 2],
            min_gain_db: 0.0,
        }
    }
}

impl StereoEffect for NoiseGate {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
    ) {
        let p = self.p;
        let sr = self.sr;
        let open_lin = db_to_lin(p.threshold);
        let close_lin = db_to_lin(p.threshold - p.hysteresis.max(0.0));
        let floor = db_to_lin(p.range.min(0.0));
        let atk = ms_coef(p.attack, sr);
        let rel = ms_coef(p.release, sr);
        let env_rel = ms_coef(10.0, sr);
        let hold_samples = js_round((p.hold / 1000.0) * sr) as i64;
        let hp_a = if p.sc_hpf > 0.0 {
            (-2.0 * std::f64::consts::PI * p.sc_hpf / sr).exp()
        } else {
            0.0
        };
        let (d_l, d_r) = match (p.sidechain, sidechain) {
            (true, Some((a, b))) => (a, b),
            _ => (in_l, in_r),
        };
        for i in 0..out_l.len() {
            let mut a = f64::from(d_l[i]);
            let mut b = f64::from(d_r[i]);
            if hp_a > 0.0 {
                let ya = hp_a * (self.hp[0] + a - self.hp_prev[0]);
                self.hp_prev[0] = a;
                self.hp[0] = flush(ya);
                let yb = hp_a * (self.hp[1] + b - self.hp_prev[1]);
                self.hp_prev[1] = b;
                self.hp[1] = flush(yb);
                a = ya;
                b = yb;
            }
            // peak detector: instant attack, 10 ms release
            let lvl = a.abs().max(b.abs());
            self.env = if lvl > self.env {
                lvl
            } else {
                flush(lvl + (self.env - lvl) * env_rel)
            };
            if self.env >= open_lin {
                self.open = true;
                self.hold_left = hold_samples;
            } else if self.open && self.env < close_lin {
                if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.open = false;
                }
            }
            let target = if self.open { 1.0 } else { floor };
            self.gain = target + (self.gain - target) * if target > self.gain { atk } else { rel };
            let g = self.gain;
            if p.listen {
                out_l[i] = a as f32;
                out_r[i] = b as f32;
            } else {
                out_l[i] = (f64::from(in_l[i]) * g) as f32;
                out_r[i] = (f64::from(in_r[i]) * g) as f32;
            }
            if g < 1.0 {
                let db = 20.0 * g.max(1e-6).log10();
                if db < self.min_gain_db {
                    self.min_gain_db = db;
                }
            }
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Transient shaper
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TransientParams {
    /// -100 to 100 %: boost or soften attacks.
    pub attack: f64,
    /// -100 to 100 %: boost or shorten tails.
    pub sustain: f64,
    /// Detector speed, ms.
    pub speed: f64,
    pub output: f64,
    pub mix: f64,
}

impl Default for TransientParams {
    fn default() -> Self {
        Self {
            attack: 0.0,
            sustain: 0.0,
            speed: 20.0,
            output: 0.0,
            mix: 1.0,
        }
    }
}

/// Level-independent transient shaper: compares a fast and a slow envelope.
pub struct TransientShaper {
    pub p: TransientParams,
    sr: f64,
    fast: f64,
    slow: f64,
    sus: f64,
    pow: f64,
    gain_db: f64,
}

impl TransientShaper {
    pub fn new(sr: f64) -> Self {
        Self {
            p: TransientParams::default(),
            sr,
            fast: 0.0,
            slow: 0.0,
            sus: 0.0,
            pow: 0.0,
            gain_db: 0.0,
        }
    }
}

impl StereoEffect for TransientShaper {
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
        let fast_a = ms_coef(0.3, sr);
        let fast_r = ms_coef((p.speed * 0.5).max(2.0), sr);
        let slow_a = ms_coef(p.speed.max(2.0), sr);
        let slow_r = ms_coef((p.speed * 0.5).max(2.0), sr);
        let sus_a = ms_coef(1.0, sr);
        let sus_r = ms_coef((p.speed * 10.0).max(20.0), sr);
        let smooth = ms_coef(0.5, sr);
        // 2 ms power smoothing removes the per-cycle ripple of a rectified waveform
        let pow_c = ms_coef(2.0, sr);
        let atk_amt = (p.attack / 100.0).clamp(-1.0, 1.0);
        let sus_amt = (p.sustain / 100.0).clamp(-1.0, 1.0);
        let out = db_to_lin(p.output);
        let mix = p.mix.clamp(0.0, 1.0);
        for i in 0..out_l.len() {
            let (xl, xr) = (f64::from(in_l[i]), f64::from(in_r[i]));
            let x2 = (xl * xl).max(xr * xr);
            self.pow = flush(x2 + (self.pow - x2) * pow_c);
            let lvl = self.pow.sqrt() + 1e-9;
            self.fast =
                flush(lvl + (self.fast - lvl) * if lvl > self.fast { fast_a } else { fast_r });
            self.slow =
                flush(lvl + (self.slow - lvl) * if lvl > self.slow { slow_a } else { slow_r });
            self.sus = flush(lvl + (self.sus - lvl) * if lvl > self.sus { sus_a } else { sus_r });
            // positive while an attack is rising faster than the slow follower
            let attack_db = (20.0 * ((self.fast + 1e-9) / (self.slow + 1e-9)).log10()).max(0.0);
            // positive while the long tail follower sits above the current level (decay)
            let tail_db = (20.0 * ((self.sus + 1e-9) / (self.fast + 1e-9)).log10()).max(0.0);
            let target =
                (atk_amt * attack_db * 2.0 + sus_amt * tail_db.min(24.0) * 0.75).clamp(-24.0, 24.0);
            self.gain_db = target + (self.gain_db - target) * smooth;
            let g = 10f64.powf(self.gain_db / 20.0) * out;
            out_l[i] = (xl * (1.0 - mix) + xl * g * mix) as f32;
            out_r[i] = (xr * (1.0 - mix) + xr * g * mix) as f32;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}
