// SPDX-License-Identifier: GPL-3.0-or-later
//! The effect catalogue and the units that play them.
//!
//! The catalogue (names, categories, parameters with ranges and defaults, and
//! factory presets) is data exported from the web version, so the app can draw
//! any effect's controls from it. An [`EffectUnit`] is one running effect;
//! [`parse_params`] turns an effect state (the JSON the web version saves) into
//! typed parameters off the audio thread, and [`EffectUnit::apply`] swaps them in
//! without allocating.

use super::delay::{Echo, EchoParams};
use super::dynamics::{
    Compressor, CompressorParams, Crusher, CrusherParams, GateParams, Limiter, LimiterParams,
    NoiseGate, TransientParams, TransientShaper,
};
use super::modulation::{
    FreqShiftParams, FrequencyShifter, PitchShiftParams, PitchShifter, Tape, TapeParams,
};
use super::native::{
    AutoFilter, AutoFilterParams, Chorus, ChorusParams, ConvReverb, DistParams, Distortion, Eq,
    EqParams, Maximizer, MaximizerParams, Phaser, PhaserParams, ReverbImpulse, ReverbParams,
    StereoParams, StereoTool,
};
use super::reverb::{Hall, HallParams};
use super::stepfx::{Position, ShaperParams, ShaperPoint, StepFx, StepFxParams, VolumeShaper};
use super::{update_params, DelayLine, StereoEffect};
use crate::engine::PatchError;
use crate::loudness::{correlation, LoudnessMeter};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// The catalogue
// ---------------------------------------------------------------------------

/// How a parameter is drawn and mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    #[default]
    Knob,
    Select,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    Lin,
    Log,
    Pow,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ParamOption {
    pub value: Value,
    pub label: String,
}

/// One parameter of an effect. `id` is a dotted path into the state.
#[derive(Clone, Debug, Deserialize)]
pub struct ParamDef {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: ParamKind,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    pub default: Value,
    #[serde(default)]
    pub curve: Option<Curve>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub options: Vec<ParamOption>,
    #[serde(default)]
    pub bipolar: bool,
    #[serde(default)]
    pub decimals: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EffectPreset {
    pub name: String,
    pub category: String,
    pub state: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    /// Accepts a sidechain source.
    pub sidechain: bool,
    pub params: Vec<ParamDef>,
    #[serde(rename = "defaultState")]
    pub default_state: Value,
    pub presets: Vec<EffectPreset>,
}

#[derive(Deserialize)]
struct Catalogue {
    effects: Vec<Descriptor>,
    #[serde(rename = "shaperPresets")]
    shaper_presets: BTreeMap<String, Vec<ShaperPoint>>,
}

fn catalogue() -> &'static Catalogue {
    static CELL: OnceLock<Catalogue> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(include_str!("../../effects/effects.json"))
            .expect("a valid effect catalogue")
    })
}

/// Every effect, in the order of the web version.
pub fn descriptors() -> &'static [Descriptor] {
    &catalogue().effects
}

pub fn descriptor(id: &str) -> Option<&'static Descriptor> {
    descriptors().iter().find(|d| d.id == id)
}

/// Curves for the volume shaper (name to points).
pub fn shaper_presets() -> &'static BTreeMap<String, Vec<ShaperPoint>> {
    &catalogue().shaper_presets
}

impl ParamDef {
    /// Map a position 0 to 1 to a value.
    pub fn from_normalized(&self, n: f64) -> f64 {
        let min = self.min.unwrap_or(0.0);
        let max = self.max.unwrap_or(1.0);
        let n = n.clamp(0.0, 1.0);
        let mut v = match self.curve {
            Some(Curve::Log) if min > 0.0 => min * (max / min).powf(n),
            Some(Curve::Pow) => min + (max - min) * n * n,
            _ => min + (max - min) * n,
        };
        if let Some(step) = self.step {
            v = (v / step).round() * step;
        }
        v
    }

    /// Map a value to a position 0 to 1.
    pub fn to_normalized(&self, v: f64) -> f64 {
        let min = self.min.unwrap_or(0.0);
        let max = self.max.unwrap_or(1.0);
        if max <= min {
            return 0.0;
        }
        let n = match self.curve {
            Some(Curve::Log) if min > 0.0 => (v.max(min) / min).ln() / (max / min).ln(),
            Some(Curve::Pow) => ((v - min) / (max - min)).max(0.0).sqrt(),
            _ => (v - min) / (max - min),
        };
        n.clamp(0.0, 1.0)
    }
}

/// The volume shaper's points for a trance gate: `steps` on or off, each open
/// for the `gate` fraction of its length.
pub fn gate_steps_to_points(steps: &[f64], gate: f64) -> Vec<ShaperPoint> {
    let n = if steps.is_empty() { 1 } else { steps.len() };
    let mut pts = Vec::new();
    for i in 0..n {
        let x0 = i as f64 / n as f64;
        let x1 = (i as f64 + gate) / n as f64;
        let on = if steps.get(i).copied().unwrap_or(0.0) != 0.0 {
            1.0
        } else {
            0.0
        };
        let end = (i + 1) as f64 / n as f64;
        pts.push(ShaperPoint { x: x0, y: on });
        pts.push(ShaperPoint {
            x: x1.min(end - 1e-4),
            y: on,
        });
        pts.push(ShaperPoint {
            x: (x1 + 1e-4).min(end - 5e-5),
            y: 0.0,
        });
    }
    pts.push(ShaperPoint { x: 1.0, y: 0.0 });
    pts
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Typed parameters of one effect, ready to swap in.
pub enum Params {
    Eq(EqParams),
    Comp(CompressorParams),
    Limiter(LimiterParams),
    Maximizer(MaximizerParams),
    Reverb(ReverbParams, Option<ReverbImpulse>),
    Delay(EchoParams),
    Dist(DistParams),
    Crusher(CrusherParams),
    Chorus(ChorusParams),
    Phaser(PhaserParams),
    Filter(AutoFilterParams),
    Stereo(StereoParams),
    Shaper(ShaperParams),
    NoiseGate(GateParams),
    Transient(TransientParams),
    Tape(TapeParams),
    FreqShift(FreqShiftParams),
    Pitch(PitchShiftParams),
    Hall(HallParams),
    StepFx(StepFxParams),
    None,
}

fn parsed<P: serde::Serialize + serde::de::DeserializeOwned + Default>(
    state: &Value,
) -> Result<P, PatchError> {
    let mut p = P::default();
    update_params(&mut p, state)?;
    Ok(p)
}

/// The sidechain choice is a source name or null in a saved state; the effects
/// want a flag.
fn with_sidechain_flag(state: &Value) -> Value {
    let mut s = state.clone();
    if let Some(o) = s.as_object_mut() {
        let on = !o.get("sidechain").is_none_or(Value::is_null);
        o.insert("sidechain".into(), Value::Bool(on));
    }
    s
}

/// The complete state of an effect: its defaults with the keys of `state`
/// replacing theirs (a shallow merge, as in the web version, so a list in the
/// state is the whole list).
pub fn full_state(id: &str, state: &Value) -> Value {
    let defaults = descriptor(id).map_or(Value::Null, |d| d.default_state.clone());
    shallow_merge(&defaults, state)
}

fn shallow_merge(defaults: &Value, state: &Value) -> Value {
    let mut out = defaults.clone();
    if let (Some(o), Some(s)) = (out.as_object_mut(), state.as_object()) {
        for (k, v) in s {
            o.insert(k.clone(), v.clone());
        }
    }
    out
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// As [`parse_params`] for an effect that is already running: a reverb whose response
/// would not change gets none built.
pub fn parse_params_for(
    unit: &EffectUnit,
    id: &str,
    state: &Value,
    sr: f64,
) -> Result<Params, PatchError> {
    let params = parse_params_with(id, state, sr, false)?;
    Ok(match (unit, params) {
        (EffectUnit::Reverb(fx), Params::Reverb(p, _)) => {
            let ir = (ConvReverb::ir_key(&p) != ConvReverb::ir_key(&fx.p))
                .then(|| ReverbImpulse::new(sr, &p));
            Params::Reverb(p, ir)
        }
        (_, params) => params,
    })
}

/// Parse an effect state into typed parameters. `sr` is for the convolution
/// reverb, which builds its impulse response here, off the audio thread.
pub fn parse_params(id: &str, state: &Value, sr: f64) -> Result<Params, PatchError> {
    parse_params_with(id, state, sr, true)
}

fn parse_params_with(
    id: &str,
    state: &Value,
    sr: f64,
    build_impulse: bool,
) -> Result<Params, PatchError> {
    let defaults = descriptor(id)
        .map(|d| d.default_state.clone())
        .unwrap_or(Value::Null);
    // like the web version: the saved keys replace the defaults' (a shallow merge,
    // so a list in the state is the whole list)
    let state = shallow_merge(&defaults, state);
    Ok(match id {
        "las-eq" => Params::Eq(parsed(&state)?),
        "las-comp" | "las-ducker" => Params::Comp(parsed(&with_sidechain_flag(&state))?),
        "las-limiter" => Params::Limiter(parsed(&state)?),
        "las-maximizer" => Params::Maximizer(parsed(&state)?),
        "las-reverb" => {
            let p: ReverbParams = parsed(&state)?;
            let ir = build_impulse.then(|| ReverbImpulse::new(sr, &p));
            Params::Reverb(p, ir)
        }
        "las-delay" => Params::Delay(parsed(&state)?),
        "las-dist" => Params::Dist(parsed(&state)?),
        "las-crusher" => Params::Crusher(parsed(&state)?),
        "las-chorus" => Params::Chorus(parsed(&state)?),
        "las-phaser" => Params::Phaser(parsed(&state)?),
        "las-filter" => Params::Filter(parsed(&state)?),
        "las-stereo" => Params::Stereo(parsed(&state)?),
        "las-shaper" => Params::Shaper(parsed(&state)?),
        "las-gate" => {
            let steps = nums(&state["steps"]);
            let gate = state["gate"].as_f64().unwrap_or(0.6);
            let p = ShaperParams {
                points: gate_steps_to_points(&steps, gate),
                div: state["div"].as_f64().unwrap_or(0.25) * steps.len().max(1) as f64,
                depth: state["depth"].as_f64().unwrap_or(1.0),
                smooth: state["smooth"].as_f64().unwrap_or(3.0),
                offset: 0.0,
            };
            Params::Shaper(p)
        }
        "las-noisegate" => Params::NoiseGate(parsed(&with_sidechain_flag(&state))?),
        "las-transient" => Params::Transient(parsed(&state)?),
        "las-tape" => Params::Tape(parsed(&state)?),
        "las-freqshift" => Params::FreqShift(parsed(&state)?),
        "las-pitch" => Params::Pitch(parsed(&state)?),
        "las-hall" => Params::Hall(parsed(&state)?),
        "las-stepfx" => {
            let mut p: StepFxParams = parsed(&state)?;
            // the lanes always have 16 steps; missing ones take the lane's rest value
            let lane = |v: &Value, d: f64| -> Vec<f64> {
                (0..16)
                    .map(|i| v.get(i).and_then(Value::as_f64).unwrap_or(d))
                    .collect()
            };
            p.cutoff = lane(&state["cutoff"], 1.0);
            p.level = lane(&state["level"], 1.0);
            p.pan = lane(&state["pan"], 0.0);
            Params::StepFx(p)
        }
        "las-scope" | "las-meter" => Params::None,
        other => return Err(PatchError(format!("no effect called {other}"))),
    })
}

// ---------------------------------------------------------------------------
// Units
// ---------------------------------------------------------------------------

/// Passes the signal through while measuring its loudness and correlation.
pub struct Meter {
    pub loudness: LoudnessMeter,
    corr_l: Vec<f32>,
    corr_r: Vec<f32>,
    corr_pos: usize,
}

impl Meter {
    pub fn new(sr: f64) -> Self {
        Self {
            loudness: LoudnessMeter::new(sr, 2),
            corr_l: vec![0.0; 4096],
            corr_r: vec![0.0; 4096],
            corr_pos: 0,
        }
    }

    /// Stereo correlation over the last 4096 samples.
    pub fn correlation(&self) -> f64 {
        correlation(&self.corr_l, &self.corr_r)
    }
}

impl StereoEffect for Meter {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sc: Option<(&[f32], &[f32])>,
    ) {
        out_l.copy_from_slice(in_l);
        out_r.copy_from_slice(in_r);
        self.loudness.process(&[in_l, in_r], in_l.len());
        for i in 0..in_l.len() {
            self.corr_l[self.corr_pos] = in_l[i];
            self.corr_r[self.corr_pos] = in_r[i];
            self.corr_pos = (self.corr_pos + 1) % self.corr_l.len();
        }
    }

    fn set_params(&mut self, _over: &Value) -> Result<(), PatchError> {
        Ok(())
    }
}

/// A running effect.
#[allow(clippy::large_enum_variant)]
pub enum EffectUnit {
    Eq(Eq),
    Comp(Compressor),
    Limiter(Limiter),
    Maximizer(Maximizer),
    Reverb(ConvReverb),
    Delay(Echo),
    Dist(Distortion),
    Crusher(Crusher),
    Chorus(Chorus),
    Phaser(Phaser),
    Filter(AutoFilter),
    Stereo(StereoTool),
    Shaper(VolumeShaper),
    NoiseGate(NoiseGate),
    Transient(TransientShaper),
    Tape(Tape),
    FreqShift(FrequencyShifter),
    Pitch(PitchShifter),
    Hall(Hall),
    StepFx(StepFx),
    Meter(Meter),
    /// The scope draws from what passes; the audio is unchanged.
    Pass,
}

macro_rules! each {
    ($self:expr, $fx:ident => $body:expr, $none:expr) => {
        match $self {
            EffectUnit::Eq($fx) => $body,
            EffectUnit::Comp($fx) => $body,
            EffectUnit::Limiter($fx) => $body,
            EffectUnit::Maximizer($fx) => $body,
            EffectUnit::Reverb($fx) => $body,
            EffectUnit::Delay($fx) => $body,
            EffectUnit::Dist($fx) => $body,
            EffectUnit::Crusher($fx) => $body,
            EffectUnit::Chorus($fx) => $body,
            EffectUnit::Phaser($fx) => $body,
            EffectUnit::Filter($fx) => $body,
            EffectUnit::Stereo($fx) => $body,
            EffectUnit::Shaper($fx) => $body,
            EffectUnit::NoiseGate($fx) => $body,
            EffectUnit::Transient($fx) => $body,
            EffectUnit::Tape($fx) => $body,
            EffectUnit::FreqShift($fx) => $body,
            EffectUnit::Pitch($fx) => $body,
            EffectUnit::Hall($fx) => $body,
            EffectUnit::StepFx($fx) => $body,
            EffectUnit::Meter($fx) => $body,
            EffectUnit::Pass => $none,
        }
    };
}

impl EffectUnit {
    /// A new unit with the web version's default settings, or `None` for an
    /// unknown id. The trance gate and the volume shaper are both a shaper.
    pub fn new(id: &str, sr: f64) -> Option<Self> {
        Some(match id {
            "las-eq" => Self::Eq(Eq::new(sr)),
            "las-comp" | "las-ducker" => Self::Comp(Compressor::new(sr)),
            "las-limiter" => Self::Limiter(Limiter::new(sr)),
            "las-maximizer" => Self::Maximizer(Maximizer::new(sr)),
            "las-reverb" => Self::Reverb(ConvReverb::new(sr)),
            "las-delay" => Self::Delay(Echo::new(sr)),
            "las-dist" => Self::Dist(Distortion::new(sr)),
            "las-crusher" => Self::Crusher(Crusher::new(sr)),
            "las-chorus" => Self::Chorus(Chorus::new(sr)),
            "las-phaser" => Self::Phaser(Phaser::new(sr)),
            "las-filter" => Self::Filter(AutoFilter::new(sr)),
            "las-stereo" => Self::Stereo(StereoTool::new(sr)),
            "las-shaper" | "las-gate" => Self::Shaper(VolumeShaper::new(sr)),
            "las-noisegate" => Self::NoiseGate(NoiseGate::new(sr)),
            "las-transient" => Self::Transient(TransientShaper::new(sr)),
            "las-tape" => Self::Tape(Tape::new(sr)),
            "las-freqshift" => Self::FreqShift(FrequencyShifter::new(sr)),
            "las-pitch" => Self::Pitch(PitchShifter::new(sr)),
            "las-hall" => Self::Hall(Hall::new(sr)),
            "las-stepfx" => Self::StepFx(StepFx::new(sr)),
            "las-meter" => Self::Meter(Meter::new(sr)),
            "las-scope" => Self::Pass,
            _ => return None,
        })
    }

    pub fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
    ) {
        each!(self, fx => fx.process(in_l, in_r, out_l, out_r, sidechain), {
            out_l.copy_from_slice(in_l);
            out_r.copy_from_slice(in_r);
        })
    }

    /// Delay this effect adds at its current settings, seconds.
    pub fn latency(&self) -> f64 {
        each!(self, fx => fx.latency(), 0.0)
    }

    /// Tell the effects that follow the song where it is: the beat at the start of
    /// the next block and the tempo.
    pub fn set_position(&mut self, pos: Position) {
        match self {
            EffectUnit::Shaper(s) => s.position = pos,
            EffectUnit::StepFx(s) => s.position = pos,
            EffectUnit::Delay(d) => d.p.bpm = pos.bpm,
            _ => {}
        }
    }

    /// Swap in new parameters made by [`parse_params`]. Does not allocate. The old
    /// parameters come back so the caller can free them elsewhere.
    pub fn apply(&mut self, p: Params) -> Params {
        macro_rules! swap {
            ($fx:expr, $variant:ident, $p:expr) => {
                Params::$variant(std::mem::replace(&mut $fx.p, $p))
            };
        }
        match (self, p) {
            (EffectUnit::Eq(fx), Params::Eq(p)) => {
                let old = swap!(fx, Eq, p);
                fx.retarget();
                old
            }
            (EffectUnit::Comp(fx), Params::Comp(p)) => swap!(fx, Comp, p),
            (EffectUnit::Limiter(fx), Params::Limiter(p)) => swap!(fx, Limiter, p),
            (EffectUnit::Maximizer(fx), Params::Maximizer(p)) => swap!(fx, Maximizer, p),
            (EffectUnit::Reverb(fx), Params::Reverb(p, ir)) => {
                let old = std::mem::replace(&mut fx.p, p);
                // the response is only replaced when its settings changed
                if let Some(ir) =
                    ir.filter(|_| ConvReverb::ir_key(&fx.p) != ConvReverb::ir_key(&old))
                {
                    fx.set_impulse(ir);
                }
                fx.retarget();
                Params::Reverb(old, None)
            }
            (EffectUnit::Delay(fx), Params::Delay(p)) => {
                // the tempo belongs to the song, not to the saved state
                let bpm = fx.p.bpm;
                let mut old = std::mem::replace(&mut fx.p, p);
                fx.p.bpm = bpm;
                old.bpm = bpm;
                Params::Delay(old)
            }
            (EffectUnit::Dist(fx), Params::Dist(p)) => {
                let old = swap!(fx, Dist, p);
                fx.retarget();
                old
            }
            (EffectUnit::Crusher(fx), Params::Crusher(p)) => swap!(fx, Crusher, p),
            (EffectUnit::Chorus(fx), Params::Chorus(p)) => {
                let old = swap!(fx, Chorus, p);
                fx.retarget();
                old
            }
            (EffectUnit::Phaser(fx), Params::Phaser(p)) => {
                let old = swap!(fx, Phaser, p);
                fx.retarget();
                old
            }
            (EffectUnit::Filter(fx), Params::Filter(p)) => {
                let old = swap!(fx, Filter, p);
                fx.retarget();
                old
            }
            (EffectUnit::Stereo(fx), Params::Stereo(p)) => {
                let old = swap!(fx, Stereo, p);
                fx.retarget();
                old
            }
            (EffectUnit::Shaper(fx), Params::Shaper(p)) => swap!(fx, Shaper, p),
            (EffectUnit::NoiseGate(fx), Params::NoiseGate(p)) => swap!(fx, NoiseGate, p),
            (EffectUnit::Transient(fx), Params::Transient(p)) => swap!(fx, Transient, p),
            (EffectUnit::Tape(fx), Params::Tape(p)) => swap!(fx, Tape, p),
            (EffectUnit::FreqShift(fx), Params::FreqShift(p)) => swap!(fx, FreqShift, p),
            (EffectUnit::Pitch(fx), Params::Pitch(p)) => swap!(fx, Pitch, p),
            (EffectUnit::Hall(fx), Params::Hall(p)) => swap!(fx, Hall, p),
            (EffectUnit::StepFx(fx), Params::StepFx(p)) => swap!(fx, StepFx, p),
            // parameters for another kind of effect, or none: nothing to change
            (_, p) => p,
        }
    }

    /// Tell the effects that glide to new settings where the next block starts, in frames.
    pub fn set_time(&mut self, frame: usize) {
        match self {
            EffectUnit::Eq(f) => f.frame = frame,
            EffectUnit::Reverb(f) => f.frame = frame,
            EffectUnit::Filter(f) => f.frame = frame,
            EffectUnit::Dist(f) => f.frame = frame,
            EffectUnit::Chorus(f) => f.frame = frame,
            EffectUnit::Phaser(f) => f.frame = frame,
            EffectUnit::Stereo(f) => f.frame = frame,
            _ => {}
        }
    }

    /// Timed automation the effect does itself, with Web Audio parameters: whether
    /// `path` is one of them.
    pub fn automate(&mut self, path: &str, points: &[(f64, f64)], first: bool) -> bool {
        match self {
            EffectUnit::Eq(eq) => eq.automate(path, points, first),
            _ => false,
        }
    }

    /// The most gain reduction since the last call (compressor, limiter, gate), dB.
    pub fn take_gain_reduction(&mut self) -> Option<f64> {
        match self {
            EffectUnit::Comp(c) => Some(c.take_gain_reduction()),
            EffectUnit::Limiter(l) => Some(l.take_gain_reduction()),
            EffectUnit::NoiseGate(g) => Some(-std::mem::take(&mut g.min_gain_db)),
            _ => None,
        }
    }

    /// The loudness meter, when this is one.
    pub fn meter(&self) -> Option<&Meter> {
        match self {
            EffectUnit::Meter(m) => Some(m),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// A mixer insert slot: an effect with on/off, dry/wet and latency matching
// ---------------------------------------------------------------------------

/// An effect in an insert chain. Like the web version's slots, the dry path is
/// delayed by the effect's latency, so the slot delays the signal by the same
/// amount whether it is on, off or mixed.
pub struct FxSlot {
    pub unit: EffectUnit,
    pub enabled: bool,
    /// 0 to 1.
    pub mix: f64,
    sr: f64,
    dry: [DelayLine; 2],
    wet_l: Vec<f32>,
    wet_r: Vec<f32>,
}

impl FxSlot {
    pub fn new(unit: EffectUnit, sr: f64) -> Self {
        let size = (sr * 1.0) as usize + 8;
        Self {
            unit,
            enabled: true,
            mix: 1.0,
            sr,
            dry: [DelayLine::new(size), DelayLine::new(size)],
            wet_l: Vec::new(),
            wet_r: Vec::new(),
        }
    }

    /// Delay this slot adds, seconds.
    pub fn latency(&self) -> f64 {
        self.unit.latency().clamp(0.0, 1.0)
    }

    /// Process a block in place.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], sidechain: Option<(&[f32], &[f32])>) {
        let m = if self.enabled {
            self.mix.clamp(0.0, 1.0)
        } else {
            0.0
        } as f32;
        self.run(l, r, sidechain, |_| (m, 1.0 - m));
    }

    /// Process a block in place with the wet and dry gains for each frame.
    pub fn process_gains(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
        wet: &[f32],
        dry: &[f32],
    ) {
        self.run(l, r, sidechain, |i| (wet[i], dry[i]));
    }

    fn run(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
        gains: impl Fn(usize) -> (f32, f32),
    ) {
        let n = l.len();
        self.wet_l.resize(n, 0.0);
        self.wet_r.resize(n, 0.0);
        self.unit
            .process(l, r, &mut self.wet_l, &mut self.wet_r, sidechain);
        let delay = (self.latency() * self.sr).round();
        for i in 0..n {
            self.dry[0].write(f64::from(l[i]));
            self.dry[1].write(f64::from(r[i]));
            let (dl, dr) = if delay >= 1.0 {
                (
                    self.dry[0].read(delay) as f32,
                    self.dry[1].read(delay) as f32,
                )
            } else {
                (l[i], r[i])
            };
            let (wet, dry) = gains(i);
            l[i] = dl * dry + self.wet_l[i] * wet;
            r[i] = dr * dry + self.wet_r[i] * wet;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_loads_and_every_effect_has_a_unit() {
        assert_eq!(descriptors().len(), 24);
        for d in descriptors() {
            assert!(
                EffectUnit::new(&d.id, 48000.0).is_some(),
                "no unit for {}",
                d.id
            );
            assert!(
                parse_params(&d.id, &d.default_state, 48000.0).is_ok(),
                "default state of {} does not parse",
                d.id
            );
            for p in &d.presets {
                assert!(
                    parse_params(&d.id, &p.state, 48000.0).is_ok(),
                    "preset {} of {} does not parse",
                    p.name,
                    d.id
                );
            }
            for p in &d.params {
                if p.kind == ParamKind::Knob {
                    assert!(p.min.is_some() && p.max.is_some(), "{}.{}", d.id, p.id);
                }
            }
        }
        assert_eq!(
            descriptors().iter().map(|d| d.presets.len()).sum::<usize>(),
            80
        );
        assert!(shaper_presets().contains_key("pump"));
    }

    #[test]
    fn knob_positions_map_both_ways() {
        let d = descriptor("las-comp").unwrap();
        let attack = d.params.iter().find(|p| p.id == "attack").unwrap();
        let v = attack.from_normalized(0.5);
        assert!(v > 0.05 && v < 200.0);
        assert!((attack.to_normalized(v) - 0.5).abs() < 1e-9);
        assert!((attack.from_normalized(0.0) - 0.05).abs() < 1e-9);
        assert!((attack.from_normalized(1.0) - 200.0).abs() < 1e-9);
    }

    #[test]
    fn a_saved_state_with_a_sidechain_source_turns_the_flag_on() {
        let state = serde_json::json!({ "sidechain": "Kick", "threshold": -30 });
        let Params::Comp(p) = parse_params("las-comp", &state, 48000.0).unwrap() else {
            panic!("expected compressor parameters");
        };
        assert!(p.sidechain && p.threshold == -30.0);
        let Params::Comp(p) = parse_params("las-comp", &Value::Null, 48000.0).unwrap() else {
            panic!("expected compressor parameters");
        };
        assert!(!p.sidechain);
    }

    #[test]
    fn applying_parameters_swaps_them_and_hands_the_old_ones_back() {
        let mut u = EffectUnit::new("las-limiter", 48000.0).unwrap();
        let p = parse_params("las-limiter", &serde_json::json!({ "gain": 9 }), 48000.0).unwrap();
        let Params::Limiter(old) = u.apply(p) else {
            panic!("expected limiter parameters");
        };
        assert_eq!(old.gain, 0.0);
        // parameters for another effect change nothing
        let other = parse_params("las-crusher", &Value::Null, 48000.0).unwrap();
        assert!(matches!(u.apply(other), Params::Crusher(_)));
    }

    #[test]
    fn the_trance_gate_follows_its_steps() {
        let state = serde_json::json!({ "steps": [1, 0, 1, 0], "gate": 0.5, "div": 0.5 });
        let Params::Shaper(p) = parse_params("las-gate", &state, 48000.0).unwrap() else {
            panic!("expected shaper parameters");
        };
        assert_eq!(p.div, 2.0);
        assert!(p.points.len() == 13 && p.points[0].y == 1.0 && p.points[3].y == 0.0);
    }

    #[test]
    fn a_slot_keeps_the_signal_aligned_and_mixes() {
        let mut slot = FxSlot::new(EffectUnit::new("las-tape", 48000.0).unwrap(), 48000.0);
        slot.mix = 0.0;
        let x: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        for (a, b) in l.chunks_mut(128).zip(r.chunks_mut(128)) {
            slot.process(a, b, None);
        }
        // fully dry: the input, delayed by the tape latency (192 samples)
        let lat = (slot.latency() * 48000.0).round() as usize;
        assert_eq!(lat, 192);
        for i in lat + 8..2048 {
            assert!((l[i] - x[i - lat]).abs() < 1e-4, "sample {i}");
        }
        // a slot that is off passes the delayed input as well
        slot.mix = 1.0;
        slot.enabled = false;
        let (mut l2, mut r2) = (x.clone(), x.clone());
        for (a, b) in l2.chunks_mut(128).zip(r2.chunks_mut(128)) {
            slot.process(a, b, None);
        }
        assert!((l2[1000] - x[1000 - lat]).abs() < 1e-3);
    }

    #[test]
    fn the_meter_passes_audio_and_measures_it() {
        let mut u = EffectUnit::new("las-meter", 48000.0).unwrap();
        let x: Vec<f32> = (0..48000)
            .map(|i| (0.5 * (2.0 * std::f64::consts::PI * 997.0 * i as f64 / 48000.0).sin()) as f32)
            .collect();
        let (mut ol, mut or) = (vec![0f32; 48000], vec![0f32; 48000]);
        for i in (0..48000).step_by(128) {
            let end = (i + 128).min(48000);
            u.process(
                &x[i..end],
                &x[i..end],
                &mut ol[i..end],
                &mut or[i..end],
                None,
            );
        }
        assert_eq!(ol, x);
        let m = u.meter().unwrap();
        assert!(m.loudness.momentary() > -12.0 && m.loudness.momentary() < -5.0);
        assert!((m.correlation() - 1.0).abs() < 1e-9);
    }
}
