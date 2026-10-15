// SPDX-License-Identifier: GPL-3.0-or-later
//! Algorithmic reverb: an 8-line feedback delay network with a Hadamard mixing
//! matrix, input diffusion, frequency-dependent decay, gentle line modulation,
//! pre-delay and freeze. Ported from `src/audio/dsp/fx/reverb2.ts`.

use super::{flush, update_params, DelayLine, StereoEffect};
use crate::engine::PatchError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HallParams {
    /// RT60 at mid frequencies, seconds.
    pub decay: f64,
    /// Room size 0 to 1 (scales the delay lengths).
    pub size: f64,
    /// Pre-delay, ms.
    pub predelay: f64,
    /// High-frequency damping 0 to 1.
    pub damp: f64,
    /// Input diffusion 0 to 1.
    pub diffusion: f64,
    /// Modulation depth 0 to 1.
    #[serde(rename = "mod")]
    pub modulation: f64,
    /// Wet low cut and high cut, Hz.
    pub lowcut: f64,
    pub highcut: f64,
    /// Stereo width of the tail 0 to 1.
    pub width: f64,
    /// Hold the current tail indefinitely.
    pub freeze: bool,
    pub dry: f64,
    pub wet: f64,
}

impl Default for HallParams {
    fn default() -> Self {
        Self {
            decay: 2.4,
            size: 0.6,
            predelay: 15.0,
            damp: 0.4,
            diffusion: 0.7,
            modulation: 0.3,
            lowcut: 120.0,
            highcut: 11000.0,
            width: 1.0,
            freeze: false,
            dry: 1.0,
            wet: 0.25,
        }
    }
}

/// Mutually prime-ish base delays (ms) for the 8 lines.
const BASE_MS: [f64; 8] = [29.7, 37.1, 41.1, 43.7, 53.9, 61.3, 67.1, 73.9];
const DIFF_MS: [f64; 4] = [4.77, 3.59, 12.73, 9.31];

#[derive(Default)]
struct OnePole {
    z: f64,
    a: f64,
}

impl OnePole {
    fn set_lowpass(&mut self, f: f64, sr: f64) {
        self.a = (-2.0 * PI * f.min(sr * 0.45) / sr).exp();
    }

    fn lp(&mut self, x: f64) -> f64 {
        self.z = flush(x * (1.0 - self.a) + self.z * self.a);
        self.z
    }
}

/// Schroeder allpass used for diffusion.
struct Allpass {
    dl: DelayLine,
    delay: f64,
}

impl Allpass {
    fn new(delay: f64) -> Self {
        Self {
            dl: DelayLine::new(delay.ceil() as usize + 8),
            delay,
        }
    }

    fn run(&mut self, x: f64, g: f64) -> f64 {
        let d = self.dl.read(self.delay);
        let v = x + g * d;
        self.dl.write(flush(v));
        d - g * v
    }
}

pub struct Hall {
    pub p: HallParams,
    sr: f64,
    lines: Vec<DelayLine>,
    lens: [f64; 8],
    gains: [f64; 8],
    damps: Vec<OnePole>,
    pre: [DelayLine; 2],
    diff: [Vec<Allpass>; 2],
    lc: [OnePole; 2],
    hc: [OnePole; 2],
    lfo: f64,
    key: Option<[f64; 5]>,
    state: [f64; 8],
    fb: [f64; 8],
}

impl Hall {
    pub fn new(sr: f64) -> Self {
        let max_len = ((sr * 74.0 * 2.2) / 1000.0).ceil() as usize + 64;
        let pre_len = (sr * 0.52).ceil() as usize;
        let diff = |c: f64| -> Vec<Allpass> {
            DIFF_MS
                .iter()
                .map(|ms| Allpass::new(((ms * (1.0 + c * 0.13)) / 1000.0) * sr))
                .collect()
        };
        Self {
            p: HallParams::default(),
            sr,
            lines: BASE_MS.iter().map(|_| DelayLine::new(max_len)).collect(),
            lens: [0.0; 8],
            gains: [0.0; 8],
            damps: BASE_MS.iter().map(|_| OnePole::default()).collect(),
            pre: [DelayLine::new(pre_len), DelayLine::new(pre_len)],
            diff: [diff(0.0), diff(1.0)],
            lc: [OnePole::default(), OnePole::default()],
            hc: [OnePole::default(), OnePole::default()],
            lfo: 0.0,
            key: None,
            state: [0.0; 8],
            fb: [0.0; 8],
        }
    }

    fn update(&mut self) {
        let p = self.p;
        let key = [p.decay, p.size, p.damp, p.lowcut, p.highcut];
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        let scale = 0.4 + p.size.clamp(0.0, 1.0) * 1.8;
        let rt = p.decay.clamp(0.1, 60.0);
        // high frequencies decay `hf_ratio` times faster
        let hf_ratio = 1.0 + p.damp.clamp(0.0, 1.0) * 6.0;
        for i in 0..8 {
            self.lens[i] = (BASE_MS[i] * scale * self.sr) / 1000.0;
            let sec = self.lens[i] / self.sr;
            // gain per pass so energy falls 60 dB in rt seconds
            self.gains[i] = 10f64.powf((-3.0 * sec) / rt);
            let g_hi = 10f64.powf((-3.0 * sec) / (rt / hf_ratio));
            // one-pole lowpass tuned so its Nyquist gain equals g_hi / g
            let r = (g_hi / self.gains[i]).min(1.0);
            self.damps[i].a = (1.0 - r) / (1.0 + r);
        }
        for c in 0..2 {
            self.lc[c].set_lowpass(p.lowcut.max(20.0), self.sr);
            self.hc[c].set_lowpass(p.highcut.max(200.0), self.sr);
        }
    }
}

impl StereoEffect for Hall {
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
        let pre_d = ((p.predelay.clamp(0.0, 500.0) / 1000.0) * sr).max(1.0);
        let diff_g = (p.diffusion * 0.75).clamp(0.0, 0.75);
        let mod_depth = p.modulation.clamp(0.0, 1.0) * 0.0012 * sr;
        let width = p.width.clamp(0.0, 1.0);
        let freeze = p.freeze;
        let (dry, wet) = (p.dry, p.wet);
        for i in 0..out_l.len() {
            let xl = f64::from(in_l[i]);
            let xr = f64::from(in_r[i]);
            self.pre[0].write(xl);
            self.pre[1].write(xr);
            let mut a = self.pre[0].read(pre_d);
            let mut b = self.pre[1].read(pre_d);
            for ap in &mut self.diff[0] {
                a = ap.run(a, diff_g);
            }
            for ap in &mut self.diff[1] {
                b = ap.run(b, diff_g);
            }
            if freeze {
                a = 0.0;
                b = 0.0;
            }
            self.lfo += 0.37 / sr;
            if self.lfo >= 1.0 {
                self.lfo -= 1.0;
            }
            let m1 = (2.0 * PI * self.lfo).sin() * mod_depth;
            let m2 = (2.0 * PI * self.lfo * 1.31).cos() * mod_depth;
            // read line outputs, apply damping and decay gain
            for k in 0..8 {
                let m = if k == 1 {
                    m1
                } else if k == 6 {
                    m2
                } else {
                    0.0
                };
                let y = self.lines[k].read(self.lens[k] + m);
                // frozen: no damping and no loss, the tail circulates unchanged
                self.state[k] = if freeze {
                    y
                } else {
                    self.damps[k].lp(y) * self.gains[k]
                };
            }
            // 8x8 Hadamard (normalised): mixes energy between all lines losslessly
            let s = self.state;
            let (h0, h1, h2, h3) = (s[0] + s[1], s[0] - s[1], s[2] + s[3], s[2] - s[3]);
            let (h4, h5, h6, h7) = (s[4] + s[5], s[4] - s[5], s[6] + s[7], s[6] - s[7]);
            let (i0, i1, i2, i3) = (h0 + h2, h1 + h3, h0 - h2, h1 - h3);
            let (i4, i5, i6, i7) = (h4 + h6, h5 + h7, h4 - h6, h5 - h7);
            let norm = 0.35355339059327373; // 1/sqrt(8)
            self.fb = [
                i0 + i4,
                i1 + i5,
                i2 + i6,
                i3 + i7,
                i0 - i4,
                i1 - i5,
                i2 - i6,
                i3 - i7,
            ];
            for k in 0..8 {
                let inj = if k % 2 == 0 { a } else { b };
                self.lines[k].write(flush(self.fb[k] * norm + inj * 0.5));
            }
            // stereo taps: even lines left, odd lines right
            let mut wl = (s[0] + s[2] + s[4] + s[6]) * 0.5;
            let mut wr = (s[1] + s[3] + s[5] + s[7]) * 0.5;
            let mid = (wl + wr) * 0.5;
            wl = mid + (wl - mid) * width;
            wr = mid + (wr - mid) * width;
            // wet tone: high cut then low cut (low cut is the signal minus its lowpass)
            wl = self.hc[0].lp(wl);
            wr = self.hc[1].lp(wr);
            wl -= self.lc[0].lp(wl);
            wr -= self.lc[1].lp(wr);
            out_l[i] = (xl * dry + wl * wet) as f32;
            out_r[i] = (xr * dry + wr * wet) as f32;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}
