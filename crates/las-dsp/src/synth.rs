// SPDX-License-Identifier: GPL-3.0-or-later
//! The wavetable synth engine behind Polaris, Vortex, Monolith, Zenith, Nebula
//! and TriOsc: four wavetable oscillators with unison and warp, a sub oscillator,
//! noise, two filters (twelve types), four envelopes, four LFOs and a modulation
//! matrix.
//!
//! Ported from `src/audio/worklets/synth.worklet.ts` and `src/audio/synth/patch.ts`
//! of the web version, step by step, so that the same patch and notes render the
//! same audio (see `docs/PARITY.md`).
//!
//! Differences from the original, on purpose:
//! - Random numbers come from the engine's seeded stream, not `Math.random`, so
//!   renders repeat. (Parity tests therefore leave random phase, noise,
//!   sample-and-hold and the `rand` source out.)
//! - Wavetables are built ahead of time by a [`TableLibrary`], not on the audio
//!   thread, and a patch is parsed ahead of time too ([`prepare_synth_patch`]).
//!
//! Not ported yet: parameter ramps (automation) and tempo changes over time.

// The index loops follow the original line by line, which keeps the port easy to check.
#![allow(clippy::needless_range_loop)]

use crate::engine::{js_round, Cc, Env, Event, PatchError, Playable, Rng};
use crate::patch::{
    merge_patch, set_path, EnvPatch, LfoPatch, LfoShape, ModSlot, Ramps, VoicePatch,
};
use crate::wavetable::{max_harmonics, Table, TableLibrary, MIP_LEVELS};
use serde::Deserialize;
use serde_json::{json, Value};
use std::f64::consts::{PI, SQRT_2};
use std::sync::Arc;

const MAX_UNISON: usize = 16;
const CONTROL: usize = 32;
const TAU: f64 = PI * 2.0;
/// Voices an engine keeps ready: 32 polyphony plus 4 for the release tails.
pub const MAX_VOICES: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum WarpMode {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "sync")]
    Sync,
    #[serde(rename = "bend+")]
    BendUp,
    #[serde(rename = "bend-")]
    BendDown,
    #[serde(rename = "pwm")]
    Pwm,
    #[serde(rename = "mirror")]
    Mirror,
    #[serde(rename = "fm")]
    Fm,
    #[serde(rename = "ring")]
    Ring,
    #[serde(rename = "quantize")]
    Quantize,
    #[serde(rename = "asym")]
    Asym,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum FilterType {
    #[serde(rename = "lp12")]
    Lp12,
    #[serde(rename = "lp24")]
    Lp24,
    #[serde(rename = "hp12")]
    Hp12,
    #[serde(rename = "hp24")]
    Hp24,
    #[serde(rename = "bp12")]
    Bp12,
    #[serde(rename = "bp24")]
    Bp24,
    #[serde(rename = "notch")]
    Notch,
    #[serde(rename = "peak")]
    Peak,
    #[serde(rename = "ladder")]
    Ladder,
    #[serde(rename = "formant")]
    Formant,
    #[serde(rename = "comb")]
    Comb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Route {
    A,
    B,
    #[serde(rename = "AB")]
    Ab,
    #[serde(rename = "direct")]
    Direct,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OscPatch {
    pub on: bool,
    pub table: String,
    pub pos: f64,
    pub oct: f64,
    pub semi: f64,
    pub fine: f64,
    pub level: f64,
    pub pan: f64,
    pub voices: f64,
    pub detune: f64,
    pub blend: f64,
    pub spread: f64,
    pub phase: f64,
    pub rand_phase: f64,
    pub warp: WarpMode,
    pub warp_amt: f64,
    pub route: Route,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SubPatch {
    pub on: bool,
    pub wave: String,
    pub oct: f64,
    pub level: f64,
    pub route: Route,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseColor {
    White,
    Pink,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NoisePatch {
    pub on: bool,
    pub color: NoiseColor,
    pub level: f64,
    pub route: Route,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterPatch {
    pub on: bool,
    #[serde(rename = "type")]
    pub kind: FilterType,
    pub cutoff: f64,
    pub res: f64,
    pub drive: f64,
    pub keytrack: f64,
    /// Envelope 2 amount in octaves.
    pub env_amt: f64,
    /// Velocity amount in octaves.
    pub vel_amt: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Routing {
    Serial,
    Parallel,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SynthPatch {
    pub osc: Vec<OscPatch>,
    pub sub: SubPatch,
    pub noise: NoisePatch,
    pub filters: Vec<FilterPatch>,
    pub filter_routing: Routing,
    /// `env[0]` amp, `env[1]` filter (env2), `env[2]` env3, `env[3]` env4.
    pub env: Vec<EnvPatch>,
    pub lfo: Vec<LfoPatch>,
    #[serde(rename = "mod")]
    pub mods: Vec<ModSlot>,
    pub macros: Vec<f64>,
    pub voice: VoicePatch,
    /// Master output level 0 to 1.
    pub level: f64,
    /// Master tune in semitones.
    pub tune: f64,
}

/// Modulation sources, in the order of the source values.
pub const MOD_SOURCES: [&str; 21] = [
    "env2", "env3", "env4", "lfo1", "lfo2", "lfo3", "lfo4", "vel", "note", "mw", "at", "pb",
    "rand", "macro1", "macro2", "macro3", "macro4", "macro5", "macro6", "macro7", "macro8",
];

/// Per-voice modulation destinations: id and the value of amount = 1.
pub fn mod_destinations() -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = vec![
        ("pitch".into(), 24.0),
        ("amp".into(), 1.0),
        ("pan".into(), 1.0),
    ];
    for i in 0..4 {
        out.push((format!("osc{i}.pitch"), 24.0));
        for n in ["pos", "level", "pan", "detune", "warpAmt"] {
            out.push((format!("osc{i}.{n}"), 1.0));
        }
    }
    out.push(("sub.level".into(), 1.0));
    out.push(("noise.level".into(), 1.0));
    for i in 0..2 {
        out.push((format!("f{i}.cutoff"), 8.0));
        out.push((format!("f{i}.res"), 1.0));
        out.push((format!("f{i}.drive"), 1.0));
    }
    out
}

const D_PITCH: usize = 0;
const D_AMP: usize = 1;
const D_PAN: usize = 2;
const D_SUB_LEVEL: usize = 27;
const D_NOISE_LEVEL: usize = 28;
const NUM_DESTS: usize = 35;

/// The default patch (the same for every instrument built on this engine).
pub fn default_synth_json() -> Value {
    let osc = |on: bool, table: &str, route: &str| {
        json!({
            "on": on, "table": table, "pos": 0, "oct": 0, "semi": 0, "fine": 0,
            "level": 0.75, "pan": 0, "voices": 1, "detune": 0.2, "blend": 0.75,
            "spread": 0.6, "phase": 0, "randPhase": 1, "warp": "none", "warpAmt": 0,
            "route": route
        })
    };
    let filter = |on: bool, kind: &str, cutoff: f64| json!({ "on": on, "type": kind, "cutoff": cutoff, "res": 0.1, "drive": 0, "keytrack": 0, "envAmt": 0, "velAmt": 0 });
    let env = |a: f64, d: f64, s: f64, r: f64| json!({ "a": a, "h": 0, "d": d, "s": s, "r": r });
    let lfo = |shape: &str, rate: f64, sync: bool, div: f64| json!({ "shape": shape, "rate": rate, "sync": sync, "div": div, "retrig": false, "phase": 0, "delay": 0 });
    json!({
        "osc": [osc(true, "saw", "A"), osc(false, "square", "A"), osc(false, "saw", "B"), osc(false, "saw", "B")],
        "sub": { "on": false, "wave": "sine", "oct": -1, "level": 0.5, "route": "direct" },
        "noise": { "on": false, "color": "white", "level": 0.2, "route": "A" },
        "filters": [filter(true, "lp24", 18000.0), filter(false, "hp12", 20.0)],
        "filterRouting": "serial",
        "env": [env(0.003, 0.3, 0.8, 0.25), env(0.003, 0.5, 0.0, 0.25), env(0.003, 0.6, 0.0, 0.25), env(0.5, 0.3, 1.0, 1.0)],
        "lfo": [lfo("sine", 2.0, false, 1.0), lfo("triangle", 0.3, false, 1.0), lfo("saw", 2.0, true, 0.25), lfo("sh", 6.0, false, 1.0)],
        "mod": [],
        "macros": [0, 0, 0, 0, 0, 0, 0, 0],
        "voice": { "poly": 8, "mono": false, "legato": false, "glide": 0, "pbRange": 2, "velSens": 0.6 },
        "level": 0.7,
        "tune": 0
    })
}

/// A patch parsed, checked and given its wavetables, ready for
/// [`SynthEngine::apply_patch`].
pub struct PreparedSynthPatch {
    json: Value,
    patch: SynthPatch,
    tables: [Arc<Table>; 4],
    sub_table: Arc<Table>,
    mod_src: Vec<usize>,
    mod_dst: Vec<usize>,
    mod_amt: Vec<f64>,
}

/// Parse a patch (only the values that differ from the defaults need to be
/// present) and build the wavetables it uses. Do this off the audio thread.
pub fn prepare_synth_patch(
    lib: &mut TableLibrary,
    over: &Value,
) -> Result<PreparedSynthPatch, PatchError> {
    let merged = merge_patch(&default_synth_json(), over);
    let patch: SynthPatch =
        serde_json::from_value(merged.clone()).map_err(|e| PatchError(e.to_string()))?;
    if patch.osc.len() < 4 || patch.filters.len() < 2 || patch.env.len() < 4 || patch.lfo.len() < 4
    {
        return Err(PatchError(
            "a synth patch needs 4 oscillators, 2 filters, 4 envelopes and 4 LFOs".into(),
        ));
    }
    let dests = mod_destinations();
    let mut mod_src = Vec::new();
    let mut mod_dst = Vec::new();
    let mut mod_amt = Vec::new();
    for m in &patch.mods {
        let src = MOD_SOURCES.iter().position(|x| *x == m.src);
        let dst = dests.iter().position(|d| d.0 == m.dest);
        if let (Some(src), Some(dst)) = (src, dst) {
            if m.amt != 0.0 {
                mod_src.push(src);
                mod_dst.push(dst);
                mod_amt.push(m.amt);
            }
        }
    }
    let tables = std::array::from_fn(|i| {
        let o = &patch.osc[i];
        lib.get(if o.on { &o.table } else { "saw" })
    });
    let sub_table = lib.get(if patch.sub.on { &patch.sub.wave } else { "saw" });
    Ok(PreparedSynthPatch {
        json: merged,
        patch,
        tables,
        sub_table,
        mod_src,
        mod_dst,
        mod_amt,
    })
}

// ---------------------------------------------------------------------------
// Filter (stereo)
// ---------------------------------------------------------------------------
const VOWEL_F: [[f64; 3]; 5] = [
    [800.0, 1150.0, 2900.0],
    [400.0, 1600.0, 2700.0],
    [270.0, 2140.0, 2950.0],
    [450.0, 800.0, 2830.0],
    [325.0, 700.0, 2530.0],
];

struct StereoFilter {
    kind: FilterType,
    // SVF states: [stage * 2 + channel]
    ic1: [f64; 6],
    ic2: [f64; 6],
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
    // formant coefficients (3 bands)
    fa1: [f64; 3],
    fa2: [f64; 3],
    fa3: [f64; 3],
    fk: f64,
    // ladder state per channel (8 values per channel)
    lad: [f64; 16],
    lad_f: f64,
    lad_fb: f64,
    // comb
    comb: [Vec<f32>; 2],
    comb_pos: usize,
    comb_len: usize,
    comb_fb: f64,
    drive: f64,
    drive_gain: f64,
}

impl StereoFilter {
    fn new(sr: f64) -> Self {
        // the comb delay lines are made up front, so no filter change allocates
        let len = (sr / 15.0).ceil() as usize;
        Self {
            kind: FilterType::Lp24,
            ic1: [0.0; 6],
            ic2: [0.0; 6],
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            k: 1.0,
            fa1: [0.0; 3],
            fa2: [0.0; 3],
            fa3: [0.0; 3],
            fk: 1.0,
            lad: [0.0; 16],
            lad_f: 0.0,
            lad_fb: 0.0,
            comb: [vec![0.0; len], vec![0.0; len]],
            comb_pos: 0,
            comb_len: 1,
            comb_fb: 0.0,
            drive: 0.0,
            drive_gain: 1.0,
        }
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 6];
        self.ic2 = [0.0; 6];
        self.lad = [0.0; 16];
        for c in &mut self.comb {
            c.fill(0.0);
        }
    }

    fn setup(&mut self, kind: FilterType, cutoff: f64, res: f64, drive: f64, sr: f64) {
        self.kind = kind;
        let fc = cutoff.max(16.0).min(sr * 0.45);
        let res = res.clamp(0.0, 1.0);
        self.drive = drive;
        self.drive_gain = 1.0 + drive * 8.0;
        match kind {
            FilterType::Ladder => {
                let f = ((fc / sr) * 2.2).min(1.0);
                self.lad_f = f;
                self.lad_fb = res * 4.2 * (1.0 - 0.15 * f * f);
            }
            FilterType::Formant => {
                let pos = ((fc / 20.0).log2() / 1000f64.log2()).clamp(0.0, 0.999)
                    * (VOWEL_F.len() - 1) as f64;
                let i = pos.floor() as usize;
                let t = pos - i as f64;
                let q = 4.0 + res * 16.0;
                self.fk = 1.0 / q;
                for b in 0..3 {
                    let f = VOWEL_F[i][b] * (1.0 - t) + VOWEL_F[(i + 1).min(4)][b] * t;
                    let g = (PI * f.min(sr * 0.45) / sr).tan();
                    let a1 = 1.0 / (1.0 + g * (g + self.fk));
                    self.fa1[b] = a1;
                    self.fa2[b] = g * a1;
                    self.fa3[b] = g * g * a1;
                }
            }
            FilterType::Comb => {
                let max = (self.comb[0].len() - 1) as f64;
                self.comb_len = 2f64.max(max.min(js_round(sr / fc))) as usize;
                self.comb_fb = res * 0.97;
            }
            _ => {
                let g = (PI * fc / sr).tan();
                let k = 2.0 - 2.0 * res * 0.985;
                self.k = k;
                self.a1 = 1.0 / (1.0 + g * (g + k));
                self.a2 = g * self.a1;
                self.a3 = g * self.a2;
            }
        }
    }

    fn svf(&mut self, stage: usize, ch: usize, v0: f64) -> f64 {
        let idx = stage * 2 + ch;
        let (ic1, ic2) = (self.ic1[idx], self.ic2[idx]);
        let v3 = v0 - ic2;
        let v1 = self.a1 * ic1 + self.a2 * v3;
        let v2 = ic2 + self.a2 * ic1 + self.a3 * v3;
        self.ic1[idx] = 2.0 * v1 - ic1;
        self.ic2[idx] = 2.0 * v2 - ic2;
        match self.kind {
            FilterType::Hp12 | FilterType::Hp24 => v0 - self.k * v1 - v2,
            FilterType::Bp12 | FilterType::Bp24 => v1 * self.k,
            FilterType::Notch => v0 - self.k * v1,
            FilterType::Peak => v0 + 2.0 * (2.0 - self.k) * v1 * self.k,
            _ => v2,
        }
    }

    fn process(&mut self, ch: usize, mut x: f64) -> f64 {
        if self.drive > 0.0 {
            x = (x * self.drive_gain).tanh() / self.drive_gain.tanh() * (1.0 + self.drive);
        }
        match self.kind {
            FilterType::Lp24 | FilterType::Hp24 | FilterType::Bp24 => {
                let a = self.svf(0, ch, x);
                self.svf(1, ch, a)
            }
            FilterType::Ladder => {
                let s = &mut self.lad;
                let o = ch * 8;
                let f = self.lad_f;
                let mut input = (x - s[o + 3] * self.lad_fb).tanh();
                input *= 0.35013 * f * f * f * f;
                s[o] = input + 0.3 * s[o + 4] + (1.0 - f) * s[o];
                s[o + 4] = input;
                s[o + 1] = s[o] + 0.3 * s[o + 5] + (1.0 - f) * s[o + 1];
                s[o + 5] = s[o];
                s[o + 2] = s[o + 1] + 0.3 * s[o + 6] + (1.0 - f) * s[o + 2];
                s[o + 6] = s[o + 1];
                s[o + 3] = s[o + 2] + 0.3 * s[o + 7] + (1.0 - f) * s[o + 3];
                s[o + 7] = s[o + 2];
                s[o + 3] * 1.6
            }
            FilterType::Formant => {
                let mut sum = 0.0;
                for b in 0..3 {
                    let idx = b * 2 + ch;
                    let (ic1, ic2) = (self.ic1[idx], self.ic2[idx]);
                    let v3 = x - ic2;
                    let v1 = self.fa1[b] * ic1 + self.fa2[b] * v3;
                    let v2 = ic2 + self.fa2[b] * ic1 + self.fa3[b] * v3;
                    self.ic1[idx] = 2.0 * v1 - ic1;
                    self.ic2[idx] = 2.0 * v2 - ic2;
                    let w = if b == 0 {
                        1.0
                    } else if b == 1 {
                        0.7
                    } else {
                        0.45
                    };
                    sum += v1 * w;
                }
                sum * 1.5
            }
            FilterType::Comb => {
                let len = self.comb[ch].len();
                let mut r = self.comb_pos as isize - self.comb_len as isize;
                if r < 0 {
                    r += len as isize;
                }
                let y = x + f64::from(self.comb[ch][r as usize]) * self.comb_fb;
                self.comb[ch][self.comb_pos] = y.tanh() as f32;
                if ch == 1 {
                    self.comb_pos = (self.comb_pos + 1) % len;
                }
                y * 0.6
            }
            _ => self.svf(0, ch, x),
        }
    }
}

// ---------------------------------------------------------------------------
// Voice
// ---------------------------------------------------------------------------
struct OscRt {
    level: f64,
    /// Which mip level and which two frames to read, and the blend between them.
    mip: usize,
    f0: usize,
    f1: usize,
    ft: f64,
    size: usize,
    mask: usize,
    /// Per unison voice. The pan gains are `f32`, as in the original.
    inc: [f64; MAX_UNISON],
    pan_l: [f32; MAX_UNISON],
    pan_r: [f32; MAX_UNISON],
    voices: usize,
    warp_amt: f64,
    gain: f64,
}

impl Default for OscRt {
    fn default() -> Self {
        Self {
            level: 0.0,
            mip: 0,
            f0: 0,
            f1: 0,
            ft: 0.0,
            size: 1,
            mask: 0,
            inc: [0.0; MAX_UNISON],
            pan_l: [0.0; MAX_UNISON],
            pan_r: [0.0; MAX_UNISON],
            voices: 1,
            warp_amt: 0.0,
            gain: 1.0,
        }
    }
}

struct Voice {
    active: bool,
    released: bool,
    key: f64,
    vel: f64,
    fine: f64,
    note_pan: f64,
    age: u64,
    pitch: f64,
    target_pitch: f64,
    rand: f64,
    killing: bool,
    kill_gain: f64,
    env: [Env; 4],
    lfo_phase: [f64; 4],
    lfo_hold: [f64; 4],
    lfo_prev: [f64; 4],
    lfo_time: f64,
    phases: [[f64; MAX_UNISON]; 4],
    sub_phase: f64,
    pink: [f64; 7],
    filters: [StereoFilter; 2],
    osc_out: [f64; 4],
    mods: [f64; NUM_DESTS],
    rt: [OscRt; 4],
}

impl Voice {
    fn new(sr: f64) -> Self {
        Self {
            active: false,
            released: false,
            key: 60.0,
            vel: 1.0,
            fine: 0.0,
            note_pan: 0.0,
            age: 0,
            pitch: 60.0,
            target_pitch: 60.0,
            rand: 0.0,
            killing: false,
            kill_gain: 1.0,
            env: Default::default(),
            lfo_phase: [0.0; 4],
            lfo_hold: [0.0; 4],
            lfo_prev: [0.0; 4],
            lfo_time: 0.0,
            phases: [[0.0; MAX_UNISON]; 4],
            sub_phase: 0.0,
            pink: [0.0; 7],
            filters: [StereoFilter::new(sr), StereoFilter::new(sr)],
            osc_out: [0.0; 4],
            mods: [0.0; NUM_DESTS],
            rt: Default::default(),
        }
    }
}

fn lfo_shape(
    shape: LfoShape,
    ph: f64,
    v: &mut Voice,
    i: usize,
    wrapped: bool,
    rng: &mut Rng,
) -> f64 {
    match shape {
        LfoShape::Sine => (TAU * ph).sin(),
        LfoShape::Triangle => {
            if ph < 0.25 {
                ph * 4.0
            } else if ph < 0.75 {
                2.0 - ph * 4.0
            } else {
                ph * 4.0 - 4.0
            }
        }
        LfoShape::Saw => 1.0 - 2.0 * ph,
        LfoShape::Ramp => 2.0 * ph - 1.0,
        LfoShape::Square => {
            if ph < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::Sh => {
            if wrapped {
                v.lfo_hold[i] = rng.next_f64() * 2.0 - 1.0;
            }
            v.lfo_hold[i]
        }
        LfoShape::Smooth => {
            if wrapped {
                v.lfo_prev[i] = v.lfo_hold[i];
                v.lfo_hold[i] = rng.next_f64() * 2.0 - 1.0;
            }
            let t = 0.5 - 0.5 * (PI * ph).cos();
            v.lfo_prev[i] + (v.lfo_hold[i] - v.lfo_prev[i]) * t
        }
    }
}

/// The polyphonic wavetable synth.
pub struct SynthEngine {
    patch: SynthPatch,
    patch_json: Value,
    ramps: Ramps,
    tables: [Arc<Table>; 4],
    sub_table: Arc<Table>,
    lib: TableLibrary,
    voices: Vec<Voice>,
    events: Vec<Event>,
    held_keys: Vec<f64>,
    last_key: f64,
    age_counter: u64,
    mw: f64,
    pb: f64,
    at: f64,
    bpm: f64,
    anchor_time: f64,
    anchor_beat: f64,
    mod_src: Vec<usize>,
    mod_dst: Vec<usize>,
    mod_amt: Vec<f64>,
    dest_range: Vec<f64>,
    src_vals: [f64; 21],
    sr: f64,
    rng: Rng,
    /// Engine time at the start of the block being rendered.
    now: f64,
}

impl Playable for SynthEngine {
    fn sample_rate(&self) -> f64 {
        SynthEngine::sample_rate(self)
    }
    fn set_patch(&mut self, over: &Value) -> Result<(), PatchError> {
        SynthEngine::set_patch(self, over)
    }
    fn queue(&mut self, evs: &[Event]) {
        SynthEngine::queue(self, evs)
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32], t0: f64) {
        SynthEngine::process(self, l, r, t0)
    }
    fn active_voices(&self) -> usize {
        SynthEngine::active_voices(self)
    }
    fn all_off(&mut self) {
        SynthEngine::all_off(self)
    }
}

pub fn synth_engine(sample_rate: f64, seed: u32) -> SynthEngine {
    SynthEngine::new(sample_rate, seed)
}

impl SynthEngine {
    pub fn new(sample_rate: f64, seed: u32) -> Self {
        let mut lib = TableLibrary::new();
        let mut first = prepare_synth_patch(&mut lib, &json!({})).expect("the defaults are valid");
        let mut e = Self {
            patch: first.patch.clone(),
            patch_json: first.json.clone(),
            ramps: Ramps::default(),
            tables: first.tables.clone(),
            sub_table: first.sub_table.clone(),
            lib,
            voices: (0..MAX_VOICES).map(|_| Voice::new(sample_rate)).collect(),
            events: Vec::new(),
            held_keys: Vec::new(),
            last_key: -1.0,
            age_counter: 0,
            mw: 0.0,
            pb: 0.0,
            at: 0.0,
            bpm: 120.0,
            anchor_time: 0.0,
            anchor_beat: 0.0,
            mod_src: Vec::new(),
            mod_dst: Vec::new(),
            mod_amt: Vec::new(),
            dest_range: mod_destinations().iter().map(|d| d.1).collect(),
            src_vals: [0.0; 21],
            sr: sample_rate,
            rng: Rng::new(seed),
            now: 0.0,
        };
        e.apply_patch(&mut first);
        e
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// Use these wavetables (with the user ones in them) for patches set from now on.
    pub fn set_tables(&mut self, lib: TableLibrary) {
        self.lib = lib;
    }

    /// Add a user wavetable (single-cycle frames). It wins over a built-in
    /// table of the same id the next time a patch is set.
    pub fn register_table(&mut self, id: &str, frames: &[Vec<f32>]) {
        self.lib
            .register(id, crate::wavetable::table_from_frames(frames));
    }

    /// Set the patch from JSON. Only the values that differ from the defaults
    /// need to be present.
    pub fn set_patch(&mut self, over: &Value) -> Result<(), PatchError> {
        let mut p = prepare_synth_patch(&mut self.lib, over)?;
        self.apply_patch(&mut p);
        // automated values go over the new patch
        self.ramps.forget();
        Ok(())
    }

    /// Automate a patch path (for example `filters.0.cutoff`) through `points`
    /// (time, value); `jump` steps to the first point. Values are read at the start
    /// of every control block.
    pub fn ramp(&mut self, path: &str, points: &[(f64, f64)], jump: bool) {
        self.ramps.ramp(path, points, jump);
    }

    /// Drop all automation.
    pub fn clear_ramps(&mut self) {
        self.ramps.clear();
    }

    fn apply_ramps(&mut self, time: f64) {
        if self.ramps.is_empty() {
            return;
        }
        let due = self.ramps.due(time);
        if due.is_empty() {
            return;
        }
        let mut json = self.patch_json.clone();
        for (path, v) in due {
            set_path(&mut json, &path, v.into());
        }
        if let Ok(mut p) = prepare_synth_patch(&mut self.lib, &json) {
            self.apply_patch(&mut p);
        }
    }

    /// Switch to a patch made by [`prepare_synth_patch`]. Does no parsing, no
    /// table building, no allocation and no freeing, so it is safe on the audio
    /// thread: the patch trades places with the current one, which stays in `p`
    /// for the caller to drop elsewhere (or to switch back to).
    pub fn apply_patch(&mut self, p: &mut PreparedSynthPatch) {
        std::mem::swap(&mut self.patch, &mut p.patch);
        std::mem::swap(&mut self.patch_json, &mut p.json);
        std::mem::swap(&mut self.tables, &mut p.tables);
        std::mem::swap(&mut self.sub_table, &mut p.sub_table);
        std::mem::swap(&mut self.mod_src, &mut p.mod_src);
        std::mem::swap(&mut self.mod_dst, &mut p.mod_dst);
        std::mem::swap(&mut self.mod_amt, &mut p.mod_amt);
    }

    /// Tempo and the beat at a point in time, for synced LFOs.
    pub fn set_transport(&mut self, bpm: f64, anchor_time: f64, anchor_beat: f64) {
        self.bpm = bpm;
        self.anchor_time = anchor_time;
        self.anchor_beat = anchor_beat;
    }

    /// Add timed events (kept in time order, equal times in the order given).
    pub fn queue(&mut self, evs: &[Event]) {
        self.events.extend_from_slice(evs);
        self.events.sort_by(|a, b| a.time().total_cmp(&b.time()));
    }

    /// Number of sounding voices.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    // --- notes ---------------------------------------------------------------
    fn alloc_voice(&mut self) -> usize {
        let limit = if self.patch.voice.mono {
            1.0
        } else {
            self.patch.voice.poly.max(1.0)
        };
        let playing = self
            .voices
            .iter()
            .filter(|v| v.active && !v.killing)
            .count() as f64;
        if playing >= limit {
            // steal: oldest released, else oldest
            let mut victim: Option<usize> = None;
            for (i, v) in self.voices.iter().enumerate() {
                if v.active
                    && !v.killing
                    && v.released
                    && victim.is_none_or(|j| v.age < self.voices[j].age)
                {
                    victim = Some(i);
                }
            }
            if victim.is_none() {
                for (i, v) in self.voices.iter().enumerate() {
                    if v.active && !v.killing && victim.is_none_or(|j| v.age < self.voices[j].age) {
                        victim = Some(i);
                    }
                }
            }
            if let Some(j) = victim {
                self.voices[j].killing = true;
            }
        }
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        // all slots busy: reuse the oldest
        let mut oldest = 0;
        for (i, v) in self.voices.iter().enumerate() {
            if v.age < self.voices[oldest].age {
                oldest = i;
            }
        }
        oldest
    }

    fn beat_at(&self, time: f64) -> f64 {
        self.anchor_beat + ((time - self.anchor_time) * self.bpm) / 60.0
    }

    fn start_voice(
        &mut self,
        idx: usize,
        key: f64,
        vel: f64,
        fine: f64,
        pan: f64,
        legato_from: Option<f64>,
    ) {
        let glide = self.patch.voice.glide;
        let legato = self.patch.voice.legato;
        let beat = self.beat_at(self.now);
        let now = self.now;
        self.age_counter += 1;
        let age = self.age_counter;
        let last_key = self.last_key;
        let rand = self.rng.next_f64() * 2.0 - 1.0;
        let v = &mut self.voices[idx];
        let was_active = v.active && !v.released;
        v.active = true;
        v.released = false;
        v.killing = false;
        v.kill_gain = 1.0;
        v.key = key;
        v.vel = vel;
        v.fine = fine;
        v.note_pan = pan;
        v.age = age;
        v.rand = rand;
        v.target_pitch = key;
        let from = legato_from.unwrap_or(if glide > 0.0 && last_key >= 0.0 {
            last_key
        } else {
            key
        });
        v.pitch = if glide > 0.0 { from } else { key };
        if legato_from.is_some() && was_active {
            return; // legato: keep envelopes running
        }
        for e in &mut v.env {
            e.trigger(!legato || !was_active);
        }
        v.lfo_time = 0.0;
        for i in 0..4 {
            let l = &self.patch.lfo[i];
            if l.retrig {
                v.lfo_phase[i] = l.phase;
            } else {
                let cycles = if l.sync {
                    beat / l.div.max(1e-3)
                } else {
                    now * l.rate
                };
                v.lfo_phase[i] = (cycles + l.phase) % 1.0;
            }
            v.lfo_hold[i] = self.rng.next_f64() * 2.0 - 1.0;
            v.lfo_prev[i] = v.lfo_hold[i];
        }
        for o in 0..4 {
            let op = &self.patch.osc[o];
            for u in 0..MAX_UNISON {
                let r = if op.rand_phase > 0.0 {
                    self.rng.next_f64() * op.rand_phase
                } else {
                    0.0
                };
                v.phases[o][u] = (op.phase + r) % 1.0;
            }
        }
        v.filters[0].reset();
        v.filters[1].reset();
    }

    pub fn note_on(&mut self, key: f64, vel: f64, fine: f64, pan: f64) {
        self.held_keys.retain(|k| *k != key);
        self.held_keys.push(key);
        let voice = self.patch.voice;
        if voice.mono {
            let found = self
                .voices
                .iter()
                .position(|x| x.active && !x.killing && !x.released);
            match found {
                Some(i) if voice.legato => {
                    let from = self.voices[i].pitch;
                    self.start_voice(i, key, vel, fine, pan, Some(from));
                }
                Some(i) => {
                    let from = if voice.glide > 0.0 {
                        Some(self.voices[i].pitch)
                    } else {
                        None
                    };
                    self.start_voice(i, key, vel, fine, pan, from);
                    // retrigger the envelopes even if mono and not legato
                    for e in &mut self.voices[i].env {
                        e.trigger(false);
                    }
                }
                None => {
                    for x in &mut self.voices {
                        if x.active {
                            x.killing = true;
                        }
                    }
                    let i = self.voices.iter().position(|x| !x.active).unwrap_or(0);
                    self.start_voice(i, key, vel, fine, pan, None);
                }
            }
        } else {
            let i = self.alloc_voice();
            self.start_voice(i, key, vel, fine, pan, None);
        }
        self.last_key = key;
    }

    pub fn note_off(&mut self, key: f64) {
        self.note_off_scaled(key, 1.0);
    }

    /// Let go of a key with its release `scale` times as long as the patch's.
    pub fn note_off_scaled(&mut self, key: f64, scale: f64) {
        self.held_keys.retain(|k| *k != key);
        let voice = self.patch.voice;
        if voice.mono && !self.held_keys.is_empty() {
            if let Some(v) = self
                .voices
                .iter_mut()
                .find(|x| x.active && !x.released && x.key == key)
            {
                let back = self.held_keys[self.held_keys.len() - 1];
                v.target_pitch = back;
                v.key = back;
                if voice.glide <= 0.0 {
                    v.pitch = back;
                }
            }
            return;
        }
        for v in &mut self.voices {
            if v.active && !v.released && v.key == key {
                v.released = true;
                for e in &mut v.env {
                    e.release_scaled(scale);
                }
            }
        }
    }

    /// Forget the notes that have not happened yet and release what sounds.
    pub fn stop(&mut self) {
        self.events.clear();
        self.all_off();
    }

    pub fn all_off(&mut self) {
        self.held_keys.clear();
        for v in &mut self.voices {
            if v.active {
                v.released = true;
                for e in &mut v.env {
                    e.release();
                }
            }
        }
    }

    // --- control rate ----------------------------------------------------------
    fn control(&mut self, idx: usize, n: usize) {
        let sr = self.sr;
        let nf = n as f64;
        let p = &self.patch;
        let v = &mut self.voices[idx];
        // envelopes 2 to 4
        let e2 = v.env[1].step(&p.env[1], nf, sr);
        let e3 = v.env[2].step(&p.env[2], nf, sr);
        let e4 = v.env[3].step(&p.env[3], nf, sr);
        let mut s = self.src_vals;
        s[0] = e2;
        s[1] = e3;
        s[2] = e4;
        v.lfo_time += nf / sr;
        for i in 0..4 {
            let l = &p.lfo[i];
            let rate = if l.sync {
                self.bpm / 60.0 / l.div.max(1e-3)
            } else {
                l.rate
            };
            let mut ph = v.lfo_phase[i] + (rate * nf) / sr;
            let wrapped = ph >= 1.0;
            ph -= ph.floor();
            v.lfo_phase[i] = ph;
            let mut val = lfo_shape(l.shape, ph, v, i, wrapped, &mut self.rng);
            if l.delay > 0.0 && v.lfo_time < l.delay {
                val *= v.lfo_time / l.delay;
            }
            s[3 + i] = val;
        }
        s[7] = v.vel;
        s[8] = (v.key - 60.0) / 60.0;
        s[9] = self.mw;
        s[10] = self.at;
        s[11] = self.pb;
        s[12] = v.rand;
        for i in 0..8 {
            s[13 + i] = p.macros.get(i).copied().unwrap_or(0.0);
        }
        self.src_vals = s;

        v.mods.fill(0.0);
        for i in 0..self.mod_src.len() {
            let d = self.mod_dst[i];
            v.mods[d] += s[self.mod_src[i]] * self.mod_amt[i] * self.dest_range[d];
        }
        let m = v.mods;

        // glide
        if p.voice.glide > 0.0 {
            let c = 1.0 - (-nf / (p.voice.glide * sr * 0.4)).exp();
            v.pitch += (v.target_pitch - v.pitch) * c;
        } else {
            v.pitch = v.target_pitch;
        }

        let base_pitch =
            v.pitch + p.tune + v.fine / 100.0 + self.pb * p.voice.pb_range + m[D_PITCH];

        for o in 0..4 {
            let op = &p.osc[o];
            let rt = &mut v.rt[o];
            if !op.on {
                rt.level = 0.0;
                continue;
            }
            let base = o * 6 + 3; // osc dest base (pitch, pos, level, pan, detune, warp)
            let pitch = base_pitch + op.oct * 12.0 + op.semi + op.fine / 100.0 + m[base];
            let pos = (op.pos + m[base + 1]).clamp(0.0, 1.0);
            rt.level = (op.level + m[base + 2]).max(0.0);
            let pan = (op.pan + m[base + 3] + v.note_pan).clamp(-1.0, 1.0);
            let detune = (op.detune + m[base + 4]).clamp(0.0, 1.0);
            rt.warp_amt = (op.warp_amt + m[base + 5]).clamp(0.0, 1.0);
            let table = &self.tables[o];
            let voices = (js_round(op.voices)).clamp(1.0, MAX_UNISON as f64) as usize;
            rt.voices = voices;
            let freq = 440.0 * 2f64.powf((pitch - 69.0) / 12.0);
            let spread_cents = detune * detune * 100.0;
            let mut max_f = freq;
            let mut gsum = 0.0;
            for u in 0..voices {
                let d = if voices == 1 {
                    0.0
                } else {
                    (u as f64 / (voices - 1) as f64) * 2.0 - 1.0
                };
                let ratio = 2f64.powf((d * spread_cents) / 1200.0);
                let f = freq * ratio;
                if f > max_f {
                    max_f = f;
                }
                rt.inc[u] = f / sr;
                let is_center = voices == 1 || (voices % 2 == 1 && u == (voices - 1) / 2);
                let g = if voices == 1 {
                    1.0
                } else if is_center {
                    1.0 - op.blend * 0.5
                } else {
                    0.25 + op.blend * 0.75
                };
                let up = (pan + d * op.spread).clamp(-1.0, 1.0);
                let a = (up + 1.0) * 0.25 * PI;
                rt.pan_l[u] = (a.cos() * g) as f32;
                rt.pan_r[u] = (a.sin() * g) as f32;
                gsum += g;
            }
            rt.gain = if voices == 1 {
                1.0
            } else {
                1.6 / (gsum * voices as f64).sqrt()
            };
            // mip level
            let warp_mul = if op.warp == WarpMode::Sync {
                1.0 + rt.warp_amt * 7.0
            } else {
                1.0
            };
            let limit = sr / 2.0 / (max_f * warp_mul).max(1.0);
            let mut level = 0;
            while level < MIP_LEVELS - 1 && max_harmonics(level) as f64 > limit {
                level += 1;
            }
            let fpos = pos * (table.frames - 1) as f64;
            let fi = (table.frames - 1).min(fpos.floor() as usize);
            rt.mip = level;
            rt.f0 = fi;
            rt.f1 = (table.frames - 1).min(fi + 1);
            rt.ft = fpos - fi as f64;
            rt.size = table.mips[level][fi].len();
            rt.mask = rt.size - 1;
        }

        // filters
        for f in 0..2 {
            let fp = &p.filters[f];
            if !fp.on {
                continue;
            }
            let fb = 3 + 24 + 2 + f * 3; // f{i}.cutoff index
            let oct = fp.env_amt * e2
                + fp.vel_amt * v.vel
                + (fp.keytrack * (v.key - 60.0)) / 12.0
                + m[fb];
            let cutoff = fp.cutoff * 2f64.powf(oct);
            v.filters[f].setup(
                fp.kind,
                cutoff,
                fp.res + m[fb + 1],
                (fp.drive + m[fb + 2]).max(0.0),
                sr,
            );
        }
    }

    // --- audio -------------------------------------------------------------------
    fn render_voice(
        &mut self,
        idx: usize,
        out_l: &mut [f32],
        out_r: &mut [f32],
        start: usize,
        n: usize,
    ) {
        let sr = self.sr;
        let p = &self.patch;
        let tables = &self.tables;
        let rng = &mut self.rng;
        let v = &mut self.voices[idx];
        let m = v.mods;
        let sub_level = if p.sub.on {
            (p.sub.level + m[D_SUB_LEVEL]).max(0.0)
        } else {
            0.0
        };
        let noise_level = if p.noise.on {
            (p.noise.level + m[D_NOISE_LEVEL]).max(0.0)
        } else {
            0.0
        };
        let f0on = p.filters[0].on;
        let f1on = p.filters[1].on;
        let parallel = p.filter_routing == Routing::Parallel;
        let vel_gain = 1.0 - p.voice.vel_sens * (1.0 - v.vel);
        let amp_mod = (1.0 + m[D_AMP]).max(0.0);
        let pan_mod = m[D_PAN].clamp(-1.0, 1.0);
        let pa = (pan_mod + 1.0) * 0.25 * PI;
        let m_pan_l = pa.cos() * SQRT_2;
        let m_pan_r = pa.sin() * SQRT_2;
        let gain = p.level * vel_gain * amp_mod * 0.5;
        let sub_freq = 440.0
            * 2f64.powf(
                (v.pitch + p.tune + p.sub.oct * 12.0 + self.pb * p.voice.pb_range - 69.0) / 12.0,
            );
        let mut sub_lvl = 0;
        if sub_level > 0.0 {
            while sub_lvl < MIP_LEVELS - 1 && max_harmonics(sub_lvl) as f64 > sr / 2.0 / sub_freq {
                sub_lvl += 1;
            }
        }
        let sub_data: Option<&[f32]> = if sub_level > 0.0 {
            Some(&self.sub_table.mips[sub_lvl][0])
        } else {
            None
        };
        let sub_inc = sub_freq / sr;

        for i in start..start + n {
            // per-sample amp envelope for click-free articulation
            let mut env = v.env[0].step(&p.env[0], 1.0, sr);
            if v.killing {
                v.kill_gain -= 1.0 / (0.004 * sr);
                if v.kill_gain <= 0.0 {
                    v.active = false;
                    v.kill_gain = 0.0;
                    return;
                }
                env *= v.kill_gain;
            }
            let (mut a_l, mut a_r, mut b_l, mut b_r, mut d_l, mut d_r) =
                (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for o in 0..4 {
                let rt = &v.rt[o];
                if rt.level <= 0.0 {
                    v.osc_out[o] = 0.0;
                    continue;
                }
                let op = &p.osc[o];
                let w = rt.warp_amt;
                let warp = op.warp;
                let f0 = &tables[o].mips[rt.mip][rt.f0];
                let f1 = &tables[o].mips[rt.mip][rt.f1];
                let (ft, size, mask) = (rt.ft, rt.size, rt.mask);
                let fm_src = v.osc_out[(o + 1) & 3];
                let (mut sl, mut sr_, mut mono) = (0.0f64, 0.0f64, 0.0f64);
                let read = |idx: f64| -> f64 {
                    let i0f = idx as i32;
                    let fr = idx - f64::from(i0f);
                    let i0 = (i0f as u32 as usize) & mask;
                    let i1 = (i0 + 1) & mask;
                    let (a0, a1) = (f64::from(f0[i0]), f64::from(f0[i1]));
                    let (b0, b1) = (f64::from(f1[i0]), f64::from(f1[i1]));
                    let a = a0 + (a1 - a0) * fr;
                    let b = b0 + (b1 - b0) * fr;
                    a + (b - a) * ft
                };
                for u in 0..rt.voices {
                    let mut phase = v.phases[o][u] + rt.inc[u];
                    if phase >= 1.0 {
                        phase -= phase.floor();
                    }
                    v.phases[o][u] = phase;
                    let mut rp = phase;
                    match warp {
                        WarpMode::Sync => rp = (phase * (1.0 + w * 7.0)) % 1.0,
                        WarpMode::BendUp => rp = phase.powf(1.0 + w * 3.0),
                        WarpMode::BendDown => rp = 1.0 - (1.0 - phase).powf(1.0 + w * 3.0),
                        WarpMode::Pwm => {
                            let pw = 0.5 * (1.0 - w * 0.96);
                            rp = if phase < pw {
                                (0.5 * phase) / pw
                            } else {
                                0.5 + (0.5 * (phase - pw)) / (1.0 - pw)
                            };
                        }
                        WarpMode::Asym => {
                            rp = phase + w * 0.15 * (TAU * phase).sin();
                            rp -= rp.floor();
                        }
                        WarpMode::Fm => {
                            rp = phase + w * 1.5 * fm_src;
                            rp -= rp.floor();
                        }
                        _ => {}
                    }
                    let mut s = read(rp * size as f64);
                    match warp {
                        WarpMode::Mirror => {
                            let mp = if phase < 0.5 { phase } else { 1.0 - phase };
                            s += (read(mp * size as f64) - s) * w;
                        }
                        WarpMode::Ring => s *= 1.0 - w + w * fm_src,
                        WarpMode::Quantize if w > 0.0 => {
                            let steps = 2f64.powf(1.0 + (1.0 - w) * 7.0);
                            s = js_round(s * steps) / steps;
                        }
                        _ => {}
                    }
                    sl += s * f64::from(rt.pan_l[u]);
                    sr_ += s * f64::from(rt.pan_r[u]);
                    mono += s;
                }
                v.osc_out[o] = mono / rt.voices as f64;
                let g = rt.level * rt.gain;
                sl *= g;
                sr_ *= g;
                match op.route {
                    Route::A => {
                        a_l += sl;
                        a_r += sr_;
                    }
                    Route::B => {
                        b_l += sl;
                        b_r += sr_;
                    }
                    Route::Ab => {
                        a_l += sl * 0.5;
                        a_r += sr_ * 0.5;
                        b_l += sl * 0.5;
                        b_r += sr_ * 0.5;
                    }
                    Route::Direct => {
                        d_l += sl;
                        d_r += sr_;
                    }
                }
            }
            if let Some(data) = sub_data {
                v.sub_phase += sub_inc;
                if v.sub_phase >= 1.0 {
                    v.sub_phase -= 1.0;
                }
                let idx = v.sub_phase * data.len() as f64;
                let i0 = idx as usize;
                let s = f64::from(data[i0 % data.len()]) * sub_level * 0.8;
                match p.sub.route {
                    Route::A => {
                        a_l += s;
                        a_r += s;
                    }
                    Route::B => {
                        b_l += s;
                        b_r += s;
                    }
                    _ => {
                        d_l += s;
                        d_r += s;
                    }
                }
            }
            if noise_level > 0.0 {
                let mut wv = rng.next_f64() * 2.0 - 1.0;
                if p.noise.color == NoiseColor::Pink {
                    let k = &mut v.pink;
                    k[0] = 0.99886 * k[0] + wv * 0.0555179;
                    k[1] = 0.99332 * k[1] + wv * 0.0750759;
                    k[2] = 0.969 * k[2] + wv * 0.153852;
                    k[3] = 0.8665 * k[3] + wv * 0.3104856;
                    k[4] = 0.55 * k[4] + wv * 0.5329522;
                    k[5] = -0.7616 * k[5] - wv * 0.016898;
                    wv = (k[0] + k[1] + k[2] + k[3] + k[4] + k[5] + k[6] + wv * 0.5362) * 0.11;
                    k[6] = wv * 0.115926;
                }
                let s = wv * noise_level * 0.5;
                match p.noise.route {
                    Route::B => {
                        b_l += s;
                        b_r += s;
                    }
                    Route::Direct => {
                        d_l += s;
                        d_r += s;
                    }
                    _ => {
                        a_l += s;
                        a_r += s;
                    }
                }
            }
            // filters
            let (mut l, mut r);
            let [fa, fb] = &mut v.filters;
            if parallel {
                if f0on {
                    a_l = fa.process(0, a_l);
                    a_r = fa.process(1, a_r);
                }
                if f1on {
                    b_l = fb.process(0, b_l);
                    b_r = fb.process(1, b_r);
                }
                l = a_l + b_l;
                r = a_r + b_r;
            } else {
                l = a_l + b_l;
                r = a_r + b_r;
                if f0on {
                    l = fa.process(0, l);
                    r = fa.process(1, r);
                }
                if f1on {
                    l = fb.process(0, l);
                    r = fb.process(1, r);
                }
            }
            l += d_l;
            r += d_r;
            let g = env * gain;
            out_l[i] = (f64::from(out_l[i]) + l * g * m_pan_l) as f32;
            out_r[i] = (f64::from(out_r[i]) + r * g * m_pan_r) as f32;
        }
        if v.env[0].is_idle() {
            v.active = false;
        }
    }

    /// Render one block, adding to `l` and `r` (which must have the same length),
    /// starting at engine time `t0` seconds.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], t0: f64) {
        let frames = l.len();
        let sr = self.sr;
        self.now = t0;
        let mut pos = 0usize;
        while pos < frames {
            while let Some(e) = self.events.first().copied() {
                if js_round((e.time() - t0) * sr) > pos as f64 {
                    break;
                }
                self.events.remove(0);
                match e {
                    Event::On {
                        key,
                        vel,
                        fine,
                        pan,
                        ..
                    } => self.note_on(key, vel, fine, pan),
                    Event::Off { key, .. } => self.note_off(key),
                    Event::OffScaled { key, scale, .. } => self.note_off_scaled(key, scale),
                    Event::AllOff { .. } => self.all_off(),
                    Event::Cc { cc, value, .. } => match cc {
                        Cc::Mw => self.mw = value,
                        Cc::At => self.at = value,
                        Cc::Pb => self.pb = value,
                    },
                }
            }
            self.apply_ramps(t0 + pos as f64 / sr);
            let mut n = CONTROL.min(frames - pos);
            if let Some(e) = self.events.first() {
                let ef = js_round((e.time() - t0) * sr);
                if ef > pos as f64 && ef - (pos as f64) < n as f64 {
                    n = (ef - pos as f64) as usize;
                }
            }
            for i in 0..self.voices.len() {
                if self.voices[i].active {
                    self.control(i, n);
                    self.render_voice(i, l, r, pos, n);
                }
            }
            pos += n;
        }
        // soft safety clip
        for x in l.iter_mut().chain(r.iter_mut()) {
            if *x > 1.5 || *x < -1.5 {
                *x = f64::from(*x).tanh() as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(e: &mut SynthEngine, seconds: f64) -> (Vec<f32>, Vec<f32>) {
        let n = (seconds * e.sample_rate()) as usize;
        let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
        let mut i = 0;
        while i < n {
            let end = (i + 128).min(n);
            e.process(&mut l[i..end], &mut r[i..end], i as f64 / e.sample_rate());
            i = end;
        }
        (l, r)
    }

    fn note(e: &mut SynthEngine, key: f64, on: f64, off: f64) {
        e.queue(&[
            Event::On {
                time: on,
                key,
                vel: 1.0,
                fine: 0.0,
                pan: 0.0,
            },
            Event::Off { time: off, key },
        ]);
    }

    fn peak(b: &[f32]) -> f32 {
        b.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    /// Frequency from upward zero crossings.
    fn frequency(x: &[f32], sr: f64) -> f64 {
        let mut crossings = Vec::new();
        for i in 1..x.len() {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                let t = f64::from(-x[i - 1]) / f64::from(x[i] - x[i - 1]);
                crossings.push(i as f64 - 1.0 + t);
            }
        }
        let n = crossings.len() - 1;
        sr * n as f64 / (crossings[n] - crossings[0])
    }

    fn quiet_osc() -> Value {
        json!({ "osc": [{ "randPhase": 0 }, { "randPhase": 0 }, { "randPhase": 0 }, { "randPhase": 0 }] })
    }

    #[test]
    fn a_sine_plays_the_note_frequency() {
        let mut e = synth_engine(48000.0, 1);
        e.set_patch(&json!({
            "osc": [{ "table": "sine", "randPhase": 0 }],
            "filters": [{ "on": false }]
        }))
        .unwrap();
        note(&mut e, 69.0, 0.0, 1.0);
        let (l, _) = render(&mut e, 0.5);
        let f = frequency(&l[8000..], 48000.0);
        assert!((f - 440.0).abs() < 0.5, "{f} Hz");
    }

    #[test]
    fn the_tune_and_octave_controls_shift_the_pitch() {
        let mut e = synth_engine(48000.0, 1);
        e.set_patch(&json!({
            "osc": [{ "table": "sine", "randPhase": 0, "oct": 1 }],
            "filters": [{ "on": false }],
            "tune": 12
        }))
        .unwrap();
        note(&mut e, 57.0, 0.0, 1.0);
        let (l, _) = render(&mut e, 0.5);
        let f = frequency(&l[8000..], 48000.0);
        assert!((f - 880.0).abs() < 1.0, "{f} Hz");
    }

    #[test]
    fn renders_repeat_exactly_and_the_seed_matters_for_random_phase() {
        let go = |seed| {
            let mut e = synth_engine(48000.0, seed);
            e.set_patch(&json!({ "osc": [{ "randPhase": 1, "voices": 4 }] }))
                .unwrap();
            note(&mut e, 60.0, 0.0, 0.2);
            render(&mut e, 0.4).0
        };
        assert_eq!(go(5), go(5));
        assert_ne!(go(5), go(6));
    }

    #[test]
    fn every_warp_mode_and_filter_type_is_stable() {
        let warps = [
            "none", "sync", "bend+", "bend-", "pwm", "mirror", "fm", "ring", "quantize", "asym",
        ];
        let filters = [
            "lp12", "lp24", "hp12", "hp24", "bp12", "bp24", "notch", "peak", "ladder", "formant",
            "comb",
        ];
        for (i, w) in warps.iter().enumerate() {
            let f = filters[i % filters.len()];
            let mut e = synth_engine(48000.0, 1);
            e.set_patch(&json!({
                "osc": [{ "randPhase": 0, "warp": w, "warpAmt": 0.7, "voices": 3 }],
                "filters": [{ "type": f, "cutoff": 2000, "res": 0.9, "drive": 0.5 }]
            }))
            .unwrap();
            note(&mut e, 48.0, 0.0, 0.3);
            let (l, r) = render(&mut e, 0.5);
            for x in l.iter().chain(&r) {
                assert!(x.is_finite(), "{w}/{f}: not finite");
            }
            assert!(peak(&l) > 0.001, "{w}/{f}: silent");
            assert!(peak(&l) <= 1.6 && peak(&r) <= 1.6, "{w}/{f}: too loud");
        }
        for f in filters {
            let mut e = synth_engine(48000.0, 1);
            e.set_patch(&json!({
                "osc": [{ "randPhase": 0 }],
                "filters": [{ "type": f, "cutoff": 800, "res": 1.0, "drive": 1.0 }, { "on": true, "type": f, "cutoff": 8000 }],
                "filterRouting": "parallel"
            }))
            .unwrap();
            note(&mut e, 36.0, 0.0, 0.3);
            let (l, _) = render(&mut e, 0.5);
            assert!(l.iter().all(|x| x.is_finite()), "{f} blew up");
        }
    }

    #[test]
    fn a_prepared_patch_swaps_in_while_notes_play() {
        let mut e = synth_engine(48000.0, 1);
        e.set_patch(&quiet_osc()).unwrap();
        note(&mut e, 60.0, 0.0, 1.0);
        let (a, _) = render(&mut e, 0.1);
        assert!(peak(&a) > 0.01);
        let mut lib = TableLibrary::new();
        let mut p = prepare_synth_patch(
            &mut lib,
            &json!({ "osc": [{ "table": "square", "randPhase": 0 }], "level": 0.2 }),
        )
        .unwrap();
        e.apply_patch(&mut p);
        // the previous patch stays in `p` and can be switched back to
        e.apply_patch(&mut p);
        let (b, _) = render(&mut e, 0.1);
        assert!(peak(&b) > 0.01);
    }

    #[test]
    fn a_bad_patch_is_refused_and_changes_nothing() {
        let mut e = synth_engine(48000.0, 1);
        assert!(e.set_patch(&json!({ "level": "loud" })).is_err());
        assert!(
            e.set_patch(&json!({ "osc": [] })).is_ok(),
            "an empty list keeps the defaults"
        );
        assert!(e
            .set_patch(&json!({ "filters": [{ "type": "nope" }] }))
            .is_err());
        note(&mut e, 60.0, 0.0, 0.2);
        assert!(peak(&render(&mut e, 0.3).0) > 0.01);
    }

    #[test]
    fn a_user_table_is_used() {
        let mut e = synth_engine(48000.0, 1);
        let cycle: Vec<f32> = (0..2048)
            .map(|i| (i as f32 / 2048.0 * std::f32::consts::TAU * 3.0).sin())
            .collect();
        e.register_table("three", &[cycle]);
        e.set_patch(&json!({
            "osc": [{ "table": "three", "randPhase": 0 }],
            "filters": [{ "on": false }]
        }))
        .unwrap();
        note(&mut e, 57.0, 0.0, 1.0); // 220 Hz, three cycles per table cycle
        let (l, _) = render(&mut e, 0.5);
        let f = frequency(&l[8000..], 48000.0);
        assert!((f - 660.0).abs() < 1.0, "{f} Hz");
    }

    #[test]
    fn many_voices_stay_within_the_limit() {
        let mut e = synth_engine(48000.0, 1);
        e.set_patch(&json!({ "voice": { "poly": 3 } })).unwrap();
        for (i, k) in [60.0, 64.0, 67.0, 71.0, 74.0].iter().enumerate() {
            note(&mut e, *k, 0.01 * i as f64, 5.0);
        }
        render(&mut e, 0.3);
        // stolen voices fade out within a few milliseconds, leaving 3
        assert_eq!(e.active_voices(), 3);
    }
}
