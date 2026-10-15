// SPDX-License-Identifier: GPL-3.0-or-later
//! Facet: four-operator phase-modulation (FM) synth on the shared voice engine.
//!
//! Each operator is an oscillator with its own envelope. Algorithms decide which
//! operators modulate which; carriers are heard. A modulator with level 1 shifts
//! its target's phase by up to one cycle (modulation index 2 pi). Operator 4 can
//! feed back into itself (averaged over two samples, as classic FM chips do,
//! which keeps high feedback stable).
//!
//! Ported from `src/audio/synth/fm.ts` of the web version.

use crate::engine::{BaseVoice, DestDef, Env, Rng, Sound, VoiceEngine, COMMON_DESTS};
use crate::patch::EnvPatch;
use serde::Deserialize;
use serde_json::{json, Value};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FmWave {
    Sine,
    Half,
    Abs,
    Square,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct FmOp {
    pub on: bool,
    /// Frequency ratio to the note (ignored when `fixed`).
    pub ratio: f64,
    pub fixed: bool,
    /// Fixed frequency, Hz.
    pub freq: f64,
    /// Cents.
    pub detune: f64,
    /// Output level 0 to 1 (carrier volume, or modulation depth).
    pub level: f64,
    /// 0: velocity ignored, 1: level follows velocity fully.
    #[serde(rename = "velSens")]
    pub vel_sens: f64,
    pub wave: FmWave,
    pub env: EnvPatch,
}

/// Facet's own patch fields.
#[derive(Clone, Debug, Deserialize)]
pub struct FmPatch {
    pub ops: Vec<FmOp>,
    /// 0 to 7, see [`ALGORITHMS`].
    pub algo: f64,
    /// Operator 4 self-feedback, 0 to 1.
    pub feedback: f64,
}

/// An algorithm: `mods[i]` lists the operators that modulate operator `i`.
/// Higher operators modulate lower ones, so evaluating 3, 2, 1, 0 is always valid.
pub struct Algorithm {
    pub name: &'static str,
    pub mods: [&'static [usize]; 4],
    pub carriers: &'static [usize],
}

pub const ALGORITHMS: [Algorithm; 8] = [
    Algorithm {
        name: "4 > 3 > 2 > 1",
        mods: [&[1], &[2], &[3], &[]],
        carriers: &[0],
    },
    Algorithm {
        name: "(3 + 4) > 2 > 1",
        mods: [&[1], &[2, 3], &[], &[]],
        carriers: &[0],
    },
    Algorithm {
        name: "4 > 3 > 1, 2 > 1",
        mods: [&[1, 2], &[], &[3], &[]],
        carriers: &[0],
    },
    Algorithm {
        name: "2 > 1, 4 > 3",
        mods: [&[1], &[], &[3], &[]],
        carriers: &[0, 2],
    },
    Algorithm {
        name: "4 > (1, 2, 3)",
        mods: [&[3], &[3], &[3], &[]],
        carriers: &[0, 1, 2],
    },
    Algorithm {
        name: "4 > 3, 1, 2",
        mods: [&[], &[], &[3], &[]],
        carriers: &[0, 1, 2],
    },
    Algorithm {
        name: "(2 + 3 + 4) > 1",
        mods: [&[1, 2, 3], &[], &[], &[]],
        carriers: &[0],
    },
    Algorithm {
        name: "1, 2, 3, 4 (additive)",
        mods: [&[], &[], &[], &[]],
        carriers: &[0, 1, 2, 3],
    },
];

/// Facet's destinations (after the common ones): feedback, then level and pitch per operator.
pub const FM_DESTS: [DestDef; 9] = [
    DestDef {
        id: "fb",
        range: 1.0,
    },
    DestDef {
        id: "op0.level",
        range: 1.0,
    },
    DestDef {
        id: "op0.pitch",
        range: 24.0,
    },
    DestDef {
        id: "op1.level",
        range: 1.0,
    },
    DestDef {
        id: "op1.pitch",
        range: 24.0,
    },
    DestDef {
        id: "op2.level",
        range: 1.0,
    },
    DestDef {
        id: "op2.pitch",
        range: 24.0,
    },
    DestDef {
        id: "op3.level",
        range: 1.0,
    },
    DestDef {
        id: "op3.pitch",
        range: 24.0,
    },
];

/// Output trim: a full-level carrier is a full-scale sine.
pub const FM_TRIM: f64 = 0.3;

const TAU: f64 = 2.0 * PI;
const HALF_MEAN: f64 = 1.0 / PI;
const ABS_MEAN: f64 = 2.0 / PI;

fn wave(w: FmWave, ph: f64) -> f64 {
    let s = (TAU * ph).sin();
    match w {
        FmWave::Half => (if s > 0.0 { s } else { 0.0 }) * 2.0 - 2.0 * HALF_MEAN,
        FmWave::Abs => (if s < 0.0 { -s } else { s }) * 2.0 - 2.0 * ABS_MEAN,
        FmWave::Square => (4.0 * s).tanh() * (1.0 / 4f64.tanh()),
        FmWave::Sine => s,
    }
}

/// Per-voice state of the FM generator.
#[derive(Default)]
pub struct FmVoice {
    phase: [f64; 4],
    inc: [f64; 4],
    /// Operator amplitude at the start and end of the control block.
    a0: [f64; 4],
    a1: [f64; 4],
    env: [Env; 4],
    fb1: f64,
    fb2: f64,
    out: [f64; 4],
}

/// The FM sound generator (use through [`FmEngine`]).
pub struct Fm {
    patch: Option<FmPatch>,
    algo: usize,
    carrier_gain: f64,
}

const D_BASE: usize = COMMON_DESTS.len();
const D_FB: usize = D_BASE;
const fn d_level(i: usize) -> usize {
    D_BASE + 1 + 2 * i
}
const fn d_pitch(i: usize) -> usize {
    D_BASE + 2 + 2 * i
}

impl Fm {
    pub fn new() -> Self {
        Self {
            patch: None,
            algo: 0,
            carrier_gain: 1.0,
        }
    }
}

impl Default for Fm {
    fn default() -> Self {
        Self::new()
    }
}

/// The default Facet patch (common and FM fields) as JSON.
pub fn default_fm_json() -> Value {
    let op = |on: bool, ratio: f64, level: f64, vel: f64, env: Value| json!({ "on": on, "ratio": ratio, "fixed": false, "freq": 440, "detune": 0, "level": level, "velSens": vel, "wave": "sine", "env": env });
    let env = |a: f64, d: f64, s: f64, r: f64| json!({ "a": a, "h": 0, "d": d, "s": s, "r": r });
    json!({
        "env": [{ "a": 0.002, "h": 0, "d": 0.3, "s": 1, "r": 0.6 }, { "a": 0.01, "h": 0, "d": 0.6, "s": 0, "r": 0.3 }],
        "ops": [
            op(true, 1.0, 0.9, 0.3, env(0.002, 1.2, 0.6, 0.5)),
            op(true, 2.0, 0.35, 0.5, env(0.002, 0.8, 0.25, 0.4)),
            op(false, 3.0, 0.2, 0.5, env(0.002, 0.6, 0.4, 0.4)),
            op(false, 1.0, 0.2, 0.5, env(0.002, 0.6, 0.4, 0.4)),
        ],
        "algo": 0,
        "feedback": 0,
    })
}

impl Sound for Fm {
    type Patch = FmPatch;
    type Voice = FmVoice;

    fn default_json() -> Value {
        default_fm_json()
    }

    fn extra_dests() -> &'static [DestDef] {
        &FM_DESTS
    }

    fn output_trim() -> f64 {
        FM_TRIM
    }

    fn new_voice(&self) -> FmVoice {
        FmVoice::default()
    }

    fn patch_changed(&mut self, patch: FmPatch) -> Option<FmPatch> {
        self.algo = (patch.algo + 0.5)
            .floor()
            .clamp(0.0, (ALGORITHMS.len() - 1) as f64) as usize;
        let carriers = ALGORITHMS[self.algo]
            .carriers
            .iter()
            .filter(|c| patch.ops.get(**c).is_some_and(|o| o.on))
            .count();
        self.carrier_gain = 1.0 / (carriers.max(1) as f64);
        self.patch.replace(patch)
    }

    fn start(&mut self, v: &mut FmVoice, _base: &BaseVoice, legato: bool) {
        if legato {
            return;
        }
        for i in 0..4 {
            v.env[i].trigger(true);
            v.phase[i] = 0.0;
            v.a0[i] = 0.0;
            v.a1[i] = 0.0;
        }
        v.fb1 = 0.0;
        v.fb2 = 0.0;
    }

    fn control(&mut self, v: &mut FmVoice, base: &BaseVoice, n: usize, pitch: f64, sr: f64) {
        let Some(p) = &self.patch else { return };
        let m = &base.mods;
        let freq_base = 440.0 * 2f64.powf((pitch - 69.0) / 12.0);
        for i in 0..4 {
            v.a0[i] = v.a1[i];
            let Some(op) = p.ops.get(i).filter(|o| o.on) else {
                v.a1[i] = 0.0;
                continue;
            };
            if base.released {
                v.env[i].release();
            }
            let e = v.env[i].step(&op.env, n as f64, sr);
            let vs = op.vel_sens.clamp(0.0, 1.0);
            let level = (op.level + m[d_level(i)]).max(0.0);
            v.a1[i] = e * level * (1.0 - vs + vs * base.vel);
            let semis = m[d_pitch(i)] + op.detune / 100.0;
            let f = (if op.fixed {
                op.freq
            } else {
                freq_base * op.ratio
            }) * 2f64.powf(semis / 12.0);
            v.inc[i] = (f / sr).clamp(0.0, 0.49);
        }
    }

    fn render(
        &mut self,
        v: &mut FmVoice,
        base: &BaseVoice,
        _rng: &mut Rng,
        l: &mut [f32],
        r: &mut [f32],
        n: usize,
    ) {
        let Some(p) = &self.patch else { return };
        let algo = &ALGORITHMS[self.algo];
        let fb = (p.feedback + base.mods[D_FB]).clamp(0.0, 1.0) * 0.5;
        let ons: [bool; 4] = std::array::from_fn(|i| p.ops.get(i).is_some_and(|o| o.on));
        let waves: [FmWave; 4] =
            std::array::from_fn(|i| p.ops.get(i).map_or(FmWave::Sine, |o| o.wave));
        for s in 0..n {
            let k = (s as f64 + 1.0) / n as f64;
            for i in (0..4).rev() {
                if !ons[i] {
                    v.out[i] = 0.0;
                    continue;
                }
                let mut pm = 0.0;
                for &j in algo.mods[i] {
                    pm += v.out[j];
                }
                if i == 3 && fb > 0.0 {
                    pm += ((v.fb1 + v.fb2) * 0.5) * fb;
                }
                let amp = v.a0[i] + (v.a1[i] - v.a0[i]) * k;
                let mut ph = v.phase[i] + v.inc[i];
                if ph >= 1.0 {
                    ph -= 1.0;
                }
                v.phase[i] = ph;
                v.out[i] = wave(waves[i], ph + pm) * amp;
            }
            v.fb2 = v.fb1;
            v.fb1 = v.out[3];
            let mut y = 0.0;
            for &c in algo.carriers {
                y += v.out[c];
            }
            y *= self.carrier_gain;
            l[s] = y as f32;
            r[s] = y as f32;
        }
    }
}

/// The Facet engine.
pub type FmEngine = VoiceEngine<Fm>;

/// A Facet engine at `sample_rate` with a random `seed` (renders repeat exactly).
pub fn fm_engine(sample_rate: f64, seed: u32) -> FmEngine {
    VoiceEngine::new(Fm::new(), sample_rate, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Event;
    use serde_json::json;

    fn render(e: &mut FmEngine, seconds: f64) -> Vec<f32> {
        let n = (seconds * e.sample_rate()) as usize;
        let mut l = vec![0f32; n];
        let mut r = vec![0f32; n];
        let mut i = 0;
        while i < n {
            let end = (i + 128).min(n);
            e.process(&mut l[i..end], &mut r[i..end], i as f64 / e.sample_rate());
            i = end;
        }
        l
    }

    #[test]
    fn the_default_patch_makes_sound_then_goes_quiet() {
        let mut e = fm_engine(48000.0, 1);
        e.queue(&[
            Event::On {
                time: 0.0,
                key: 69.0,
                vel: 1.0,
                fine: 0.0,
                pan: 0.0,
            },
            Event::Off {
                time: 0.3,
                key: 69.0,
            },
        ]);
        let out = render(&mut e, 1.5);
        let peak = out[..14400].iter().fold(0f32, |a, b| a.max(b.abs()));
        let tail = out[out.len() - 4800..]
            .iter()
            .fold(0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.05 && peak < 1.0, "peak {peak}");
        assert!(tail < 1e-4, "tail {tail}");
        assert_eq!(e.active_voices(), 0);
    }

    #[test]
    fn a_single_carrier_plays_the_note_frequency() {
        let mut e = fm_engine(48000.0, 1);
        e.set_patch(&json!({
            "level": 1, "voice": { "velSens": 0 },
            "env": [{ "a": 0.0005, "d": 1, "s": 1, "r": 0.05 }],
            "ops": [{ "level": 1, "velSens": 0, "env": { "a": 0.0005, "d": 1, "s": 1 } }, { "on": false }, { "on": false }, { "on": false }],
        })).unwrap();
        e.queue(&[Event::On {
            time: 0.0,
            key: 69.0,
            vel: 1.0,
            fine: 0.0,
            pan: 0.0,
        }]);
        let out = render(&mut e, 0.5);
        // single-bin DFT at 440 Hz over whole periods (0.1 s to 0.3 s)
        let (from, len) = (4800usize, 9600usize);
        let (mut re, mut im) = (0.0, 0.0);
        for i in 0..len {
            let ph = TAU * 440.0 * i as f64 / 48000.0;
            re += f64::from(out[from + i]) * ph.cos();
            im += f64::from(out[from + i]) * ph.sin();
        }
        let amp = 2.0 * re.hypot(im) / len as f64 / FM_TRIM;
        assert!((amp - 1.0).abs() < 0.01, "amplitude {amp}");
    }

    #[test]
    fn renders_repeat_exactly() {
        let run = || {
            let mut e = fm_engine(48000.0, 7);
            e.set_patch(&json!({ "lfo": [{ "shape": "sh", "rate": 9 }], "mod": [{ "src": "lfo1", "dest": "op1.level", "amt": 0.4 }] })).unwrap();
            e.queue(&[Event::On {
                time: 0.0,
                key: 60.0,
                vel: 0.7,
                fine: 0.0,
                pan: 0.0,
            }]);
            render(&mut e, 0.3)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn the_prng_is_mulberry32() {
        // the first values of mulberry32 with seed 1, as the web version produces them
        let mut r = crate::engine::Rng::new(1);
        for want in [
            0.627_073_940_588_161_3,
            0.002_735_721_180_215_478,
            0.527_447_039_959_952_2,
        ] {
            assert_eq!(r.next_f64(), want);
        }
    }
}
