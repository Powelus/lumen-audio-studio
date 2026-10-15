// SPDX-License-Identifier: GPL-3.0-or-later
//! Stereo echo: tempo-synced or free time, ping-pong, filtered and optionally
//! saturated feedback, tape-style modulation and input ducking.
//! Ported from `src/audio/dsp/fx/delay2.ts`.

use super::{flush, update_params, Biq, BiqKind, DelayLine, StereoEffect, TAU};
use crate::engine::PatchError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EchoParams {
    /// Synced time in beats.
    pub time: f64,
    /// Use `time` (beats) when true, `ms` when false.
    pub sync: bool,
    pub ms: f64,
    /// Right channel time offset, % of the left time (-50 to 50).
    pub offset: f64,
    pub feedback: f64,
    pub pingpong: bool,
    pub lowcut: f64,
    pub highcut: f64,
    /// Feedback saturation, dB (0 is clean).
    pub drive: f64,
    #[serde(rename = "modRate")]
    pub mod_rate: f64,
    /// Modulation depth, ms.
    #[serde(rename = "modDepth")]
    pub mod_depth: f64,
    /// Duck the echoes while the input plays, 0 to 1.
    pub duck: f64,
    pub dry: f64,
    pub wet: f64,
    /// Tempo, set by the host.
    pub bpm: f64,
}

impl Default for EchoParams {
    fn default() -> Self {
        Self {
            time: 0.75,
            sync: true,
            ms: 375.0,
            offset: 0.0,
            feedback: 0.4,
            pingpong: true,
            lowcut: 200.0,
            highcut: 7000.0,
            drive: 0.0,
            mod_rate: 0.5,
            mod_depth: 0.0,
            duck: 0.0,
            dry: 1.0,
            wet: 0.3,
            bpm: 120.0,
        }
    }
}

pub const ECHO_MAX_SECONDS: f64 = 8.0;
/// Web Audio's default biquad Q (1 dB) as a linear Q.
fn q() -> f64 {
    10f64.powf(1.0 / 20.0)
}

pub struct Echo {
    pub p: EchoParams,
    sr: f64,
    lines: [DelayLine; 2],
    hp: [Biq; 2],
    lp: [Biq; 2],
    fkey: Option<(f64, f64)>,
    d_l: f64,
    d_r: f64,
    fb_l: f64,
    fb_r: f64,
    phase: f64,
    env: f64,
}

impl Echo {
    pub fn new(sr: f64) -> Self {
        let size = (sr * (ECHO_MAX_SECONDS + 0.05)).ceil() as usize + 8;
        Self {
            p: EchoParams::default(),
            sr,
            lines: [DelayLine::new(size), DelayLine::new(size)],
            hp: [Biq::default(), Biq::default()],
            lp: [Biq::default(), Biq::default()],
            fkey: None,
            d_l: -1.0,
            d_r: -1.0,
            fb_l: 0.0,
            fb_r: 0.0,
            phase: 0.0,
            env: 0.0,
        }
    }

    /// Left delay time in seconds at the current settings.
    pub fn seconds(&self) -> f64 {
        let p = &self.p;
        let bpm = if p.bpm == 0.0 { 120.0 } else { p.bpm };
        let s = if p.sync {
            (p.time.max(0.01) * 60.0) / bpm.max(10.0)
        } else {
            p.ms.max(1.0) / 1000.0
        };
        s.min(ECHO_MAX_SECONDS - 0.02)
    }

    fn filters(&mut self) {
        let key = (self.p.lowcut, self.p.highcut);
        if self.fkey == Some(key) {
            return;
        }
        self.fkey = Some(key);
        for c in 0..2 {
            self.hp[c].set(
                BiqKind::HighPass,
                self.p.lowcut.max(10.0),
                q(),
                0.0,
                self.sr,
            );
            self.lp[c].set(
                BiqKind::LowPass,
                self.p.highcut.max(100.0),
                q(),
                0.0,
                self.sr,
            );
        }
    }
}

impl StereoEffect for Echo {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        self.filters();
        let p = self.p;
        let sr = self.sr;
        let n = out_l.len();
        let t_l = self.seconds() * sr;
        let t_r =
            ((ECHO_MAX_SECONDS - 0.02) * sr).min(t_l * (1.0 + (p.offset / 100.0).clamp(-0.5, 0.5)));
        if self.d_l < 0.0 {
            self.d_l = t_l;
            self.d_r = t_r;
        }
        // time changes glide (about 60 ms) like a tape delay instead of clicking
        let glide = 1.0 - (-1.0 / (0.06 * sr)).exp();
        let fb = p.feedback.clamp(0.0, 0.98);
        let pp = p.pingpong;
        let drive = p.drive.max(0.0);
        let dg = 10f64.powf(drive / 20.0);
        let sat = drive > 0.01;
        let depth = (p.mod_depth.clamp(0.0, 20.0) / 1000.0) * sr;
        let d_phase = p.mod_rate.max(0.0) / sr;
        let duck = p.duck.clamp(0.0, 1.0);
        let env_a = (-1.0 / (0.005 * sr)).exp();
        let env_r = (-1.0 / (0.25 * sr)).exp();
        let (dry, wet) = (p.dry, p.wet);
        for i in 0..n {
            let xl = f64::from(in_l[i]);
            let xr = f64::from(in_r[i]);
            self.d_l += (t_l - self.d_l) * glide;
            self.d_r += (t_r - self.d_r) * glide;
            let (mut m_l, mut m_r) = (0.0, 0.0);
            if depth > 0.0 {
                self.phase += d_phase;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }
                let s = (TAU * self.phase).sin();
                m_l = (s + 1.0) * 0.5 * depth;
                m_r = ((TAU * self.phase).cos() + 1.0) * 0.5 * depth;
            }
            // line outputs pass the loop filters; that filtered signal is heard and fed back
            let mut yl = self.lp[0].run(self.hp[0].run(self.lines[0].read(self.d_l + m_l)));
            let mut yr = self.lp[1].run(self.hp[1].run(self.lines[1].read(self.d_r + m_r)));
            if sat {
                yl = (yl * dg).tanh() / dg;
                yr = (yr * dg).tanh() / dg;
            }
            self.fb_l = yl;
            self.fb_r = yr;
            if pp {
                self.lines[0].write(flush((xl + xr) * 0.5 + self.fb_r * fb));
                self.lines[1].write(flush(self.fb_l * fb));
            } else {
                self.lines[0].write(flush(xl + self.fb_l * fb));
                self.lines[1].write(flush(xr + self.fb_r * fb));
            }
            let mut g = wet;
            if duck > 0.0 {
                let lvl = xl.abs().max(xr.abs());
                self.env = flush(if lvl > self.env {
                    lvl + (self.env - lvl) * env_a
                } else {
                    lvl + (self.env - lvl) * env_r
                });
                // full duck at about -12 dBFS input
                g *= 1.0 - duck * (self.env * 4.0).min(1.0);
            }
            out_l[i] = (xl * dry + yl * g) as f32;
            out_r[i] = (xr * dry + yr * g) as f32;
        }
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}
