// SPDX-License-Identifier: GPL-3.0-or-later
//! Step FX (a 16-step sequencer driving a filter, the volume and the pan) and
//! the tempo-synced volume shaper. Ported from `src/audio/dsp/fx/stepfx.ts` and
//! the shaper in `src/audio/worklets/fx.worklet.ts`.
//!
//! Both follow the song position, so the host sets the beat of the first sample
//! of each block and the tempo in `position`; the effect advances it.

use super::{update_params, StereoEffect};
use crate::engine::{js_round, PatchError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::f64::consts::{PI, SQRT_2};

fn flush(x: f64) -> f64 {
    if x.abs() < 1e-20 {
        0.0
    } else {
        x
    }
}

/// Where the song is, for the effects that follow it.
#[derive(Clone, Copy, Debug)]
pub struct Position {
    /// Beat of the first sample of the next block.
    pub beat: f64,
    pub bpm: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StepFxParams {
    /// Step length in beats (0.25 is sixteenths).
    pub rate: f64,
    /// Lane values per step: cutoff 0 to 1 (20 Hz to 20 kHz, log), level 0 to 1, pan -1 to 1.
    pub cutoff: Vec<f64>,
    pub level: Vec<f64>,
    pub pan: Vec<f64>,
    #[serde(rename = "cutoffOn")]
    pub cutoff_on: bool,
    #[serde(rename = "levelOn")]
    pub level_on: bool,
    #[serde(rename = "panOn")]
    pub pan_on: bool,
    /// Steps in the loop (1 to 16).
    pub length: f64,
    pub filter: String,
    pub res: f64,
    /// Smoothing between steps, ms.
    pub glide: f64,
    pub mix: f64,
}

impl Default for StepFxParams {
    fn default() -> Self {
        Self {
            rate: 0.25,
            cutoff: vec![
                0.85, 0.35, 0.6, 0.35, 0.85, 0.35, 0.6, 0.35, 0.85, 0.35, 0.6, 0.35, 0.85, 0.35,
                0.7, 0.45,
            ],
            level: vec![1.0; 16],
            pan: vec![0.0; 16],
            cutoff_on: true,
            level_on: false,
            pan_on: false,
            length: 16.0,
            filter: "lp".into(),
            res: 0.4,
            glide: 15.0,
            mix: 1.0,
        }
    }
}

/// Cutoff lane value (0 to 1) to Hz, logarithmic from 20 Hz to 20 kHz.
pub fn lane_to_hz(v: f64) -> f64 {
    20.0 * 1000f64.powf(v.clamp(0.0, 1.0))
}

pub struct StepFx {
    pub p: StepFxParams,
    sr: f64,
    ic1: [f64; 2],
    ic2: [f64; 2],
    log_f: f64,
    lvl: f64,
    pn: f64,
    started: bool,
    pub position: Position,
}

impl StepFx {
    pub fn new(sr: f64) -> Self {
        Self {
            p: StepFxParams::default(),
            sr,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
            log_f: 1000f64.ln(),
            lvl: 1.0,
            pn: 0.0,
            started: false,
            position: Position {
                beat: 0.0,
                bpm: 120.0,
            },
        }
    }

    /// The current step at a beat position.
    pub fn step_at(&self, beat: f64) -> usize {
        let len = js_round(self.p.length).clamp(1.0, 16.0) as i64;
        let s = (beat / self.p.rate.max(1.0 / 64.0)).floor() as i64;
        (((s % len) + len) % len) as usize
    }

    fn svf(&mut self, c: usize, x: f64, a1: f64, a2: f64, a3: f64, k: f64) -> f64 {
        let v3 = x - self.ic2[c];
        let v1 = a1 * self.ic1[c] + a2 * v3;
        let v2 = self.ic2[c] + a2 * self.ic1[c] + a3 * v3;
        self.ic1[c] = flush(2.0 * v1 - self.ic1[c]);
        self.ic2[c] = flush(2.0 * v2 - self.ic2[c]);
        match self.p.filter.as_str() {
            "lp" => v2,
            "bp" => v1,
            _ => x - k * v1 - v2,
        }
    }
}

fn lane(v: &[f64], s: usize, d: f64) -> f64 {
    v.get(s).copied().unwrap_or(d)
}

impl StereoEffect for StepFx {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let sr = self.sr;
        let (mut beat, bps) = (self.position.beat, self.position.bpm / 60.0 / sr);
        let sm = 1.0 - (-1.0 / (self.p.glide.max(0.1) * 0.001 * sr)).exp();
        let k = 2.0 - 1.95 * self.p.res.clamp(0.0, 1.0);
        let mix = self.p.mix.clamp(0.0, 1.0);
        for i in 0..out_l.len() {
            let s = self.step_at(beat);
            let tf = if self.p.cutoff_on {
                lane_to_hz(lane(&self.p.cutoff, s, 1.0))
            } else {
                20000.0
            }
            .ln();
            let tl = if self.p.level_on {
                lane(&self.p.level, s, 1.0).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let tp = if self.p.pan_on {
                lane(&self.p.pan, s, 0.0).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            if !self.started {
                self.log_f = tf;
                self.lvl = tl;
                self.pn = tp;
                self.started = true;
            }
            self.log_f += (tf - self.log_f) * sm;
            self.lvl += (tl - self.lvl) * sm;
            self.pn += (tp - self.pn) * sm;
            // zero-delay-feedback SVF, coefficients per sample (the cutoff moves smoothly)
            let g = (PI * (sr * 0.45).min(self.log_f.exp()) / sr).tan();
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            let a = (self.pn + 1.0) * 0.25 * PI;
            let gl = a.cos() * SQRT_2 * self.lvl;
            let gr = a.sin() * SQRT_2 * self.lvl;
            let xl = f64::from(in_l[i]);
            let xr = f64::from(in_r[i]);
            let yl = if self.p.cutoff_on {
                self.svf(0, xl, a1, a2, a3, k)
            } else {
                xl
            };
            let yr = if self.p.cutoff_on {
                self.svf(1, xr, a1, a2, a3, k)
            } else {
                xr
            };
            out_l[i] = (xl * (1.0 - mix) + yl * gl * mix) as f32;
            out_r[i] = (xr * (1.0 - mix) + yr * gr * mix) as f32;
            beat += bps;
        }
        self.position.beat = beat;
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}

// ---------------------------------------------------------------------------
// Volume shaper
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShaperPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaperParams {
    pub points: Vec<ShaperPoint>,
    /// Cycle length in beats.
    pub div: f64,
    pub depth: f64,
    pub smooth: f64,
    pub offset: f64,
}

impl Default for ShaperParams {
    fn default() -> Self {
        Self {
            points: vec![
                ShaperPoint { x: 0.0, y: 0.0 },
                ShaperPoint { x: 0.08, y: 1.0 },
                ShaperPoint { x: 1.0, y: 1.0 },
            ],
            div: 1.0,
            depth: 1.0,
            smooth: 2.0,
            offset: 0.0,
        }
    }
}

/// Tempo-synced volume shaper (LFO-style sidechain pumping).
pub struct VolumeShaper {
    pub p: ShaperParams,
    sr: f64,
    g: f64,
    pub position: Position,
}

impl VolumeShaper {
    pub fn new(sr: f64) -> Self {
        Self {
            p: ShaperParams::default(),
            sr,
            g: 1.0,
            position: Position {
                beat: 0.0,
                bpm: 120.0,
            },
        }
    }

    fn curve(&self, x: f64) -> f64 {
        let pts = &self.p.points;
        if pts.is_empty() {
            return 1.0;
        }
        if x <= pts[0].x {
            return pts[0].y;
        }
        for i in 1..pts.len() {
            if x <= pts[i].x {
                let (a, b) = (pts[i - 1], pts[i]);
                let t = (x - a.x) / (b.x - a.x).max(1e-6);
                return a.y + (b.y - a.y) * t;
            }
        }
        pts[pts.len() - 1].y
    }
}

impl StereoEffect for VolumeShaper {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        let sr = self.sr;
        let div = self.p.div.max(1.0 / 64.0);
        let beats_per_sample = self.position.bpm / 60.0 / sr;
        let mut beat = self.position.beat;
        let sm = 1.0 - (-1.0 / (self.p.smooth.max(0.05) * 0.001 * sr)).exp();
        for i in 0..out_l.len() {
            let mut ph = (beat / div + self.p.offset) % 1.0;
            if ph < 0.0 {
                ph += 1.0;
            }
            let target = 1.0 - self.p.depth * (1.0 - self.curve(ph));
            self.g += (target - self.g) * sm;
            out_l[i] = (f64::from(in_l[i]) * self.g) as f32;
            out_r[i] = (f64::from(in_r[i]) * self.g) as f32;
            beat += beats_per_sample;
        }
        self.position.beat = beat;
    }

    fn set_params(&mut self, over: &Value) -> Result<(), PatchError> {
        update_params(&mut self.p, over)
    }
}
