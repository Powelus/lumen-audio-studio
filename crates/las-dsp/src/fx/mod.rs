// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio effects, ported from `src/audio/dsp/fx/*.ts` and
//! `src/audio/worklets/fx.worklet.ts` of the web version.
//!
//! Every effect is a stereo processor with a typed parameter set. Parameters
//! come from JSON (only the keys present change), so an effect state saved by
//! the web version loads as it is. As in the original, delay lines store `f32`
//! samples and the rest of the arithmetic is `f64`.

// The index loops follow the original line by line, which keeps the port easy to check.
#![allow(clippy::needless_range_loop)]

use crate::engine::PatchError;
use crate::patch::merge_patch;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::f64::consts::PI;

pub mod delay;
pub mod dynamics;
pub mod modulation;
pub mod reverb;
pub mod stepfx;

pub(crate) const TAU: f64 = PI * 2.0;

pub(crate) fn flush(x: f64) -> f64 {
    if x.abs() < 1e-25 {
        0.0
    } else {
        x
    }
}

pub(crate) fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

pub(crate) fn lin_to_db(g: f64) -> f64 {
    if g <= 1e-9 {
        -180.0
    } else {
        20.0 * g.log10()
    }
}

/// Smoothing coefficient for a time constant in milliseconds.
pub(crate) fn ms_coef(ms: f64, sr: f64) -> f64 {
    (-1.0 / (ms.max(0.01) * 0.001 * sr)).exp()
}

/// Merge the keys of `over` into the typed parameters `p`.
pub fn update_params<P: Serialize + DeserializeOwned>(
    p: &mut P,
    over: &Value,
) -> Result<(), PatchError> {
    let cur = serde_json::to_value(&*p).map_err(|e| PatchError(e.to_string()))?;
    let merged = merge_patch(&cur, over);
    *p = serde_json::from_value(merged).map_err(|e| PatchError(e.to_string()))?;
    Ok(())
}

/// A stereo effect. `sidechain` is the external detector input, for the effects
/// that have one.
pub trait StereoEffect {
    /// Process `out_l.len()` samples. All slices have the same length.
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
    );

    /// Change parameters from JSON; only the keys present change.
    fn set_params(&mut self, over: &Value) -> Result<(), PatchError>;

    /// Delay the effect adds to its output at the current settings, seconds.
    fn latency(&self) -> f64 {
        0.0
    }
}

/// Circular delay line with 4-point Hermite interpolation. Stores `f32`.
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
    size: usize,
}

impl DelayLine {
    pub fn new(size: usize) -> Self {
        Self {
            buf: vec![0.0; size],
            pos: 0,
            size,
        }
    }

    pub fn write(&mut self, x: f64) {
        self.buf[self.pos] = x as f32;
        self.pos = (self.pos + 1) % self.size;
    }

    /// Read `delay` samples behind the last written sample (fractional).
    pub fn read(&self, delay: f64) -> f64 {
        let d = (self.size as f64 - 3.0).min(delay.max(1.0));
        let p = self.pos as f64 - 1.0 - d;
        let i = p.floor();
        let f = p - i;
        let i = i as i64;
        let s = self.size as i64;
        let at = |k: i64| f64::from(self.buf[(((i + k) % s) + s) as usize % self.size]);
        let xm1 = at(-1);
        let x0 = at(0);
        let x1 = at(1);
        let x2 = at(2);
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * f + c2) * f + c1) * f + x0
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiqKind {
    Peak,
    LowPass,
    HighPass,
    HighShelf,
    LowShelf,
}

/// RBJ peaking, shelving and pass biquad (transposed direct form II).
#[derive(Clone, Debug)]
pub struct Biq {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Default for Biq {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biq {
    pub fn set(&mut self, kind: BiqKind, f: f64, q: f64, gain_db: f64, sr: f64) {
        let w = (TAU * f.min(sr * 0.49)) / sr;
        let cs = w.cos();
        let sn = w.sin();
        let a = 10f64.powf(gain_db / 40.0);
        let al = sn / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2);
        match kind {
            BiqKind::LowPass => {
                b0 = (1.0 - cs) / 2.0;
                b1 = 1.0 - cs;
                b2 = (1.0 - cs) / 2.0;
                a0 = 1.0 + al;
                a1 = -2.0 * cs;
                a2 = 1.0 - al;
            }
            BiqKind::HighPass => {
                b0 = (1.0 + cs) / 2.0;
                b1 = -(1.0 + cs);
                b2 = (1.0 + cs) / 2.0;
                a0 = 1.0 + al;
                a1 = -2.0 * cs;
                a2 = 1.0 - al;
            }
            BiqKind::HighShelf => {
                let s2 = 2.0 * a.sqrt() * al;
                b0 = a * (a + 1.0 + (a - 1.0) * cs + s2);
                b1 = -2.0 * a * (a - 1.0 + (a + 1.0) * cs);
                b2 = a * (a + 1.0 + (a - 1.0) * cs - s2);
                a0 = a + 1.0 - (a - 1.0) * cs + s2;
                a1 = 2.0 * (a - 1.0 - (a + 1.0) * cs);
                a2 = a + 1.0 - (a - 1.0) * cs - s2;
            }
            BiqKind::LowShelf => {
                let s2 = 2.0 * a.sqrt() * al;
                b0 = a * (a + 1.0 - (a - 1.0) * cs + s2);
                b1 = 2.0 * a * (a - 1.0 - (a + 1.0) * cs);
                b2 = a * (a + 1.0 - (a - 1.0) * cs - s2);
                a0 = a + 1.0 + (a - 1.0) * cs + s2;
                a1 = -2.0 * (a - 1.0 + (a + 1.0) * cs);
                a2 = a + 1.0 + (a - 1.0) * cs - s2;
            }
            BiqKind::Peak => {
                b0 = 1.0 + al * a;
                b1 = -2.0 * cs;
                b2 = 1.0 - al * a;
                a0 = 1.0 + al / a;
                a1 = -2.0 * cs;
                a2 = 1.0 - al / a;
            }
        }
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = flush(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = flush(self.b2 * x - self.a2 * y);
        y
    }
}
