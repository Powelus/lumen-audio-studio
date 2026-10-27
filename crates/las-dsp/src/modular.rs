// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! Weave, a modular synth: modules (oscillators, envelopes, filters and so on) joined
//! by cables. A cable runs from a module's output to another module's input, and every
//! parameter is an input too, so anything can modulate anything; each cable has its
//! own amount. A loop of cables is allowed and runs one sample late.
//!
//! Each voice runs its own copy of the graph, one sample at a time; the `Keys` module
//! gives a voice its note. The sound is what reaches the first `Output` module, summed
//! over the voices. A [`Patch`] is plain data (it is kept in projects as JSON); it is
//! compiled once, so nothing is allocated while playing.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::PI;

/// The kinds of module.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The voice's note: pitch (octaves from middle C), gate, velocity and the
    /// performance controls.
    Keys,
    /// An oscillator with unison, sync, FM and a wave that can morph.
    Osc,
    /// A sine operator with a frequency ratio and feedback, for FM sounds.
    Fm,
    Noise,
    /// Four knobs whose values are outputs, to be cabled anywhere and automated.
    Macro,
    Lfo,
    /// An ADSR envelope that follows its gate.
    Env,
    /// Sample and hold: takes its input at a rate or on a trigger and keeps it.
    Hold,
    /// An eight step sequencer.
    Seq,
    /// Makes a signal move no faster than its rise and fall times.
    Slew,
    Filter,
    /// A four pole ladder lowpass with drive.
    Ladder,
    /// Wavefolder and distortion.
    Shaper,
    /// Ring modulator: one input times the other.
    Ring,
    Delay,
    Chorus,
    Reverb,
    /// Scales its input by a control signal.
    Vca,
    /// Four inputs with a level each.
    Mix,
    /// `a * scale + b + offset`.
    Math,
    /// Snaps a pitch (in octaves) to the semitones of a scale.
    Quant,
    /// 1 while `a` is above `b` plus a threshold, else 0.
    Compare,
    /// Places a mono signal between left and right.
    Pan,
    /// What the synth plays: left and right (right follows left when it has no cable).
    Output,
    /// Plays a sample of the project: from a start to an end, looped or not, at the
    /// key's pitch or its own.
    Sampler,
    /// A dot on a cable: what comes in goes out, to tidy cables.
    Reroute,
}

/// A parameter of a kind of module, in its own units.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// Moves on a log scale (frequencies and times).
    pub log: bool,
    /// The number of choices for a stepped parameter (0 for a continuous one).
    pub steps: u32,
}

const fn p(name: &'static str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        log: false,
        steps: 0,
    }
}

const fn plog(name: &'static str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        log: true,
        steps: 0,
    }
}

const fn pstep(name: &'static str, steps: u32, default: f64) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.0,
        max: (steps - 1) as f64,
        default,
        log: false,
        steps,
    }
}

/// A whole number from `min` to `max`.
const fn pint(name: &'static str, min: u32, max: u32, default: f64) -> ParamSpec {
    ParamSpec {
        name,
        min: min as f64,
        max: max as f64,
        default,
        log: false,
        steps: max - min + 1,
    }
}

impl ParamSpec {
    /// From 0 to 1.
    pub fn normalize(&self, v: f64) -> f64 {
        let v = v.clamp(self.min, self.max);
        if self.log {
            (v / self.min).ln() / (self.max / self.min).ln()
        } else if self.max > self.min {
            (v - self.min) / (self.max - self.min)
        } else {
            0.0
        }
    }

    /// From 0 to 1 back to the parameter's units.
    pub fn denormalize(&self, n: f64) -> f64 {
        let n = n.clamp(0.0, 1.0);
        let v = if self.log {
            self.min * (self.max / self.min).powf(n)
        } else {
            self.min + n * (self.max - self.min)
        };
        if self.steps > 0 {
            v.round()
        } else {
            v
        }
    }
}

/// An input that is not a parameter, with the value it has when nothing is plugged in.
#[derive(Clone, Copy, Debug)]
pub struct InputSpec {
    pub name: &'static str,
    pub unplugged: f32,
    /// Takes any number of cables (summed); other inputs take one, and a new cable
    /// plugged into one replaces the old.
    pub many: bool,
}

const fn i(name: &'static str) -> InputSpec {
    InputSpec {
        name,
        unplugged: 0.0,
        many: false,
    }
}

/// An input that sums every cable plugged into it.
const fn sum(name: &'static str) -> InputSpec {
    InputSpec {
        name,
        unplugged: 0.0,
        many: true,
    }
}

/// What a kind of module has.
#[derive(Debug)]
pub struct Spec {
    pub inputs: &'static [InputSpec],
    pub params: &'static [ParamSpec],
    pub outputs: &'static [&'static str],
}

/// The basic waves.
pub const WAVES: [&str; 4] = ["sine", "triangle", "saw", "square"];
/// The choices of an oscillator's `wave`: the basic waves, or `morph` between them.
pub const OSC_WAVES: [&str; 5] = ["sine", "triangle", "saw", "square", "morph"];
/// The choices of an LFO's `wave`: the basic waves and a random step.
pub const LFO_WAVES: [&str; 5] = ["sine", "triangle", "saw", "square", "random"];
/// The choices of a filter's `mode`.
pub const FILTER_MODES: [&str; 3] = ["lowpass", "bandpass", "highpass"];
/// The choices of a ladder's `mode`.
pub const LADDER_MODES: [&str; 2] = ["24 dB", "12 dB"];
/// The choices of a shaper's `shape`.
pub const SHAPES: [&str; 3] = ["tanh", "fold", "clip"];
/// The choices of a quantizer's `scale`.
/// What the Math module does with its inputs (a is scaled first, the offset added last).
pub const MATH_OPS: [&str; 6] = ["add", "subtract", "multiply", "min", "max", "average"];
/// A symbol for each of [`MATH_OPS`].
pub const MATH_SYMBOLS: [&str; 6] = ["+", "\u{2212}", "\u{d7}", "min", "max", "avg"];
pub const SCALES: [&str; 4] = ["chromatic", "major", "minor", "pentatonic"];
/// The notes of each scale, as a bit per semitone above the root.
const SCALE_BITS: [u16; 4] = [0xfff, 0xab5, 0x5ad, 0x295];
/// The names of the module categories, in the order [`Kind::category`] counts them.
pub const CATEGORIES: [&str; 5] = ["Sources", "Modulators", "Filters", "Effects", "Utilities"];

static KEYS: Spec = Spec {
    inputs: &[],
    params: &[p("glide", 0.0, 2.0, 0.0), p("bend_range", 0.0, 24.0, 2.0)],
    outputs: &["pitch", "gate", "velocity", "wheel", "bend", "pressure"],
};

static OSC: Spec = Spec {
    inputs: &[sum("pitch"), i("fm"), i("sync"), i("pfm")],
    params: &[
        pstep("wave", 5, 2.0),
        p("tune", -24.0, 24.0, 0.0),
        p("fine", -100.0, 100.0, 0.0),
        p("pw", 0.05, 0.95, 0.5),
        p("level", 0.0, 1.0, 1.0),
        p("morph", 0.0, 3.0, 0.0),
        pint("voices", 1, 8, 1.0),
        p("detune", 0.0, 100.0, 20.0),
        p("spread", 0.0, 1.0, 0.5),
        p("phase", 0.0, 1.0, 0.0),
        pstep("retrig", 2, 1.0),
    ],
    outputs: &["out", "right"],
};

static FM: Spec = Spec {
    inputs: &[sum("pitch"), i("pm")],
    params: &[
        plog("ratio", 0.125, 16.0, 1.0),
        p("tune", -24.0, 24.0, 0.0),
        p("feedback", 0.0, 1.0, 0.0),
        p("level", 0.0, 1.0, 1.0),
    ],
    outputs: &["out"],
};

static NOISE: Spec = Spec {
    inputs: &[],
    params: &[p("level", 0.0, 1.0, 1.0)],
    outputs: &["out"],
};

static MACRO: Spec = Spec {
    inputs: &[],
    params: &[
        p("knob_1", 0.0, 1.0, 0.0),
        p("knob_2", 0.0, 1.0, 0.0),
        p("knob_3", 0.0, 1.0, 0.0),
        p("knob_4", 0.0, 1.0, 0.0),
    ],
    outputs: &["out_1", "out_2", "out_3", "out_4"],
};

static LFO: Spec = Spec {
    inputs: &[],
    params: &[
        plog("rate", 0.01, 50.0, 2.0),
        pstep("wave", 5, 0.0),
        p("depth", 0.0, 1.0, 1.0),
        p("phase", 0.0, 1.0, 0.0),
        pstep("retrig", 2, 1.0),
    ],
    outputs: &["out", "uni"],
};

static ENV: Spec = Spec {
    inputs: &[i("gate")],
    params: &[
        plog("attack", 0.001, 10.0, 0.005),
        plog("decay", 0.001, 10.0, 0.3),
        p("sustain", 0.0, 1.0, 0.7),
        plog("release", 0.001, 10.0, 0.3),
        p("curve", -1.0, 1.0, 0.0),
        pstep("loop", 2, 0.0),
    ],
    outputs: &["out"],
};

static HOLD: Spec = Spec {
    inputs: &[i("in"), i("trig")],
    params: &[plog("rate", 0.05, 200.0, 8.0)],
    outputs: &["out"],
};

static SEQ: Spec = Spec {
    inputs: &[i("clock"), i("reset")],
    params: &[
        plog("rate", 0.1, 50.0, 4.0),
        pint("length", 1, 8, 8.0),
        p("gate_len", 0.05, 1.0, 0.5),
        p("step_1", -1.0, 1.0, 0.0),
        p("step_2", -1.0, 1.0, 0.0),
        p("step_3", -1.0, 1.0, 0.0),
        p("step_4", -1.0, 1.0, 0.0),
        p("step_5", -1.0, 1.0, 0.0),
        p("step_6", -1.0, 1.0, 0.0),
        p("step_7", -1.0, 1.0, 0.0),
        p("step_8", -1.0, 1.0, 0.0),
    ],
    outputs: &["out", "gate"],
};

static SLEW: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        plog("rise", 0.001, 10.0, 0.05),
        plog("fall", 0.001, 10.0, 0.05),
    ],
    outputs: &["out"],
};

static FILTER: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        pstep("mode", 3, 0.0),
        plog("cutoff", 20.0, 20_000.0, 2_000.0),
        p("res", 0.0, 1.0, 0.2),
    ],
    outputs: &["out"],
};

static LADDER: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        plog("cutoff", 20.0, 20_000.0, 1_000.0),
        p("res", 0.0, 1.0, 0.3),
        plog("drive", 1.0, 10.0, 1.0),
        pstep("mode", 2, 0.0),
    ],
    outputs: &["out"],
};

static SHAPER: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        plog("drive", 1.0, 40.0, 2.0),
        pstep("shape", 3, 0.0),
        p("mix", 0.0, 1.0, 1.0),
    ],
    outputs: &["out"],
};

static RING: Spec = Spec {
    inputs: &[i("a"), i("b")],
    params: &[p("mix", 0.0, 1.0, 1.0)],
    outputs: &["out"],
};

static DELAY: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        plog("time", 0.001, 2.0, 0.3),
        p("feedback", 0.0, 0.98, 0.4),
        p("mix", 0.0, 1.0, 0.3),
        p("tone", 0.0, 1.0, 0.7),
    ],
    outputs: &["out"],
};

static CHORUS: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        plog("rate", 0.05, 8.0, 0.6),
        p("depth", 0.0, 1.0, 0.5),
        p("mix", 0.0, 1.0, 0.5),
    ],
    outputs: &["left", "right"],
};

static REVERB: Spec = Spec {
    inputs: &[i("in")],
    params: &[
        p("size", 0.0, 1.0, 0.6),
        p("damp", 0.0, 1.0, 0.4),
        p("mix", 0.0, 1.0, 0.3),
    ],
    outputs: &["left", "right"],
};

static VCA: Spec = Spec {
    inputs: &[
        i("in"),
        InputSpec {
            name: "cv",
            unplugged: 1.0,
            many: false,
        },
    ],
    params: &[p("gain", 0.0, 1.0, 1.0)],
    outputs: &["out"],
};

static MIX: Spec = Spec {
    inputs: &[sum("a"), sum("b"), sum("c"), sum("d")],
    params: &[
        p("level_a", 0.0, 1.0, 1.0),
        p("level_b", 0.0, 1.0, 1.0),
        p("level_c", 0.0, 1.0, 1.0),
        p("level_d", 0.0, 1.0, 1.0),
    ],
    outputs: &["out"],
};

static MATH: Spec = Spec {
    inputs: &[i("a"), i("b")],
    params: &[
        p("scale", -4.0, 4.0, 1.0),
        p("offset", -1.0, 1.0, 0.0),
        pstep("op", 6, 0.0),
    ],
    outputs: &["out"],
};

static QUANT: Spec = Spec {
    inputs: &[i("in")],
    params: &[pstep("scale", 4, 0.0)],
    outputs: &["out"],
};

static COMPARE: Spec = Spec {
    inputs: &[i("a"), i("b")],
    params: &[p("threshold", -1.0, 1.0, 0.0)],
    outputs: &["out", "inv"],
};

static PAN: Spec = Spec {
    inputs: &[i("in")],
    params: &[p("pan", -1.0, 1.0, 0.0)],
    outputs: &["left", "right"],
};

static SAMPLER: Spec = Spec {
    inputs: &[sum("pitch"), i("gate"), i("pos")],
    params: &[
        p("start", 0.0, 1.0, 0.0),
        p("end", 0.0, 1.0, 1.0),
        pstep("loop", 2, 0.0),
        pstep("follow", 2, 1.0),
        pint("root", 0, 127, 60.0),
        pstep("trigger", 2, 0.0),
        pstep("reverse", 2, 0.0),
        p("gain", 0.0, 2.0, 1.0),
    ],
    outputs: &["left", "right"],
};

static REROUTE: Spec = Spec {
    inputs: &[i("in")],
    params: &[],
    outputs: &["out"],
};

static OUTPUT: Spec = Spec {
    inputs: &[sum("left"), sum("right")],
    params: &[p("level", 0.0, 1.0, 0.8), p("width", 0.0, 2.0, 1.0)],
    outputs: &[],
};

impl Kind {
    pub const ALL: [Kind; 26] = [
        Kind::Keys,
        Kind::Osc,
        Kind::Fm,
        Kind::Noise,
        Kind::Macro,
        Kind::Lfo,
        Kind::Env,
        Kind::Hold,
        Kind::Seq,
        Kind::Slew,
        Kind::Filter,
        Kind::Ladder,
        Kind::Shaper,
        Kind::Ring,
        Kind::Delay,
        Kind::Chorus,
        Kind::Reverb,
        Kind::Vca,
        Kind::Mix,
        Kind::Math,
        Kind::Quant,
        Kind::Compare,
        Kind::Pan,
        Kind::Output,
        Kind::Sampler,
        Kind::Reroute,
    ];

    pub fn spec(self) -> &'static Spec {
        match self {
            Kind::Keys => &KEYS,
            Kind::Osc => &OSC,
            Kind::Fm => &FM,
            Kind::Noise => &NOISE,
            Kind::Macro => &MACRO,
            Kind::Lfo => &LFO,
            Kind::Env => &ENV,
            Kind::Hold => &HOLD,
            Kind::Seq => &SEQ,
            Kind::Slew => &SLEW,
            Kind::Filter => &FILTER,
            Kind::Ladder => &LADDER,
            Kind::Shaper => &SHAPER,
            Kind::Ring => &RING,
            Kind::Delay => &DELAY,
            Kind::Chorus => &CHORUS,
            Kind::Reverb => &REVERB,
            Kind::Vca => &VCA,
            Kind::Mix => &MIX,
            Kind::Math => &MATH,
            Kind::Quant => &QUANT,
            Kind::Compare => &COMPARE,
            Kind::Pan => &PAN,
            Kind::Output => &OUTPUT,
            Kind::Sampler => &SAMPLER,
            Kind::Reroute => &REROUTE,
        }
    }

    /// Which of [`CATEGORIES`] the kind is in.
    pub fn category(self) -> usize {
        match self {
            Kind::Keys | Kind::Osc | Kind::Fm | Kind::Noise | Kind::Macro | Kind::Sampler => 0,
            Kind::Lfo | Kind::Env | Kind::Hold | Kind::Seq | Kind::Slew => 1,
            Kind::Filter | Kind::Ladder => 2,
            Kind::Shaper | Kind::Ring | Kind::Delay | Kind::Chorus | Kind::Reverb => 3,
            _ => 4,
        }
    }

    /// The names of a stepped parameter's choices (none for a plain number).
    pub fn choices(self, param: &str) -> Option<&'static [&'static str]> {
        Some(match (self, param) {
            (Kind::Osc, "wave") => &OSC_WAVES,
            (Kind::Lfo, "wave") => &LFO_WAVES,
            (Kind::Filter, "mode") => &FILTER_MODES,
            (Kind::Ladder, "mode") => &LADDER_MODES,
            (Kind::Shaper, "shape") => &SHAPES,
            (Kind::Quant, "scale") => &SCALES,
            (Kind::Math, "op") => &MATH_OPS,
            (Kind::Osc, "retrig") => &["free", "reset"],
            (Kind::Lfo, "retrig") => &["free", "on note"],
            (Kind::Env, "loop") => &["off", "on"],
            (Kind::Sampler, "loop") => &["off", "on"],
            (Kind::Sampler, "follow") => &["fixed", "key"],
            (Kind::Sampler, "trigger") => &["one-shot", "gate"],
            (Kind::Sampler, "reverse") => &["forward", "reverse"],
            _ => return None,
        })
    }

    /// The rows a module has in the editor: its outputs, inputs and parameters.
    pub fn rows(self) -> usize {
        let s = self.spec();
        s.outputs.len() + s.inputs.len() + s.params.len()
    }

    /// The input a cable can reach: a plain input, then the parameters.
    pub fn input_index(self, name: &str) -> Option<usize> {
        let s = self.spec();
        s.inputs.iter().position(|i| i.name == name).or_else(|| {
            s.params
                .iter()
                .position(|p| p.name == name)
                .map(|i| i + s.inputs.len())
        })
    }

    pub fn output_index(self, name: &str) -> Option<usize> {
        self.spec().outputs.iter().position(|o| *o == name)
    }

    /// Whether an input takes more than one cable. A parameter does (each cable
    /// moves it by its own amount, and they add up); a plain input says so in its spec.
    pub fn accepts_many(self, input: &str) -> bool {
        let s = self.spec();
        s.params.iter().any(|p| p.name == input)
            || s.inputs.iter().any(|i| i.name == input && i.many)
    }

    pub fn param(self, name: &str) -> Option<&'static ParamSpec> {
        self.spec().params.iter().find(|p| p.name == name)
    }
}

/// A module of a patch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u32,
    pub kind: Kind,
    /// Parameter values in their own units; a missing one has its default.
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
    /// Where the module sits in the editor.
    #[serde(default)]
    pub pos: [f32; 2],
    /// The project sample a sampler plays (its id in the project's samples).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
}

/// A labelled frame round some modules in the editor (it moves them together).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Frame {
    pub label: String,
    pub nodes: Vec<u32>,
}

/// A sample a sampler module plays: its channels and their rate.
#[derive(Clone, Debug)]
pub struct SampleBuf {
    pub left: std::sync::Arc<[f32]>,
    pub right: std::sync::Arc<[f32]>,
    pub rate: f64,
}

impl SampleBuf {
    pub fn len(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Node {
    pub fn new(id: u32, kind: Kind, pos: [f32; 2]) -> Self {
        Self {
            id,
            kind,
            params: BTreeMap::new(),
            pos,
            sample: None,
        }
    }

    /// A parameter's value (its default when the node has none).
    pub fn param(&self, name: &str) -> f64 {
        let Some(spec) = self.kind.param(name) else {
            return 0.0;
        };
        self.params
            .get(name)
            .copied()
            .unwrap_or(spec.default)
            .clamp(spec.min, spec.max)
    }

    pub fn with(mut self, name: &str, v: f64) -> Self {
        self.params.insert(name.to_string(), v);
        self
    }
}

/// A cable from an output to an input (or a parameter), scaled by `amount`. Into a
/// parameter, a signal of 1 times the amount moves it over its whole range.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Cable {
    pub from: u32,
    pub output: String,
    pub to: u32,
    pub input: String,
    #[serde(default = "one")]
    pub amount: f64,
}

fn one() -> f64 {
    1.0
}

impl Cable {
    pub fn new(from: u32, output: &str, to: u32, input: &str, amount: f64) -> Self {
        Self {
            from,
            output: output.into(),
            to,
            input: input.into(),
            amount,
        }
    }
}

/// A whole patch: modules, cables, how many voices play and how they play.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Patch {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub cables: Vec<Cable>,
    #[serde(default = "voices")]
    pub voices: usize,
    /// One voice only, which takes the newest key held.
    #[serde(default)]
    pub mono: bool,
    /// In mono, a key pressed while another is held moves the pitch without starting
    /// the envelopes again.
    #[serde(default)]
    pub legato: bool,
    /// In mono, how long the pitch takes to slide to a new key, in seconds.
    #[serde(default)]
    pub glide: f64,
    /// The level of the whole patch.
    #[serde(default = "one")]
    pub volume: f64,
    /// Frames round modules in the editor (they do not change the sound).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,
}

fn voices() -> usize {
    8
}

/// The id a parameter is automated by: the node's id and the parameter's place.
pub fn param_id(node: u32, index: usize) -> u32 {
    node * 64 + index as u32
}

/// The node and the parameter's place an automation id names.
pub fn split_param_id(id: u32) -> (u32, usize) {
    (id / 64, (id % 64) as usize)
}

/// The most voices a patch can have.
pub const MAX_VOICES: usize = 32;
/// The longest glide, in seconds.
pub const MAX_GLIDE: f64 = 2.0;

impl Default for Patch {
    /// Keys into a saw, through a lowpass and an amp that an envelope opens, which also
    /// moves the cutoff.
    fn default() -> Self {
        let mut p = Self::new(
            vec![
                Node::new(1, Kind::Keys, [20.0, 140.0]),
                Node::new(2, Kind::Osc, [240.0, 20.0]),
                Node::new(3, Kind::Env, [240.0, 300.0]),
                Node::new(4, Kind::Filter, [460.0, 20.0]).with("cutoff", 800.0),
                Node::new(5, Kind::Vca, [680.0, 140.0]),
                Node::new(6, Kind::Output, [900.0, 140.0]),
            ],
            vec![
                Cable::new(1, "pitch", 2, "pitch", 1.0),
                Cable::new(1, "gate", 3, "gate", 1.0),
                Cable::new(2, "out", 4, "in", 1.0),
                Cable::new(3, "out", 4, "cutoff", 0.35),
                Cable::new(4, "out", 5, "in", 1.0),
                Cable::new(3, "out", 5, "cv", 1.0),
                Cable::new(5, "out", 6, "left", 1.0),
            ],
            voices(),
        );
        p.arrange();
        p
    }
}

impl Patch {
    /// A patch of these modules and cables, played polyphonically at full volume.
    pub fn new(nodes: Vec<Node>, cables: Vec<Cable>, voices: usize) -> Self {
        Self {
            nodes,
            cables,
            voices,
            mono: false,
            legato: false,
            glide: 0.0,
            volume: 1.0,
            frames: Vec::new(),
        }
    }

    /// A patch read back. A single input with several cables (an older patch) keeps
    /// only the first; inputs that sum keep them all.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let mut p: Self = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        p.one_cable_per_input();
        Ok(p)
    }

    /// Drop every cable but the first into each input that takes one cable.
    pub fn one_cable_per_input(&mut self) {
        let mut seen: Vec<(u32, &str)> = Vec::new();
        let keep: Vec<bool> = self
            .cables
            .iter()
            .map(|c| {
                let Some(k) = self.node(c.to).map(|n| n.kind) else {
                    return true;
                };
                if !self.is_valid(c) || k.accepts_many(&c.input) {
                    return true;
                }
                let key = (c.to, c.input.as_str());
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                true
            })
            .collect();
        let mut keep = keep.into_iter();
        self.cables.retain(|_| keep.next().unwrap_or(true));
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// An id no node has.
    pub fn free_id(&self) -> u32 {
        self.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1
    }

    /// Whether a cable joins ports that exist.
    /// Why a module's parameter is not heard as the patch is now (`None` when it is):
    /// a control that only works with another setting, or that a cable overrides.
    pub fn idle_reason(&self, node: &Node, param: &str) -> Option<&'static str> {
        let plugged = |input: &str| {
            self.cables
                .iter()
                .any(|c| c.to == node.id && c.input == input && self.is_valid(c))
        };
        let wave = node.param("wave") as usize;
        match (node.kind, param) {
            (Kind::Osc, "pw") if wave != 3 && wave != 4 => {
                Some("only the square and morph waves have a pulse width")
            }
            (Kind::Osc, "morph") if wave != 4 => Some("only the morph wave morphs"),
            (Kind::Osc, "detune" | "spread") if node.param("voices") < 2.0 => {
                Some("needs 2 or more unison voices")
            }
            (Kind::Seq, "rate") if plugged("clock") => Some("the clock input sets the pace"),
            (Kind::Hold, "rate") if plugged("trig") => Some("the trig input sets the pace"),
            (Kind::Seq, n) if n.starts_with("step_") => {
                let k: f64 = n[5..].parse().unwrap_or(1.0);
                (k > node.param("length")).then_some("past the sequence's length")
            }
            _ => None,
        }
    }

    /// Whether `other` has the same modules (ids and kinds, in order) and the same
    /// cables (ends and order) as this patch: it differs only in values (parameters,
    /// cable amounts, voices, mono, glide, volume, places), so a playing synth can take
    /// it live with [`Modular::apply_live`].
    pub fn same_shape(&self, other: &Patch) -> bool {
        self.nodes.len() == other.nodes.len()
            && self.cables.len() == other.cables.len()
            && self
                .nodes
                .iter()
                .zip(&other.nodes)
                .all(|(a, b)| a.id == b.id && a.kind == b.kind && a.sample == b.sample)
            && self.cables.iter().zip(&other.cables).all(|(a, b)| {
                a.from == b.from && a.to == b.to && a.output == b.output && a.input == b.input
            })
    }

    pub fn is_valid(&self, c: &Cable) -> bool {
        let (Some(a), Some(b)) = (self.node(c.from), self.node(c.to)) else {
            return false;
        };
        a.kind.output_index(&c.output).is_some() && b.kind.input_index(&c.input).is_some()
    }

    /// Place the modules in columns, each after the ones that feed it.
    pub fn arrange(&mut self) {
        let n = self.nodes.len();
        let index = |id: u32| self.nodes.iter().position(|x| x.id == id);
        let links: Vec<(usize, usize)> = self
            .cables
            .iter()
            .filter(|c| self.is_valid(c))
            .filter_map(|c| Some((index(c.from)?, index(c.to)?)))
            .filter(|(a, b)| a != b)
            .collect();
        let mut col = vec![0usize; n];
        // a loop stops growing at the number of modules
        for _ in 0..n {
            for &(a, b) in &links {
                if col[b] < col[a] + 1 && col[a] + 1 < n {
                    col[b] = col[a] + 1;
                }
            }
        }
        // a tall column goes on in another one beside it
        const TALL: f32 = 760.0;
        let mut x = 20.0;
        for column in 0..n {
            let mut y = 20.0;
            let mut used = false;
            for (k, node) in self.nodes.iter_mut().enumerate() {
                if col[k] != column {
                    continue;
                }
                let h = 50.0 + node.kind.rows() as f32 * 20.0;
                if y > 20.0 && y + h > TALL {
                    x += 220.0;
                    y = 20.0;
                }
                node.pos = [x, y];
                y += h;
                used = true;
            }
            if used {
                x += 220.0;
            }
        }
    }
}

/// A module with some parameters set, for the factory patches.
fn m(id: u32, kind: Kind, params: &[(&str, f64)]) -> Node {
    params
        .iter()
        .fold(Node::new(id, kind, [0.0; 2]), |n, &(k, v)| n.with(k, v))
}

/// A factory patch: its modules placed in columns.
fn built(voices: usize, nodes: Vec<Node>, cables: Vec<Cable>) -> Patch {
    let mut p = Patch::new(nodes, cables, voices);
    p.arrange();
    p
}

/// The same patch played by one voice that glides.
fn mono(mut p: Patch, glide: f64, legato: bool) -> Patch {
    p.mono = true;
    p.voices = 1;
    p.glide = glide;
    p.legato = legato;
    p
}

/// The factory patches: a name and the patch.
pub fn factory() -> Vec<(&'static str, Patch)> {
    use Kind::*;
    let c = Cable::new;
    vec![
        // ----- bass
        (
            "Wobble bass",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("tune", -12.0)]),
                    m(3, Osc, &[("wave", 3.0), ("tune", -12.0), ("fine", 7.0)]),
                    m(4, Mix, &[]),
                    m(5, Lfo, &[("rate", 3.0)]),
                    m(6, Filter, &[("cutoff", 300.0), ("res", 0.6)]),
                    m(7, Env, &[("release", 0.1)]),
                    m(8, Vca, &[]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(2, "out", 4, "a", 0.6),
                    c(3, "out", 4, "b", 0.6),
                    c(4, "out", 6, "in", 1.0),
                    c(5, "out", 6, "cutoff", 0.3),
                    c(1, "gate", 7, "gate", 1.0),
                    c(6, "out", 8, "in", 1.0),
                    c(7, "out", 8, "cv", 1.0),
                    c(8, "out", 9, "left", 1.0),
                ],
            ),
        ),
        (
            "Sub bass",
            mono(
                built(
                    1,
                    vec![
                        m(1, Keys, &[]),
                        m(2, Osc, &[("wave", 0.0), ("tune", -12.0)]),
                        m(3, Osc, &[("wave", 1.0), ("tune", -24.0), ("level", 0.5)]),
                        m(4, Mix, &[]),
                        m(
                            5,
                            Env,
                            &[("decay", 0.3), ("sustain", 0.8), ("release", 0.15)],
                        ),
                        m(6, Vca, &[]),
                        m(7, Output, &[]),
                    ],
                    vec![
                        c(1, "pitch", 2, "pitch", 1.0),
                        c(1, "pitch", 3, "pitch", 1.0),
                        c(2, "out", 4, "a", 0.7),
                        c(3, "out", 4, "b", 0.5),
                        c(1, "gate", 5, "gate", 1.0),
                        c(4, "out", 6, "in", 1.0),
                        c(5, "out", 6, "cv", 1.0),
                        c(6, "out", 7, "left", 1.0),
                    ],
                ),
                0.08,
                true,
            ),
        ),
        (
            "Acid bass",
            mono(
                built(
                    1,
                    vec![
                        m(1, Keys, &[]),
                        m(2, Osc, &[("tune", -12.0)]),
                        m(
                            3,
                            Ladder,
                            &[("cutoff", 250.0), ("res", 0.8), ("drive", 2.0)],
                        ),
                        m(
                            4,
                            Env,
                            &[("attack", 0.002), ("decay", 0.25), ("sustain", 0.0)],
                        ),
                        m(
                            5,
                            Env,
                            &[("attack", 0.002), ("sustain", 0.8), ("release", 0.08)],
                        ),
                        m(6, Vca, &[]),
                        m(7, Shaper, &[("drive", 2.0), ("mix", 0.5)]),
                        m(8, Output, &[]),
                    ],
                    vec![
                        c(1, "pitch", 2, "pitch", 1.0),
                        c(1, "gate", 4, "gate", 1.0),
                        c(1, "gate", 5, "gate", 1.0),
                        c(2, "out", 3, "in", 1.0),
                        c(4, "out", 3, "cutoff", 0.45),
                        c(1, "velocity", 3, "res", 0.15),
                        c(3, "out", 6, "in", 1.0),
                        c(5, "out", 6, "cv", 1.0),
                        c(6, "out", 7, "in", 1.0),
                        c(7, "out", 8, "left", 1.0),
                    ],
                ),
                0.06,
                true,
            ),
        ),
        (
            "Reese bass",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(
                        2,
                        Osc,
                        &[
                            ("tune", -12.0),
                            ("voices", 4.0),
                            ("detune", 25.0),
                            ("spread", 0.7),
                        ],
                    ),
                    m(3, Ladder, &[("cutoff", 500.0), ("res", 0.3)]),
                    m(4, Ladder, &[("cutoff", 500.0), ("res", 0.3)]),
                    m(5, Lfo, &[("rate", 0.3)]),
                    m(6, Env, &[("sustain", 0.9), ("release", 0.2)]),
                    m(7, Vca, &[]),
                    m(8, Vca, &[]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(2, "out", 3, "in", 1.0),
                    c(2, "right", 4, "in", 1.0),
                    c(5, "out", 3, "cutoff", 0.15),
                    c(5, "out", 4, "cutoff", 0.15),
                    c(3, "out", 7, "in", 1.0),
                    c(4, "out", 8, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(6, "out", 8, "cv", 1.0),
                    c(7, "out", 9, "left", 1.0),
                    c(8, "out", 9, "right", 1.0),
                ],
            ),
        ),
        (
            "FM bass",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Fm, &[("tune", -12.0), ("feedback", 0.3)]),
                    m(3, Fm, &[("ratio", 2.0), ("tune", -12.0)]),
                    m(4, Env, &[("decay", 0.2), ("sustain", 0.2)]),
                    m(5, Vca, &[]),
                    m(6, Env, &[("sustain", 0.8), ("release", 0.1)]),
                    m(7, Vca, &[]),
                    m(8, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(1, "gate", 4, "gate", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(3, "out", 5, "in", 1.0),
                    c(4, "out", 5, "cv", 1.0),
                    c(5, "out", 2, "pm", 0.6),
                    c(2, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "left", 1.0),
                ],
            ),
        ),
        // ----- lead
        ("Saw lead", Patch::default()),
        (
            "Sync lead",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("wave", 0.0)]),
                    m(3, Osc, &[]),
                    m(4, Env, &[("decay", 0.6), ("sustain", 0.2)]),
                    m(5, Env, &[("sustain", 0.8), ("release", 0.2)]),
                    m(6, Filter, &[("cutoff", 4000.0)]),
                    m(7, Vca, &[]),
                    m(8, Output, &[("level", 0.6)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(2, "out", 3, "sync", 1.0),
                    c(1, "gate", 4, "gate", 1.0),
                    c(1, "gate", 5, "gate", 1.0),
                    c(4, "out", 3, "tune", 0.4),
                    c(3, "out", 6, "in", 1.0),
                    c(6, "out", 7, "in", 1.0),
                    c(5, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "left", 1.0),
                ],
            ),
        ),
        (
            "Supersaw lead",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(
                        2,
                        Osc,
                        &[("voices", 7.0), ("detune", 30.0), ("spread", 0.9)],
                    ),
                    m(3, Filter, &[("cutoff", 5000.0)]),
                    m(4, Filter, &[("cutoff", 5000.0)]),
                    m(5, Env, &[("sustain", 0.8), ("release", 0.3)]),
                    m(6, Vca, &[]),
                    m(7, Vca, &[]),
                    m(8, Output, &[("level", 0.6), ("width", 1.3)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "gate", 5, "gate", 1.0),
                    c(2, "out", 3, "in", 1.0),
                    c(2, "right", 4, "in", 1.0),
                    c(3, "out", 6, "in", 1.0),
                    c(4, "out", 7, "in", 1.0),
                    c(5, "out", 6, "cv", 1.0),
                    c(5, "out", 7, "cv", 1.0),
                    c(6, "out", 8, "left", 1.0),
                    c(7, "out", 8, "right", 1.0),
                ],
            ),
        ),
        (
            "Glide lead",
            mono(
                built(
                    1,
                    vec![
                        m(1, Keys, &[]),
                        m(2, Osc, &[("wave", 3.0), ("pw", 0.4)]),
                        m(3, Osc, &[("fine", 6.0)]),
                        m(4, Mix, &[("level_a", 0.5), ("level_b", 0.5)]),
                        m(5, Lfo, &[("rate", 5.5)]),
                        m(6, Filter, &[("cutoff", 2500.0), ("res", 0.4)]),
                        m(7, Env, &[("sustain", 0.9), ("release", 0.2)]),
                        m(8, Vca, &[]),
                        m(
                            9,
                            Delay,
                            &[("time", 0.33), ("feedback", 0.35), ("mix", 0.25)],
                        ),
                        m(10, Output, &[]),
                    ],
                    vec![
                        c(1, "pitch", 2, "pitch", 1.0),
                        c(1, "pitch", 3, "pitch", 1.0),
                        c(5, "out", 2, "pfm", 0.01),
                        c(5, "out", 3, "pfm", 0.01),
                        c(1, "wheel", 5, "depth", 1.0),
                        c(2, "out", 4, "a", 1.0),
                        c(3, "out", 4, "b", 1.0),
                        c(4, "out", 6, "in", 1.0),
                        c(1, "gate", 7, "gate", 1.0),
                        c(6, "out", 8, "in", 1.0),
                        c(7, "out", 8, "cv", 1.0),
                        c(8, "out", 9, "in", 1.0),
                        c(9, "out", 10, "left", 1.0),
                    ],
                ),
                0.12,
                true,
            ),
        ),
        (
            "Fold lead",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("wave", 0.0)]),
                    m(3, Shaper, &[("shape", 1.0), ("drive", 2.0)]),
                    m(4, Lfo, &[("rate", 0.5)]),
                    m(5, Env, &[("decay", 0.8), ("sustain", 0.3)]),
                    m(6, Env, &[("sustain", 0.8), ("release", 0.25)]),
                    m(7, Vca, &[]),
                    m(8, Filter, &[("cutoff", 6000.0)]),
                    m(9, Output, &[("level", 0.6)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(2, "out", 3, "in", 1.0),
                    c(4, "out", 3, "drive", 0.15),
                    c(5, "out", 3, "drive", 0.3),
                    c(1, "gate", 5, "gate", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(3, "out", 8, "in", 1.0),
                    c(8, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 9, "left", 1.0),
                ],
            ),
        ),
        // ----- pad
        (
            "Slow pad",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("fine", -8.0)]),
                    m(3, Osc, &[("fine", 8.0)]),
                    m(4, Lfo, &[("rate", 0.2)]),
                    m(5, Filter, &[("cutoff", 1200.0)]),
                    m(
                        6,
                        Env,
                        &[("attack", 1.2), ("sustain", 0.9), ("release", 2.0)],
                    ),
                    m(7, Vca, &[]),
                    m(8, Output, &[("level", 0.6)]),
                    m(9, Mix, &[("level_a", 0.5), ("level_b", 0.5)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(2, "out", 9, "a", 1.0),
                    c(3, "out", 9, "b", 1.0),
                    c(9, "out", 5, "in", 1.0),
                    c(4, "out", 5, "cutoff", 0.1),
                    c(1, "gate", 6, "gate", 1.0),
                    c(5, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "left", 1.0),
                ],
            ),
        ),
        (
            "Morph pad",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(
                        2,
                        Osc,
                        &[
                            ("wave", 4.0),
                            ("morph", 0.5),
                            ("voices", 3.0),
                            ("detune", 12.0),
                            ("spread", 0.7),
                        ],
                    ),
                    m(3, Lfo, &[("rate", 0.15)]),
                    m(4, Filter, &[("cutoff", 1800.0)]),
                    m(5, Filter, &[("cutoff", 1800.0)]),
                    m(
                        6,
                        Env,
                        &[("attack", 0.8), ("sustain", 0.9), ("release", 1.5)],
                    ),
                    m(7, Vca, &[]),
                    m(8, Vca, &[]),
                    m(9, Output, &[("level", 0.6)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(3, "uni", 2, "morph", 0.6),
                    c(2, "out", 4, "in", 1.0),
                    c(2, "right", 5, "in", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(4, "out", 7, "in", 1.0),
                    c(5, "out", 8, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(6, "out", 8, "cv", 1.0),
                    c(7, "out", 9, "left", 1.0),
                    c(8, "out", 9, "right", 1.0),
                ],
            ),
        ),
        (
            "Shimmer pad",
            built(
                6,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("voices", 5.0), ("detune", 18.0), ("tune", 12.0)]),
                    m(3, Osc, &[("wave", 1.0), ("tune", -12.0)]),
                    m(4, Mix, &[("level_a", 0.4), ("level_b", 0.6)]),
                    m(5, Filter, &[("cutoff", 2200.0)]),
                    m(
                        6,
                        Env,
                        &[("attack", 0.6), ("sustain", 0.9), ("release", 1.5)],
                    ),
                    m(7, Vca, &[]),
                    m(8, Reverb, &[("size", 0.85), ("mix", 0.5)]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(2, "out", 4, "a", 1.0),
                    c(3, "out", 4, "b", 1.0),
                    c(4, "out", 5, "in", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(5, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "left", 9, "left", 1.0),
                    c(8, "right", 9, "right", 1.0),
                ],
            ),
        ),
        (
            "Chorus strings",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("fine", -6.0)]),
                    m(3, Osc, &[("fine", 6.0)]),
                    m(4, Mix, &[("level_a", 0.5), ("level_b", 0.5)]),
                    m(5, Filter, &[("cutoff", 3000.0)]),
                    m(
                        6,
                        Env,
                        &[("attack", 0.3), ("sustain", 0.9), ("release", 0.8)],
                    ),
                    m(7, Vca, &[]),
                    m(8, Chorus, &[("rate", 0.5), ("depth", 0.7)]),
                    m(9, Output, &[("level", 0.7)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(2, "out", 4, "a", 1.0),
                    c(3, "out", 4, "b", 1.0),
                    c(4, "out", 5, "in", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(5, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "left", 9, "left", 1.0),
                    c(8, "right", 9, "right", 1.0),
                ],
            ),
        ),
        // ----- pluck
        (
            "Pluck",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("wave", 3.0), ("pw", 0.3)]),
                    m(
                        3,
                        Env,
                        &[("decay", 0.25), ("sustain", 0.0), ("release", 0.2)],
                    ),
                    m(4, Filter, &[("cutoff", 400.0), ("res", 0.3)]),
                    m(5, Vca, &[]),
                    m(6, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "gate", 3, "gate", 1.0),
                    c(2, "out", 4, "in", 1.0),
                    c(3, "out", 4, "cutoff", 0.5),
                    c(4, "out", 5, "in", 1.0),
                    c(3, "out", 5, "cv", 1.0),
                    c(1, "velocity", 5, "gain", 0.5),
                    c(5, "out", 6, "left", 1.0),
                ],
            ),
        ),
        (
            "Echo pluck",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[]),
                    m(
                        3,
                        Env,
                        &[("decay", 0.2), ("sustain", 0.0), ("release", 0.2)],
                    ),
                    m(4, Filter, &[("cutoff", 600.0), ("res", 0.4)]),
                    m(5, Vca, &[]),
                    m(
                        6,
                        Delay,
                        &[("time", 0.375), ("feedback", 0.5), ("mix", 0.35)],
                    ),
                    m(7, Pan, &[]),
                    m(8, Lfo, &[("rate", 0.2)]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "gate", 3, "gate", 1.0),
                    c(2, "out", 4, "in", 1.0),
                    c(3, "out", 4, "cutoff", 0.5),
                    c(4, "out", 5, "in", 1.0),
                    c(3, "out", 5, "cv", 1.0),
                    c(5, "out", 6, "in", 1.0),
                    c(6, "out", 7, "in", 1.0),
                    c(8, "out", 7, "pan", 0.3),
                    c(7, "left", 9, "left", 1.0),
                    c(7, "right", 9, "right", 1.0),
                ],
            ),
        ),
        (
            "FM bell",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Fm, &[]),
                    m(3, Fm, &[("ratio", 3.5)]),
                    m(
                        4,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 1.5),
                            ("sustain", 0.0),
                            ("release", 1.5),
                        ],
                    ),
                    m(5, Vca, &[]),
                    m(
                        6,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 2.5),
                            ("sustain", 0.0),
                            ("release", 2.0),
                        ],
                    ),
                    m(7, Vca, &[]),
                    m(8, Reverb, &[("size", 0.6), ("mix", 0.25)]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(1, "gate", 4, "gate", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(3, "out", 5, "in", 1.0),
                    c(4, "out", 5, "cv", 1.0),
                    c(5, "out", 2, "pm", 0.5),
                    c(2, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "left", 9, "left", 1.0),
                    c(8, "right", 9, "right", 1.0),
                ],
            ),
        ),
        // ----- keys
        (
            "Electric piano",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Fm, &[]),
                    m(3, Fm, &[]),
                    m(4, Env, &[("decay", 0.5), ("sustain", 0.1)]),
                    m(5, Vca, &[("gain", 0.4)]),
                    m(
                        6,
                        Env,
                        &[("decay", 1.2), ("sustain", 0.3), ("release", 0.4)],
                    ),
                    m(7, Vca, &[]),
                    m(8, Chorus, &[("depth", 0.3), ("mix", 0.3)]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(1, "gate", 4, "gate", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(3, "out", 5, "in", 1.0),
                    c(4, "out", 5, "cv", 1.0),
                    c(1, "velocity", 5, "gain", 0.5),
                    c(5, "out", 2, "pm", 0.4),
                    c(2, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "left", 9, "left", 1.0),
                    c(8, "right", 9, "right", 1.0),
                ],
            ),
        ),
        (
            "Organ",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("wave", 0.0)]),
                    m(3, Osc, &[("wave", 0.0), ("tune", 12.0)]),
                    m(4, Osc, &[("wave", 0.0), ("tune", 19.0)]),
                    m(5, Osc, &[("wave", 0.0), ("tune", 24.0)]),
                    m(
                        6,
                        Mix,
                        &[
                            ("level_a", 0.5),
                            ("level_b", 0.35),
                            ("level_c", 0.25),
                            ("level_d", 0.2),
                        ],
                    ),
                    m(
                        7,
                        Env,
                        &[
                            ("attack", 0.005),
                            ("decay", 0.1),
                            ("sustain", 1.0),
                            ("release", 0.05),
                        ],
                    ),
                    m(8, Vca, &[]),
                    m(9, Chorus, &[("rate", 5.5), ("depth", 0.3), ("mix", 0.4)]),
                    m(10, Output, &[("level", 0.6)]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(1, "pitch", 4, "pitch", 1.0),
                    c(1, "pitch", 5, "pitch", 1.0),
                    c(2, "out", 6, "a", 1.0),
                    c(3, "out", 6, "b", 1.0),
                    c(4, "out", 6, "c", 1.0),
                    c(5, "out", 6, "d", 1.0),
                    c(1, "gate", 7, "gate", 1.0),
                    c(6, "out", 8, "in", 1.0),
                    c(7, "out", 8, "cv", 1.0),
                    c(8, "out", 9, "in", 1.0),
                    c(9, "left", 10, "left", 1.0),
                    c(9, "right", 10, "right", 1.0),
                ],
            ),
        ),
        // ----- effects
        (
            "Noise sweep",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Noise, &[]),
                    m(
                        3,
                        Env,
                        &[
                            ("attack", 0.5),
                            ("decay", 1.5),
                            ("sustain", 0.0),
                            ("release", 0.5),
                        ],
                    ),
                    m(4, Filter, &[("mode", 1.0), ("cutoff", 200.0), ("res", 0.7)]),
                    m(5, Vca, &[]),
                    m(6, Output, &[]),
                ],
                vec![
                    c(1, "gate", 3, "gate", 1.0),
                    c(2, "out", 4, "in", 1.0),
                    c(3, "out", 4, "cutoff", 0.7),
                    c(4, "out", 5, "in", 1.0),
                    c(3, "out", 5, "cv", 1.0),
                    c(5, "out", 6, "left", 1.0),
                ],
            ),
        ),
        (
            "Riser",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Noise, &[("level", 0.5)]),
                    m(3, Osc, &[("voices", 4.0), ("detune", 40.0)]),
                    m(
                        4,
                        Env,
                        &[("attack", 4.0), ("sustain", 1.0), ("release", 1.0)],
                    ),
                    m(5, Mix, &[]),
                    m(6, Filter, &[("cutoff", 400.0), ("res", 0.5)]),
                    m(7, Vca, &[]),
                    m(8, Reverb, &[("mix", 0.3)]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(1, "gate", 4, "gate", 1.0),
                    c(4, "out", 3, "tune", 0.5),
                    c(4, "out", 6, "cutoff", 0.6),
                    c(2, "out", 5, "a", 1.0),
                    c(3, "out", 5, "b", 0.6),
                    c(5, "out", 6, "in", 1.0),
                    c(6, "out", 7, "in", 1.0),
                    c(4, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "left", 9, "left", 1.0),
                    c(8, "right", 9, "right", 1.0),
                ],
            ),
        ),
        (
            "Random bleeps",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Hold, &[("rate", 8.0)]),
                    m(3, Quant, &[("scale", 3.0)]),
                    m(4, Osc, &[("wave", 3.0)]),
                    m(5, Env, &[("sustain", 0.8), ("release", 0.3)]),
                    m(6, Filter, &[("cutoff", 2500.0), ("res", 0.3)]),
                    m(7, Vca, &[]),
                    m(8, Delay, &[("time", 0.25), ("feedback", 0.4), ("mix", 0.3)]),
                    m(9, Output, &[("level", 0.5)]),
                ],
                vec![
                    c(2, "out", 3, "in", 1.0),
                    c(1, "pitch", 4, "pitch", 1.0),
                    c(3, "out", 4, "pitch", 1.0),
                    c(1, "gate", 5, "gate", 1.0),
                    c(4, "out", 6, "in", 1.0),
                    c(6, "out", 7, "in", 1.0),
                    c(5, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "out", 9, "left", 1.0),
                ],
            ),
        ),
        (
            "Ring robot",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[]),
                    m(3, Osc, &[("wave", 0.0), ("tune", 7.0)]),
                    m(4, Ring, &[]),
                    m(5, Lfo, &[("rate", 4.0)]),
                    m(
                        6,
                        Filter,
                        &[("mode", 1.0), ("cutoff", 1200.0), ("res", 0.5)],
                    ),
                    m(7, Env, &[("sustain", 0.8), ("release", 0.2)]),
                    m(8, Vca, &[]),
                    m(9, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(5, "out", 3, "tune", 0.1),
                    c(2, "out", 4, "a", 1.0),
                    c(3, "out", 4, "b", 1.0),
                    c(4, "out", 6, "in", 1.0),
                    c(1, "gate", 7, "gate", 1.0),
                    c(6, "out", 8, "in", 1.0),
                    c(7, "out", 8, "cv", 1.0),
                    c(8, "out", 9, "left", 1.0),
                ],
            ),
        ),
        // ----- percussion
        (
            "Kick",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Osc, &[("wave", 0.0), ("tune", -24.0)]),
                    m(
                        3,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 0.08),
                            ("sustain", 0.0),
                            ("release", 0.05),
                        ],
                    ),
                    m(
                        4,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 0.4),
                            ("sustain", 0.0),
                            ("release", 0.2),
                        ],
                    ),
                    m(5, Vca, &[]),
                    m(6, Shaper, &[("drive", 3.0), ("mix", 0.6)]),
                    m(7, Output, &[]),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 0.25),
                    c(1, "gate", 3, "gate", 1.0),
                    c(1, "gate", 4, "gate", 1.0),
                    c(3, "out", 2, "tune", 0.6),
                    c(2, "out", 5, "in", 1.0),
                    c(4, "out", 5, "cv", 1.0),
                    c(5, "out", 6, "in", 1.0),
                    c(6, "out", 7, "left", 1.0),
                ],
            ),
        ),
        (
            "Snare",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Noise, &[]),
                    m(
                        3,
                        Filter,
                        &[("mode", 1.0), ("cutoff", 3000.0), ("res", 0.2)],
                    ),
                    m(4, Osc, &[("wave", 1.0), ("tune", -12.0)]),
                    m(
                        5,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 0.1),
                            ("sustain", 0.0),
                            ("release", 0.1),
                        ],
                    ),
                    m(
                        6,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 0.18),
                            ("sustain", 0.0),
                            ("release", 0.15),
                        ],
                    ),
                    m(7, Vca, &[]),
                    m(8, Vca, &[]),
                    m(9, Mix, &[("level_a", 0.8), ("level_b", 0.6)]),
                    m(10, Output, &[]),
                ],
                vec![
                    c(1, "gate", 5, "gate", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(2, "out", 3, "in", 1.0),
                    c(3, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(4, "out", 8, "in", 1.0),
                    c(5, "out", 8, "cv", 1.0),
                    c(7, "out", 9, "a", 1.0),
                    c(8, "out", 9, "b", 1.0),
                    c(9, "out", 10, "left", 1.0),
                ],
            ),
        ),
        (
            "Hi hat",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Noise, &[]),
                    m(
                        3,
                        Filter,
                        &[("mode", 2.0), ("cutoff", 7000.0), ("res", 0.3)],
                    ),
                    m(
                        4,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 0.08),
                            ("sustain", 0.0),
                            ("release", 0.05),
                        ],
                    ),
                    m(5, Vca, &[]),
                    m(6, Output, &[("level", 1.0)]),
                ],
                vec![
                    c(1, "gate", 4, "gate", 1.0),
                    c(2, "out", 3, "in", 1.0),
                    c(3, "out", 5, "in", 1.0),
                    c(4, "out", 5, "cv", 1.0),
                    c(5, "out", 6, "left", 1.0),
                ],
            ),
        ),
        // ----- sequenced
        (
            "Step arp",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(
                        2,
                        Seq,
                        &[
                            ("rate", 8.0),
                            ("step_1", 0.0),
                            ("step_2", 0.25),
                            ("step_3", 0.5833),
                            ("step_4", 1.0),
                            ("step_5", 0.5833),
                            ("step_6", 0.25),
                            ("step_7", 0.4167),
                            ("step_8", 0.8333),
                        ],
                    ),
                    m(3, Quant, &[("scale", 1.0)]),
                    m(4, Osc, &[]),
                    m(
                        5,
                        Env,
                        &[
                            ("attack", 0.002),
                            ("decay", 0.12),
                            ("sustain", 0.0),
                            ("release", 0.1),
                        ],
                    ),
                    m(6, Ladder, &[("cutoff", 900.0), ("res", 0.5)]),
                    m(7, Vca, &[]),
                    m(
                        8,
                        Env,
                        &[("attack", 0.001), ("sustain", 1.0), ("release", 0.1)],
                    ),
                    m(9, Vca, &[]),
                    m(
                        10,
                        Delay,
                        &[("time", 0.1875), ("feedback", 0.45), ("mix", 0.3)],
                    ),
                    m(11, Output, &[]),
                ],
                vec![
                    c(2, "out", 3, "in", 1.0),
                    c(1, "pitch", 4, "pitch", 1.0),
                    c(3, "out", 4, "pitch", 1.0),
                    c(2, "gate", 5, "gate", 1.0),
                    c(4, "out", 6, "in", 1.0),
                    c(5, "out", 6, "cutoff", 0.35),
                    c(6, "out", 7, "in", 1.0),
                    c(5, "out", 7, "cv", 1.0),
                    c(1, "gate", 8, "gate", 1.0),
                    c(7, "out", 9, "in", 1.0),
                    c(8, "out", 9, "cv", 1.0),
                    c(9, "out", 10, "in", 1.0),
                    c(10, "out", 11, "left", 1.0),
                ],
            ),
        ),
        (
            "Sequenced bass",
            mono(
                built(
                    1,
                    vec![
                        m(1, Keys, &[]),
                        m(
                            2,
                            Seq,
                            &[
                                ("rate", 6.0),
                                ("gate_len", 0.6),
                                ("step_3", 1.0),
                                ("step_5", 0.5833),
                                ("step_7", 0.25),
                                ("step_8", 1.0),
                            ],
                        ),
                        m(3, Osc, &[("tune", -12.0)]),
                        m(4, Osc, &[("wave", 3.0), ("tune", -24.0), ("level", 0.5)]),
                        m(5, Mix, &[]),
                        m(
                            6,
                            Ladder,
                            &[("cutoff", 400.0), ("res", 0.6), ("drive", 2.0)],
                        ),
                        m(
                            7,
                            Env,
                            &[
                                ("attack", 0.002),
                                ("decay", 0.15),
                                ("sustain", 0.2),
                                ("release", 0.05),
                            ],
                        ),
                        m(8, Vca, &[]),
                        m(
                            9,
                            Env,
                            &[("attack", 0.001), ("sustain", 1.0), ("release", 0.1)],
                        ),
                        m(10, Vca, &[]),
                        m(11, Output, &[]),
                    ],
                    vec![
                        c(1, "pitch", 3, "pitch", 1.0),
                        c(1, "pitch", 4, "pitch", 1.0),
                        c(2, "out", 3, "pitch", 1.0),
                        c(2, "out", 4, "pitch", 1.0),
                        c(3, "out", 5, "a", 1.0),
                        c(4, "out", 5, "b", 1.0),
                        c(5, "out", 6, "in", 1.0),
                        c(2, "gate", 7, "gate", 1.0),
                        c(7, "out", 6, "cutoff", 0.4),
                        c(6, "out", 8, "in", 1.0),
                        c(7, "out", 8, "cv", 1.0),
                        c(1, "gate", 9, "gate", 1.0),
                        c(8, "out", 10, "in", 1.0),
                        c(9, "out", 10, "cv", 1.0),
                        c(10, "out", 11, "left", 1.0),
                    ],
                ),
                0.0,
                false,
            ),
        ),
        (
            "Gate rhythm",
            built(
                4,
                vec![
                    m(1, Keys, &[]),
                    m(2, Lfo, &[("rate", 6.0), ("wave", 2.0)]),
                    m(3, Lfo, &[("rate", 0.25)]),
                    m(4, Compare, &[]),
                    m(
                        5,
                        Env,
                        &[
                            ("attack", 0.001),
                            ("decay", 0.08),
                            ("sustain", 0.3),
                            ("release", 0.05),
                        ],
                    ),
                    m(6, Slew, &[("rise", 0.2), ("fall", 0.6)]),
                    m(7, Osc, &[("voices", 2.0), ("detune", 10.0)]),
                    m(8, Filter, &[("cutoff", 600.0), ("res", 0.5)]),
                    m(9, Vca, &[]),
                    m(
                        10,
                        Env,
                        &[("attack", 0.001), ("sustain", 1.0), ("release", 0.1)],
                    ),
                    m(11, Vca, &[]),
                    m(12, Output, &[]),
                ],
                vec![
                    c(2, "out", 4, "a", 1.0),
                    c(3, "out", 4, "b", 0.8),
                    c(4, "out", 5, "gate", 1.0),
                    c(4, "out", 6, "in", 1.0),
                    c(6, "out", 8, "cutoff", 0.3),
                    c(1, "pitch", 7, "pitch", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "out", 9, "in", 1.0),
                    c(5, "out", 9, "cv", 1.0),
                    c(1, "gate", 10, "gate", 1.0),
                    c(9, "out", 11, "in", 1.0),
                    c(10, "out", 11, "cv", 1.0),
                    c(11, "out", 12, "left", 1.0),
                ],
            ),
        ),
        (
            "Macro lab",
            built(
                8,
                vec![
                    m(1, Keys, &[]),
                    m(
                        2,
                        Macro,
                        &[("knob_1", 0.5), ("knob_2", 0.3), ("knob_3", 0.4)],
                    ),
                    m(3, Osc, &[("voices", 3.0), ("detune", 15.0)]),
                    m(4, Ladder, &[("cutoff", 300.0), ("res", 0.4)]),
                    m(5, Shaper, &[("drive", 1.0), ("mix", 0.7)]),
                    m(6, Env, &[("sustain", 0.8), ("release", 0.3)]),
                    m(7, Vca, &[]),
                    m(8, Delay, &[("time", 0.3), ("mix", 0.0)]),
                    m(9, Output, &[("level", 0.6)]),
                ],
                vec![
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(3, "out", 4, "in", 1.0),
                    c(2, "out_1", 4, "cutoff", 0.5),
                    c(2, "out_2", 5, "drive", 0.6),
                    c(2, "out_3", 8, "mix", 0.8),
                    c(2, "out_4", 4, "res", 0.5),
                    c(4, "out", 5, "in", 1.0),
                    c(1, "gate", 6, "gate", 1.0),
                    c(5, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "in", 1.0),
                    c(8, "out", 9, "left", 1.0),
                ],
            ),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The compiled graph
// ---------------------------------------------------------------------------

/// A module ready to run: where its inputs come from and where its outputs go.
struct Compiled {
    kind: Kind,
    /// The sample a sampler plays.
    sample: Option<SampleBuf>,
    /// For each input (plain inputs, then parameters): (output slot, amount).
    sources: Vec<Vec<(usize, f32)>>,
    /// The parameters' own values, from 0 to 1.
    base: Vec<f64>,
    /// The same in their units, so a parameter with no cable costs nothing to read.
    value: Vec<f64>,
    /// How many plain inputs and outputs it has.
    inputs: usize,
    outputs: usize,
    /// The first of this module's output slots.
    out: usize,
}

struct Graph {
    /// The modules in the order they run.
    nodes: Vec<Compiled>,
    /// The node ids, in the same order.
    ids: Vec<u32>,
    slots: usize,
    /// Where the sound is taken from: the `Output` node's place in `nodes`.
    output: Option<usize>,
    /// The patch's own glide (mono only), in seconds.
    glide: f64,
    volume: f32,
}

impl Graph {
    fn compile(patch: &Patch, samples: &[(String, SampleBuf)]) -> Self {
        // the order: a module runs after the ones that feed it; what is left in a loop
        // runs in id order and reads its loop inputs from the sample before
        let n = patch.nodes.len();
        let index = |id: u32| patch.nodes.iter().position(|x| x.id == id);
        let valid: Vec<&Cable> = patch.cables.iter().filter(|c| patch.is_valid(c)).collect();
        let mut indeg = vec![0usize; n];
        for c in &valid {
            if let (Some(_), Some(b)) = (index(c.from), index(c.to)) {
                indeg[b] += 1;
            }
        }
        let mut order = Vec::with_capacity(n);
        let mut done = vec![false; n];
        loop {
            let next = (0..n).find(|&k| !done[k] && indeg[k] == 0).or_else(|| {
                // a loop: take the first module left
                (0..n).find(|&k| !done[k])
            });
            let Some(k) = next else { break };
            done[k] = true;
            order.push(k);
            for c in &valid {
                if index(c.from) == Some(k) {
                    if let Some(b) = index(c.to) {
                        indeg[b] = indeg[b].saturating_sub(1);
                    }
                }
            }
        }
        // output slots, in the order of the patch
        let mut first = vec![0usize; n];
        let mut slots = 0;
        for (k, node) in patch.nodes.iter().enumerate() {
            first[k] = slots;
            slots += node.kind.spec().outputs.len();
        }
        let mut nodes = Vec::with_capacity(n);
        let mut ids = Vec::with_capacity(n);
        let mut output = None;
        for &k in &order {
            let node = &patch.nodes[k];
            let spec = node.kind.spec();
            let mut sources = vec![Vec::new(); spec.inputs.len() + spec.params.len()];
            for c in valid.iter().filter(|c| c.to == node.id) {
                let (Some(from), Some(o), Some(inp)) = (
                    index(c.from),
                    patch.nodes[index(c.from).unwrap_or(0)]
                        .kind
                        .output_index(&c.output),
                    node.kind.input_index(&c.input),
                ) else {
                    continue;
                };
                sources[inp].push((first[from] + o, c.amount as f32));
            }
            let base: Vec<f64> = spec
                .params
                .iter()
                .map(|p| p.normalize(node.param(p.name)))
                .collect();
            let value = spec
                .params
                .iter()
                .zip(&base)
                .map(|(p, &b)| p.denormalize(b))
                .collect();
            if node.kind == Kind::Output && output.is_none() {
                output = Some(nodes.len());
            }
            let sample = node
                .sample
                .as_ref()
                .filter(|_| node.kind == Kind::Sampler)
                .and_then(|id| samples.iter().find(|(s, _)| s == id))
                .map(|(_, b)| b.clone());
            nodes.push(Compiled {
                kind: node.kind,
                sample,
                sources,
                base,
                value,
                inputs: spec.inputs.len(),
                outputs: spec.outputs.len(),
                out: first[k],
            });
            ids.push(node.id);
        }
        Self {
            nodes,
            ids,
            slots,
            output,
            glide: if patch.mono {
                patch.glide.clamp(0.0, MAX_GLIDE)
            } else {
                0.0
            },
            volume: patch.volume.clamp(0.0, 2.0) as f32,
        }
    }
}

// ---------------------------------------------------------------------------
// Voices
// ---------------------------------------------------------------------------

/// What a module keeps from one sample to the next.
#[derive(Clone, Copy)]
struct NodeState {
    /// Phases: one for each unison voice of an oscillator.
    ph: [f64; 8],
    /// What else the module keeps: filter integrators, held values and so on.
    v: [f64; 8],
    /// Envelope stage: 0 idle, 1 attack, 2 decay/sustain, 3 release. For other
    /// modules, 0 until the first sample of the voice.
    stage: u8,
    gate: bool,
    rng: u32,
    /// Samples played, where a delay line writes.
    n: usize,
    /// Costly values kept until what they come from changes.
    memo: [[f64; 2]; 3],
    /// The frequency ratios of an oscillator's unison voices, for `uni_key`.
    uni: [f64; 8],
    uni_key: f64,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            ph: [0.0; 8],
            v: [0.0; 8],
            stage: 0,
            gate: false,
            rng: 0,
            n: 0,
            // no key matches NaN, so every memo is worked out the first time
            memo: [[f64::NAN, 0.0]; 3],
            uni: [1.0; 8],
            uni_key: f64::NAN,
        }
    }
}

impl NodeState {
    /// A random number from -1 to 1 (xorshift).
    fn rand(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        f64::from(x) / f64::from(u32::MAX) * 2.0 - 1.0
    }
}

struct Voice {
    outs: Vec<f32>,
    states: Vec<NodeState>,
    /// The delay lines of the modules that have them (empty for the rest).
    lines: Vec<Vec<f32>>,
    key: u8,
    vel: f32,
    gate: bool,
    /// The voice was just (re)started: its gate drops for one sample and modules
    /// that follow notes start again.
    trig: bool,
    active: bool,
    /// The pitch now, in octaves from middle C (it slides when there is glide).
    pitch: f64,
    /// When the voice started, to steal the oldest.
    age: u64,
    /// Samples in a row it has been quiet since its key was let go.
    quiet: usize,
}

/// The length of a module's delay line at a sample rate.
fn line_len(kind: Kind, sr: f64) -> usize {
    match kind {
        Kind::Delay => (DELAY.params[0].max * sr) as usize + 4,
        Kind::Chorus => (0.05 * sr) as usize + 4,
        Kind::Reverb => reverb_lines(sr).iter().sum(),
        _ => 0,
    }
}

/// The lengths of a reverb's lines: four combs a side, then two allpasses a side.
fn reverb_lines(sr: f64) -> [usize; 12] {
    const AT_44K: [usize; 12] = [
        1116, 1188, 1277, 1356, 1139, 1211, 1300, 1379, 556, 441, 579, 464,
    ];
    let k = sr / 44_100.0;
    AT_44K.map(|n| ((n as f64 * k) as usize).max(1))
}

impl Voice {
    fn new(graph: &Graph, seed: u32, sr: f64) -> Self {
        let mut states = vec![NodeState::default(); graph.nodes.len()];
        for (k, s) in states.iter_mut().enumerate() {
            s.rng = seed
                .wrapping_mul(2_654_435_761)
                .wrapping_add(k as u32 * 97 + 1)
                | 1;
        }
        Self {
            outs: vec![0.0; graph.slots],
            states,
            lines: graph
                .nodes
                .iter()
                .map(|c| vec![0.0; line_len(c.kind, sr)])
                .collect(),
            key: 0,
            vel: 0.0,
            gate: false,
            trig: false,
            active: false,
            pitch: 0.0,
            age: 0,
            quiet: 0,
        }
    }

    /// Start from silence.
    fn clear(&mut self) {
        self.outs.fill(0.0);
        for s in &mut self.states {
            let rng = s.rng;
            *s = NodeState {
                rng,
                ..NodeState::default()
            };
        }
        for l in &mut self.lines {
            l.fill(0.0);
        }
    }
}

/// The value of a polyBLEP step correction at `t` (phase, 0 to 1) for a step every
/// `dt` of phase.
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

fn wave(shape: u32, phase: f64, dt: f64, pw: f64) -> f64 {
    match shape {
        0 => (2.0 * PI * phase).sin(),
        1 => 1.0 - 4.0 * (phase - 0.5).abs(),
        2 => 2.0 * phase - 1.0 - blep(phase, dt),
        _ => {
            let mut v = if phase < pw { 1.0 } else { -1.0 };
            v += blep(phase, dt);
            v -= blep((phase + 1.0 - pw) % 1.0, dt);
            v
        }
    }
}

/// An oscillator's wave: one of the basic waves, or (shape 4) a blend along sine,
/// triangle, saw and square by `morph` from 0 to 3.
fn osc_wave(shape: u32, morph: f64, phase: f64, dt: f64, pw: f64) -> f64 {
    if shape < 4 {
        return wave(shape, phase, dt, pw);
    }
    let k = (morph.floor() as u32).min(2);
    let t = morph - f64::from(k);
    let a = wave(k, phase, dt, pw);
    if t <= 0.0 {
        a
    } else {
        a + (wave(k + 1, phase, dt, pw) - a) * t
    }
}

/// A triangle fold that leaves -1 to 1 as it is and folds back what goes past.
fn fold(u: f64) -> f64 {
    4.0 * (((u - 1.0) / 4.0).rem_euclid(1.0) - 0.5).abs() - 1.0
}

/// Read a delay line `d` samples (at least 1) behind where it writes next.
#[inline]
fn tap(line: &[f32], at: usize, d: f64) -> f64 {
    let len = line.len();
    let pos = (at as f64 - d).rem_euclid(len as f64);
    let i0 = pos as usize % len;
    let i1 = (i0 + 1) % len;
    let t = pos - pos.floor();
    let (a, b) = (f64::from(line[i0]), f64::from(line[i1]));
    a + (b - a) * t
}

/// Bend an envelope's level (0 to 1) by a curve: above 0 it starts slow, below it
/// starts fast.
fn curve(a: f64, c: f64) -> f64 {
    if c > 1e-3 {
        a.powf(1.0 + 3.0 * c)
    } else if c < -1e-3 {
        1.0 - (1.0 - a).powf(1.0 - 3.0 * c)
    } else {
        a
    }
}

/// The nearest note of a scale to a pitch in octaves.
fn quantize(octaves: f64, scale: usize) -> f64 {
    let bits = SCALE_BITS[scale.min(SCALE_BITS.len() - 1)];
    let semis = octaves * 12.0;
    let near = semis.round() as i64;
    let mut best = near as f64;
    let mut dist = f64::MAX;
    for d in -6..=6 {
        let n = near + d;
        if bits & (1 << n.rem_euclid(12)) != 0 {
            let e = (n as f64 - semis).abs();
            if e < dist {
                dist = e;
                best = n as f64;
            }
        }
    }
    best / 12.0
}

/// Middle C, the pitch 0 of `Keys`.
const C4: f64 = 261.625_565_300_598_6;
/// How long a released voice must be quiet before it is free, in seconds.
const QUIET_TIME: f64 = 0.05;

/// The performance controls a `Keys` module puts out: the mod wheel (0 to 1), pitch
/// bend (-1 to 1) and aftertouch (0 to 1).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Controls {
    pub wheel: f32,
    pub bend: f32,
    pub pressure: f32,
}

/// How many voices a patch lets play: 1 in mono, else its `voices` (1 to
/// [`MAX_VOICES`]).
fn poly_of(p: &Patch) -> usize {
    if p.mono {
        1
    } else {
        p.voices.clamp(1, MAX_VOICES)
    }
}

/// The modular synth.
pub struct Modular {
    patch: Patch,
    graph: Graph,
    /// Always [`MAX_VOICES`] of them, made up front, so the number that plays can
    /// change at any time (live, while keys are held) without making any.
    voices: Vec<Voice>,
    /// How many of `voices` new notes may take (1 in mono).
    poly: usize,
    sr: f64,
    clock: u64,
    controls: Controls,
    /// The keys held in mono, the newest last.
    held: Vec<u8>,
    /// The pitch of the last note, where a new voice glides from.
    last_pitch: Option<f64>,
    /// The samples the patch's samplers play, by sample id.
    samples: Vec<(String, SampleBuf)>,
}

impl Modular {
    /// A synth playing a patch; a single input with several cables plays only the first.
    pub fn new(patch: &Patch, sr: f64) -> Self {
        Self::with_samples(patch, sr, Vec::new())
    }

    /// [`Self::new`] with the samples its samplers play (by sample id).
    pub fn with_samples(patch: &Patch, sr: f64, samples: Vec<(String, SampleBuf)>) -> Self {
        let mut patch = patch.clone();
        patch.one_cable_per_input();
        let patch = &patch;
        let graph = Graph::compile(patch, &samples);
        let voices = (0..MAX_VOICES)
            .map(|v| Voice::new(&graph, v as u32, sr))
            .collect();
        Self {
            patch: patch.clone(),
            graph,
            voices,
            poly: poly_of(patch),
            sr,
            clock: 0,
            controls: Controls::default(),
            held: Vec::with_capacity(128),
            last_pitch: None,
            samples,
        }
    }

    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Take a new patch. A patch of the same shape (see [`Patch::same_shape`]) keeps
    /// the voices playing; one with other modules or cables starts them again.
    pub fn set_patch(&mut self, patch: &Patch) {
        if self.patch.same_shape(patch) {
            drop(self.apply_live(Box::new(patch.clone())));
        } else {
            let controls = self.controls;
            let samples = std::mem::take(&mut self.samples);
            *self = Self::with_samples(patch, self.sr, samples);
            self.controls = controls;
        }
    }

    /// Take a patch of the same shape as the one playing, while it plays: parameters,
    /// cable amounts, voices, mono, legato, glide and volume move at once and the
    /// voices sounding carry on. This neither allocates nor frees, so the audio thread
    /// can call it: the patch it replaces comes back, to be dropped elsewhere. A patch
    /// of another shape is not taken and comes back as it was.
    pub fn apply_live(&mut self, mut patch: Box<Patch>) -> Box<Patch> {
        if !self.patch.same_shape(&patch) {
            return patch;
        }
        for (k, c) in self.graph.nodes.iter_mut().enumerate() {
            let Some(node) = self.graph.ids.get(k).and_then(|&id| patch.node(id)) else {
                continue;
            };
            let spec = c.kind.spec();
            for (pi, ps) in spec.params.iter().enumerate() {
                if let (Some(b), Some(v)) = (c.base.get_mut(pi), c.value.get_mut(pi)) {
                    *b = ps.normalize(node.param(ps.name));
                    *v = ps.denormalize(*b);
                }
            }
            // the cables into this module, in the order compile put them in
            for (k_in, sources) in c.sources.iter_mut().enumerate() {
                let into = patch.cables.iter().filter(|cb| {
                    cb.to == node.id
                        && patch.is_valid(cb)
                        && node.kind.input_index(&cb.input) == Some(k_in)
                });
                for (src, cb) in sources.iter_mut().zip(into) {
                    src.1 = cb.amount as f32;
                }
            }
        }
        self.graph.glide = if patch.mono {
            patch.glide.clamp(0.0, MAX_GLIDE)
        } else {
            0.0
        };
        self.graph.volume = patch.volume.clamp(0.0, 2.0) as f32;
        let was_mono = self.patch.mono;
        std::mem::swap(&mut self.patch, &mut patch);
        self.set_poly(poly_of(&self.patch), was_mono != self.patch.mono);
        patch
    }

    /// Let `poly` voices take notes. Voices past it let go of their keys and ring out;
    /// a switch between mono and poly lets go of every key.
    fn set_poly(&mut self, poly: usize, switched: bool) {
        self.poly = poly.clamp(1, MAX_VOICES);
        if switched {
            self.held.clear();
            for v in &mut self.voices {
                v.gate = false;
            }
            return;
        }
        for v in self.voices.iter_mut().skip(self.poly) {
            v.gate = false;
        }
    }

    /// How many voices may play at once now.
    pub fn polyphony(&self) -> usize {
        self.poly
    }

    /// Move one parameter of a module, in its own units.
    pub fn set_param(&mut self, node: u32, name: &str, value: f64) {
        let Some(k) = self.graph.ids.iter().position(|&i| i == node) else {
            return;
        };
        let c = &mut self.graph.nodes[k];
        let spec = c.kind.spec();
        if let Some(pi) = spec.params.iter().position(|p| p.name == name) {
            c.base[pi] = spec.params[pi].normalize(value);
            c.value[pi] = spec.params[pi].denormalize(c.base[pi]);
        }
        if let Some(n) = self.patch.nodes.iter_mut().find(|n| n.id == node) {
            // no new string for a parameter the patch already names (automation moves
            // parameters on the audio thread)
            match n.params.get_mut(name) {
                Some(v) => *v = value,
                None => {
                    n.params.insert(name.to_string(), value);
                }
            }
        }
    }

    /// Set the mod wheel, pitch bend and aftertouch that `Keys` puts out.
    pub fn set_controls(&mut self, c: Controls) {
        self.controls = c;
    }

    pub fn note_on(&mut self, key: u8, vel: f32) {
        self.clock += 1;
        let vel = vel.clamp(0.0, 1.0);
        if self.patch.mono {
            self.held.retain(|&k| k != key);
            if self.held.len() < 128 {
                self.held.push(key);
            }
            let legato = self.patch.legato;
            let last = self.last_pitch;
            let Some(v) = self.voices.first_mut() else {
                return;
            };
            let slide = legato && v.active && v.gate;
            if !v.active {
                v.clear();
                v.pitch = last.unwrap_or((f64::from(key) - 60.0) / 12.0);
            }
            v.key = key;
            v.vel = vel;
            v.trig = !slide;
            v.gate = true;
            v.active = true;
            v.age = self.clock;
            v.quiet = 0;
            self.last_pitch = Some((f64::from(key) - 60.0) / 12.0);
            return;
        }
        // the same key again, a free voice, or one to take over (of the first `poly`)
        let poly = self.poly.clamp(1, self.voices.len().max(1));
        let pool = &self.voices[..poly.min(self.voices.len())];
        let v = pool
            .iter()
            .position(|v| v.active && v.key == key)
            .or_else(|| pool.iter().position(|v| !v.active))
            // then the oldest of the ones ringing out, then the oldest held
            .or_else(|| {
                pool.iter()
                    .enumerate()
                    .filter(|(_, v)| !v.gate)
                    .min_by_key(|(_, v)| v.age)
                    .map(|(i, _)| i)
            })
            .unwrap_or_else(|| {
                pool.iter()
                    .enumerate()
                    .min_by_key(|(_, v)| v.age)
                    .map_or(0, |(i, _)| i)
            });
        let last = self.last_pitch;
        // a voice past `poly` (it was ringing out) that has the key again stops
        for o in self.voices.iter_mut().skip(poly) {
            if o.active && o.key == key {
                o.gate = false;
            }
        }
        let Some(voice) = self.voices.get_mut(v) else {
            return;
        };
        if !voice.active {
            voice.clear();
            voice.pitch = last.unwrap_or((f64::from(key) - 60.0) / 12.0);
        }
        voice.key = key;
        voice.vel = vel;
        voice.gate = true;
        voice.trig = true;
        voice.active = true;
        voice.age = self.clock;
        voice.quiet = 0;
        self.last_pitch = Some((f64::from(key) - 60.0) / 12.0);
    }

    pub fn note_off(&mut self, key: u8) {
        if self.patch.mono {
            self.held.retain(|&k| k != key);
            let legato = self.patch.legato;
            let Some(v) = self.voices.first_mut() else {
                return;
            };
            if v.active && v.key == key {
                match self.held.last() {
                    // back to the key still held
                    Some(&k) => {
                        v.key = k;
                        v.trig = !legato;
                    }
                    None => v.gate = false,
                }
            }
            return;
        }
        for v in self.voices.iter_mut().filter(|v| v.active && v.key == key) {
            v.gate = false;
        }
    }

    /// Let go of every key.
    pub fn release_all(&mut self) {
        self.held.clear();
        for v in &mut self.voices {
            v.gate = false;
        }
    }

    /// Stop every voice at once.
    pub fn stop(&mut self) {
        self.held.clear();
        for v in &mut self.voices {
            v.active = false;
            v.gate = false;
        }
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Play into `l` and `r` (what was in them is replaced).
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
        let Some(out_at) = self.graph.output else {
            return;
        };
        let quiet_after = (QUIET_TIME * self.sr) as usize;
        let ctx = Ctx {
            sr: self.sr,
            out_at,
            controls: self.controls,
        };
        let graph = &self.graph;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            for (ol, or) in l.iter_mut().zip(r.iter_mut()) {
                let (a, b) = step(graph, v, &ctx);
                *ol += a;
                *or += b;
                if !v.gate {
                    if a.abs() + b.abs() < 1e-5 {
                        v.quiet += 1;
                    } else {
                        v.quiet = 0;
                    }
                    if v.quiet > quiet_after {
                        v.active = false;
                        break;
                    }
                }
            }
        }
    }
}

/// What every voice's step needs to know.
struct Ctx {
    sr: f64,
    /// The `Output` module's place.
    out_at: usize,
    controls: Controls,
}

/// The value of input `k` of a module for this sample.
#[inline]
fn input(c: &Compiled, outs: &[f32], k: usize, unplugged: f32) -> f32 {
    let s = &c.sources[k];
    if s.is_empty() {
        return unplugged;
    }
    s.iter().map(|&(slot, amt)| outs[slot] * amt).sum()
}

/// A parameter of a module for this sample, with what its cables add, in its units.
#[inline]
fn param(c: &Compiled, outs: &[f32], p: usize) -> f64 {
    let k = c.inputs + p;
    if c.sources[k].is_empty() {
        return c.value[p];
    }
    let m = f64::from(input(c, outs, k, 0.0));
    c.kind.spec().params[p].denormalize(c.base[p] + m)
}

/// `f(x)`, worked out again only when `x` changes.
#[inline]
fn memo(slot: &mut [f64; 2], x: f64, f: impl FnOnce(f64) -> f64) -> f64 {
    if slot[0] != x {
        *slot = [x, f(x)];
    }
    slot[1]
}

/// Whether input `k` has a cable.
#[inline]
fn plugged(c: &Compiled, k: usize) -> bool {
    !c.sources[k].is_empty()
}

/// A one pole smoothing coefficient that gets within 1% in `t` seconds.
#[inline]
fn coef(t: f64, sr: f64) -> f64 {
    (-4.6 / (t * sr).max(1.0)).exp()
}

/// One sample of a sampler module: (left, right). Its state keeps the place it reads
/// (`v[0]`, in frames), whether it plays (`v[1]`) and the gate before (`v[2]`).
fn sampler_step(
    c: &Compiled,
    s: &mut NodeState,
    outs: &[f32],
    key_pitch: f64,
    key_gate: bool,
    trig: bool,
    sr: f64,
) -> (f32, f32) {
    let Some(b) = &c.sample else {
        return (0.0, 0.0);
    };
    let n = b.len();
    if n < 2 {
        return (0.0, 0.0);
    }
    let start = param(c, outs, 0).clamp(0.0, 1.0);
    let end = param(c, outs, 1).clamp(0.0, 1.0);
    let (lo, hi) = (start.min(end), start.max(end));
    let looped = param(c, outs, 2) > 0.5;
    let follow = param(c, outs, 3) > 0.5;
    let root = param(c, outs, 4);
    let gated = param(c, outs, 5) > 0.5;
    let reverse = param(c, outs, 6) > 0.5;
    let gain = param(c, outs, 7) as f32;
    let last = (n - 1) as f64;
    let (a, z) = (lo * last, (hi * last).max(lo * last + 1.0));
    let gate = if plugged(c, 1) {
        input(c, outs, 1, 0.0) > 0.0
    } else {
        key_gate
    };
    let rising = (gate && s.v[2] <= 0.0) || (trig && !plugged(c, 1));
    s.v[2] = if gate { 1.0 } else { 0.0 };
    s.stage = 1;
    if rising {
        s.v[0] = if reverse { z } else { a };
        s.v[1] = 1.0;
    } else if gated && !gate {
        s.v[1] = 0.0;
    }
    if s.v[1] <= 0.0 {
        return (0.0, 0.0);
    }
    let pitch = if plugged(c, 0) {
        f64::from(input(c, outs, 0, 0.0))
    } else {
        key_pitch
    };
    let semis = if follow {
        pitch * 12.0 + 60.0 - root
    } else {
        0.0
    };
    let rate = b.rate / sr * memo(&mut s.memo[0], semis, |x| (x / 12.0).exp2());
    // the position input moves where it reads by up to the whole length
    let shift = f64::from(input(c, outs, 2, 0.0)) * (z - a);
    let at = (s.v[0] + shift).clamp(0.0, last);
    let i = at.floor() as usize;
    let f = (at - i as f64) as f32;
    let j = (i + 1).min(n - 1);
    let l = b.left[i] + (b.left[j] - b.left[i]) * f;
    let r = b.right[i] + (b.right[j] - b.right[i]) * f;
    let step = if reverse { -rate } else { rate };
    s.v[0] += step;
    if s.v[0] > z || s.v[0] < a {
        if looped {
            let len = z - a;
            s.v[0] = a + (s.v[0] - a).rem_euclid(len);
        } else {
            s.v[1] = 0.0;
        }
    }
    (l * gain, r * gain)
}

/// One sample of a voice: (left, right).
fn step(graph: &Graph, v: &mut Voice, ctx: &Ctx) -> (f32, f32) {
    let sr = ctx.sr;
    let trig = v.trig;
    let mut result = (0.0, 0.0);
    for (k, c) in graph.nodes.iter().enumerate() {
        let s = &mut v.states[k];
        let outs = &mut v.outs;
        let line = &mut v.lines[k];
        let fresh = s.stage == 0;
        match c.kind {
            Kind::Keys => {
                let target = (f64::from(v.key) - 60.0) / 12.0;
                let glide = param(c, outs, 0).max(graph.glide);
                if glide > 0.0 && !fresh {
                    v.pitch =
                        target + (v.pitch - target) * memo(&mut s.memo[0], glide, |t| coef(t, sr));
                } else if glide <= 0.0 {
                    v.pitch = target;
                }
                s.stage = 1;
                let ct = ctx.controls;
                let bend = f64::from(ct.bend) * param(c, outs, 1) / 12.0;
                outs[c.out] = (v.pitch + bend) as f32;
                outs[c.out + 1] = if v.gate && !trig { 1.0 } else { 0.0 };
                outs[c.out + 2] = v.vel;
                outs[c.out + 3] = ct.wheel;
                outs[c.out + 4] = ct.bend;
                outs[c.out + 5] = ct.pressure;
            }
            Kind::Osc => {
                let pitch = f64::from(input(c, outs, 0, 0.0) + input(c, outs, 3, 0.0));
                let fm = f64::from(input(c, outs, 1, 0.0));
                let sync = f64::from(input(c, outs, 2, 0.0));
                let shape = param(c, outs, 0) as u32;
                let tune = param(c, outs, 1);
                let fine = param(c, outs, 2);
                let pw = param(c, outs, 3);
                let level = param(c, outs, 4);
                let morph = param(c, outs, 5);
                let count = (param(c, outs, 6) as usize).clamp(1, 8);
                let detune = param(c, outs, 7);
                let spread = param(c, outs, 8);
                let phase = param(c, outs, 9);
                let reset = param(c, outs, 10) > 0.5;
                // hard sync: a rising edge on `sync` starts the wave again
                let synced = plugged(c, 2) && sync > 0.0 && s.v[0] <= 0.0;
                s.v[0] = sync;
                if (trig && reset) || synced {
                    s.ph = [phase; 8];
                } else if fresh && !reset {
                    let ph = std::array::from_fn(|_| 0.5 + 0.5 * s.rand());
                    s.ph = ph;
                }
                s.stage = 1;
                let lim = sr * 0.45;
                let f0 =
                    C4 * memo(
                        &mut s.memo[0],
                        pitch + tune / 12.0 + fine / 1200.0,
                        f64::exp2,
                    ) * (1.0 + fm);
                if count == 1 {
                    let f = f0.clamp(-lim, lim);
                    let dt = (f / sr).abs().max(1e-9);
                    let y = (osc_wave(shape, morph, s.ph[0], dt, pw) * level) as f32;
                    outs[c.out] = y;
                    outs[c.out + 1] = y;
                    s.ph[0] = (s.ph[0] + f / sr).rem_euclid(1.0);
                } else {
                    let (mut l, mut r) = (0.0, 0.0);
                    let last = (count - 1) as f64;
                    let key = detune + 1000.0 * count as f64;
                    if s.uni_key != key {
                        s.uni_key = key;
                        for j in 0..count {
                            let at = 2.0 * j as f64 / last - 1.0;
                            s.uni[j] = (detune * at / 1200.0).exp2();
                        }
                    }
                    for j in 0..count {
                        let at = 2.0 * j as f64 / last - 1.0;
                        let f = (f0 * s.uni[j]).clamp(-lim, lim);
                        let dt = (f / sr).abs().max(1e-9);
                        let y = osc_wave(shape, morph, s.ph[j], dt, pw);
                        let pan = spread * at;
                        l += y * (1.0 - pan).min(1.0);
                        r += y * (1.0 + pan).min(1.0);
                        s.ph[j] = (s.ph[j] + f / sr).rem_euclid(1.0);
                    }
                    let g = level / (count as f64).sqrt();
                    outs[c.out] = (l * g) as f32;
                    outs[c.out + 1] = (r * g) as f32;
                }
            }
            Kind::Fm => {
                let pitch = f64::from(input(c, outs, 0, 0.0));
                let pm = f64::from(input(c, outs, 1, 0.0));
                let ratio = param(c, outs, 0);
                let tune = param(c, outs, 1);
                let fb = param(c, outs, 2);
                let level = param(c, outs, 3);
                if trig {
                    s.ph[0] = 0.0;
                }
                s.stage = 1;
                let f = (C4 * ratio * memo(&mut s.memo[0], pitch + tune / 12.0, f64::exp2))
                    .min(sr * 0.45);
                // feedback from the mean of the last two samples, which keeps it calm
                let back = fb * 1.5 * (s.v[0] + s.v[1]) * 0.5;
                let y = (2.0 * PI * (s.ph[0] + pm) + back).sin();
                s.v[1] = s.v[0];
                s.v[0] = y;
                s.ph[0] = (s.ph[0] + f / sr).rem_euclid(1.0);
                outs[c.out] = (y * level) as f32;
            }
            Kind::Noise => {
                let level = param(c, outs, 0);
                outs[c.out] = (s.rand() * level) as f32;
            }
            Kind::Macro => {
                for q in 0..4 {
                    outs[c.out + q] = param(c, outs, q) as f32;
                }
            }
            Kind::Lfo => {
                let rate = param(c, outs, 0);
                let shape = param(c, outs, 1) as u32;
                let depth = param(c, outs, 2);
                let phase = param(c, outs, 3);
                let on_note = param(c, outs, 4) > 0.5;
                if (trig && on_note) || (fresh && on_note) {
                    s.ph[0] = phase;
                } else if fresh {
                    s.ph[0] = s.rand() * 0.5 + 0.5;
                }
                if fresh || (trig && shape == 4) {
                    s.v[0] = s.rand();
                }
                s.stage = 1;
                // no correction: an LFO's steps are meant to be steps
                let w = if shape == 4 {
                    s.v[0]
                } else {
                    wave(shape, s.ph[0], 0.0, 0.5)
                } * depth;
                outs[c.out] = w as f32;
                outs[c.out + 1] = ((w + depth) * 0.5) as f32;
                let next = s.ph[0] + rate / sr;
                if next >= 1.0 {
                    s.v[0] = s.rand();
                }
                s.ph[0] = next.rem_euclid(1.0);
            }
            Kind::Env => {
                let gate = input(c, outs, 0, 0.0) > 0.5;
                let (att, dec, sus, rel) = (
                    param(c, outs, 0),
                    param(c, outs, 1),
                    param(c, outs, 2),
                    param(c, outs, 3),
                );
                let bend = param(c, outs, 4);
                let looped = param(c, outs, 5) > 0.5;
                if gate && !s.gate {
                    s.stage = 1;
                } else if !gate && s.gate {
                    s.stage = 3;
                }
                s.gate = gate;
                // exponential moves that get within 1% of where they go in their time
                match s.stage {
                    1 => {
                        s.v[0] += 1.0 / (att * sr).max(1.0);
                        if s.v[0] >= 1.0 {
                            s.v[0] = 1.0;
                            s.stage = 2;
                        }
                    }
                    2 => {
                        s.v[0] = sus + (s.v[0] - sus) * memo(&mut s.memo[0], dec, |t| coef(t, sr));
                        // a looping envelope starts again once it has decayed
                        if looped && (s.v[0] - sus).abs() < 0.01 {
                            s.stage = 1;
                        }
                    }
                    3 => {
                        s.v[0] *= memo(&mut s.memo[1], rel, |t| coef(t, sr));
                        if s.v[0] < 1e-6 {
                            s.v[0] = 0.0;
                            s.stage = 0;
                        }
                    }
                    _ => s.v[0] = 0.0,
                }
                outs[c.out] = curve(s.v[0], bend) as f32;
            }
            Kind::Hold => {
                let rate = param(c, outs, 0);
                let take = if plugged(c, 1) {
                    let t = f64::from(input(c, outs, 1, 0.0));
                    let rise = t > 0.5 && s.v[1] <= 0.5;
                    s.v[1] = t;
                    rise
                } else {
                    s.ph[0] += rate / sr;
                    let wrap = s.ph[0] >= 1.0;
                    s.ph[0] = s.ph[0].rem_euclid(1.0);
                    wrap
                };
                if take || fresh {
                    // nothing plugged into `in` takes noise
                    s.v[0] = if plugged(c, 0) {
                        f64::from(input(c, outs, 0, 0.0))
                    } else {
                        s.rand()
                    };
                }
                s.stage = 1;
                outs[c.out] = s.v[0] as f32;
            }
            Kind::Seq => {
                // v: 0 the step, 1 the clock before, 2 samples since a clock,
                // 3 the clock's period, 4 the reset before, 5 clocked yet
                let rate = param(c, outs, 0);
                let length = (param(c, outs, 1) as usize).clamp(1, 8);
                let gate_len = param(c, outs, 2);
                let reset_in = f64::from(input(c, outs, 1, 0.0));
                if trig || (reset_in > 0.5 && s.v[4] <= 0.5) {
                    s.v[0] = 0.0;
                    s.v[5] = 0.0;
                    s.ph[0] = 0.0;
                }
                s.v[4] = reset_in;
                let advance = |s: &mut NodeState| {
                    s.v[0] = ((s.v[0] as usize + 1) % length) as f64;
                };
                let open = if plugged(c, 0) {
                    let clk = f64::from(input(c, outs, 0, 0.0));
                    if clk > 0.5 && s.v[1] <= 0.5 {
                        // the first clock plays the first step
                        if s.v[5] > 0.5 {
                            advance(s);
                        }
                        s.v[5] = 1.0;
                        s.v[3] = s.v[2];
                        s.v[2] = 0.0;
                    }
                    s.v[1] = clk;
                    s.v[2] += 1.0;
                    s.v[5] > 0.5 && s.v[2] < gate_len * s.v[3].max(1.0)
                } else {
                    let open = s.ph[0] < gate_len;
                    s.ph[0] += rate / sr;
                    if s.ph[0] >= 1.0 {
                        s.ph[0] -= 1.0;
                        advance(s);
                    }
                    open
                };
                if s.v[0] as usize >= length {
                    s.v[0] = 0.0;
                }
                s.stage = 1;
                outs[c.out] = param(c, outs, 3 + s.v[0] as usize) as f32;
                outs[c.out + 1] = if open { 1.0 } else { 0.0 };
            }
            Kind::Slew => {
                let x = f64::from(input(c, outs, 0, 0.0));
                if fresh {
                    s.v[0] = x;
                }
                s.stage = 1;
                let t = if x > s.v[0] {
                    param(c, outs, 0)
                } else {
                    param(c, outs, 1)
                };
                let k = if x > s.v[0] { 0 } else { 1 };
                s.v[0] = x + (s.v[0] - x) * memo(&mut s.memo[k], t, |t| coef(t, sr));
                outs[c.out] = s.v[0] as f32;
            }
            Kind::Filter => {
                let x = f64::from(input(c, outs, 0, 0.0));
                let mode = param(c, outs, 0) as u32;
                let cutoff = param(c, outs, 1).min(sr * 0.45);
                let res = param(c, outs, 2);
                // a state variable filter in the topology preserving form
                let g = memo(&mut s.memo[0], cutoff, |f| (PI * f / sr).tan());
                let kq = 2.0 - 1.96 * res;
                let a1 = 1.0 / (1.0 + g * (g + kq));
                let v1 = a1 * (s.v[0] + g * (x - s.v[1]));
                let v2 = s.v[1] + g * v1;
                s.v[0] = 2.0 * v1 - s.v[0];
                s.v[1] = 2.0 * v2 - s.v[1];
                let y = match mode {
                    0 => v2,
                    1 => v1,
                    _ => x - kq * v1 - v2,
                };
                outs[c.out] = if y.is_finite() {
                    y as f32
                } else {
                    s.v = [0.0; 8];
                    0.0
                };
            }
            Kind::Ladder => {
                let x = f64::from(input(c, outs, 0, 0.0));
                let cutoff = param(c, outs, 0).min(sr * 0.45);
                let res = param(c, outs, 1);
                let drive = param(c, outs, 2);
                let mode = param(c, outs, 3) as u32;
                // four one pole stages, with the last fed back through a soft clip
                let g = memo(&mut s.memo[0], cutoff, |f| 1.0 - (-2.0 * PI * f / sr).exp());
                let fb = 4.0 * res;
                let u = (drive * x * (1.0 + res) - fb * s.v[3]).tanh();
                s.v[0] += g * (u - s.v[0]);
                s.v[1] += g * (s.v[0] - s.v[1]);
                s.v[2] += g * (s.v[1] - s.v[2]);
                s.v[3] += g * (s.v[2] - s.v[3]);
                let y = if mode == 1 { s.v[1] } else { s.v[3] };
                outs[c.out] = if y.is_finite() {
                    y as f32
                } else {
                    s.v = [0.0; 8];
                    0.0
                };
            }
            Kind::Shaper => {
                let x = f64::from(input(c, outs, 0, 0.0));
                let drive = param(c, outs, 0);
                let shape = param(c, outs, 1) as u32;
                let mix = param(c, outs, 2);
                let y = match shape {
                    0 => (drive * x).tanh() / drive.tanh(),
                    1 => fold(drive * x),
                    _ => (drive * x).clamp(-1.0, 1.0),
                };
                outs[c.out] = (x + (y - x) * mix) as f32;
            }
            Kind::Ring => {
                let a = input(c, outs, 0, 0.0);
                let b = input(c, outs, 1, 0.0);
                let mix = param(c, outs, 0) as f32;
                outs[c.out] = a * (1.0 - mix) + a * b * mix;
            }
            Kind::Delay => {
                let x = f64::from(input(c, outs, 0, 0.0));
                let time = param(c, outs, 0);
                let fb = param(c, outs, 1);
                let mix = param(c, outs, 2);
                let tone = param(c, outs, 3);
                let len = line.len();
                let d = (time * sr).clamp(1.0, (len - 2) as f64);
                let y = tap(line, s.n, d);
                // the echoes darken as they repeat
                s.v[0] += (y - s.v[0]) * (0.05 + 0.95 * tone);
                line[s.n % len] = (x + s.v[0] * fb) as f32;
                s.n = (s.n + 1) % len;
                outs[c.out] = (x + (y - x) * mix) as f32;
            }
            Kind::Chorus => {
                let x = f64::from(input(c, outs, 0, 0.0));
                let rate = param(c, outs, 0);
                let depth = param(c, outs, 1);
                let mix = param(c, outs, 2);
                let len = line.len();
                line[s.n % len] = x as f32;
                // two taps that swing on triangles a quarter turn apart
                let base = 0.012 * sr;
                let swing = 0.006 * sr * depth;
                let dl = base + swing * (1.0 + wave(1, s.ph[0], 0.0, 0.5));
                let dr = base + swing * (1.0 + wave(1, (s.ph[0] + 0.25) % 1.0, 0.0, 0.5));
                let yl = tap(line, s.n, dl);
                let yr = tap(line, s.n, dr);
                s.n = (s.n + 1) % len;
                s.ph[0] = (s.ph[0] + rate / sr).rem_euclid(1.0);
                outs[c.out] = (x + (yl - x) * mix) as f32;
                outs[c.out + 1] = (x + (yr - x) * mix) as f32;
            }
            Kind::Reverb => {
                // a small Schroeder reverb: four damped combs a side into two allpasses
                let x = f64::from(input(c, outs, 0, 0.0));
                let size = param(c, outs, 0);
                let damp = param(c, outs, 1) * 0.7;
                let mix = param(c, outs, 2);
                let lens = reverb_lines(sr);
                let fb = 0.7 + 0.28 * size;
                let mut start = [0usize; 12];
                let mut at = 0;
                for (q, l) in lens.iter().enumerate() {
                    start[q] = at;
                    at += l;
                }
                let input_gain = x * 0.05;
                let mut wet = [0.0f64; 2];
                for (side, w) in wet.iter_mut().enumerate() {
                    let mut acc = 0.0;
                    for j in 0..4 {
                        let q = side * 4 + j;
                        let i = start[q] + s.n % lens[q];
                        let y = f64::from(line[i]);
                        s.v[q] = y * (1.0 - damp) + s.v[q] * damp;
                        line[i] = (input_gain + s.v[q] * fb) as f32;
                        acc += y;
                    }
                    for j in 0..2 {
                        let q = 8 + side * 2 + j;
                        let i = start[q] + s.n % lens[q];
                        let b = f64::from(line[i]);
                        line[i] = (acc + b * 0.5) as f32;
                        acc = b - acc;
                    }
                    *w = acc;
                }
                s.n = s.n.wrapping_add(1);
                outs[c.out] = (x * (1.0 - mix) + wet[0] * mix) as f32;
                outs[c.out + 1] = (x * (1.0 - mix) + wet[1] * mix) as f32;
            }
            Kind::Vca => {
                let x = input(c, outs, 0, 0.0);
                let cv = input(c, outs, 1, 1.0);
                outs[c.out] = x * cv * param(c, outs, 0) as f32;
            }
            Kind::Mix => {
                let mut sum = 0.0;
                for q in 0..4 {
                    sum += input(c, outs, q, 0.0) * param(c, outs, q) as f32;
                }
                outs[c.out] = sum;
            }
            Kind::Math => {
                let a = input(c, outs, 0, 0.0);
                let b = input(c, outs, 1, 0.0);
                let a = a * param(c, outs, 0) as f32;
                let y = match param(c, outs, 2) as u32 {
                    0 => a + b,
                    1 => a - b,
                    2 => a * b,
                    3 => a.min(b),
                    4 => a.max(b),
                    _ => (a + b) * 0.5,
                };
                outs[c.out] = y + param(c, outs, 1) as f32;
            }
            Kind::Reroute => {
                outs[c.out] = input(c, outs, 0, 0.0);
            }
            Kind::Sampler => {
                let (l, r) = sampler_step(c, s, outs, v.pitch, v.gate && !trig, trig, sr);
                outs[c.out] = l;
                outs[c.out + 1] = r;
            }
            Kind::Quant => {
                let x = f64::from(input(c, outs, 0, 0.0));
                outs[c.out] = quantize(x, param(c, outs, 0) as usize) as f32;
            }
            Kind::Compare => {
                let a = input(c, outs, 0, 0.0);
                let b = input(c, outs, 1, 0.0);
                let above = a > b + param(c, outs, 0) as f32;
                outs[c.out] = if above { 1.0 } else { 0.0 };
                outs[c.out + 1] = if above { 0.0 } else { 1.0 };
            }
            Kind::Pan => {
                let x = input(c, outs, 0, 0.0);
                // equal power, at full level in the middle
                let a = (param(c, outs, 0) + 1.0) * PI * 0.25;
                outs[c.out] = x * (a.cos() * std::f64::consts::SQRT_2) as f32;
                outs[c.out + 1] = x * (a.sin() * std::f64::consts::SQRT_2) as f32;
            }
            Kind::Output => {
                if k == ctx.out_at {
                    let left = input(c, outs, 0, 0.0);
                    let right = if plugged(c, 1) {
                        input(c, outs, 1, 0.0)
                    } else {
                        left
                    };
                    let level = param(c, outs, 0) as f32 * graph.volume;
                    let width = param(c, outs, 1) as f32;
                    let mid = (left + right) * 0.5;
                    let side = (left - right) * 0.5 * width;
                    // a runaway loop is held back rather than blowing up
                    result = (
                        ((mid + side) * level).clamp(-4.0, 4.0),
                        ((mid - side) * level).clamp(-4.0, 4.0),
                    );
                }
            }
        }
        // a cable loop can grow without end; keep every signal finite and bounded
        for o in &mut v.outs[c.out..c.out + c.outputs] {
            *o = if o.is_finite() {
                o.clamp(-64.0, 64.0)
            } else {
                0.0
            };
        }
    }
    v.trig = false;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn run(m: &mut Modular, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        m.render(&mut l, &mut r);
        (l, r)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Frequency from the rising zero crossings.
    fn freq(x: &[f32]) -> f64 {
        let ups: Vec<usize> = (1..x.len())
            .filter(|&i| x[i - 1] <= 0.0 && x[i] > 0.0)
            .collect();
        if ups.len() < 2 {
            return 0.0;
        }
        (ups.len() - 1) as f64 * SR / (ups[ups.len() - 1] - ups[0]) as f64
    }

    fn sine_patch() -> Patch {
        Patch::new(
            vec![
                Node::new(1, Kind::Keys, [0.0; 2]),
                Node::new(2, Kind::Osc, [0.0; 2]).with("wave", 0.0),
                Node::new(3, Kind::Output, [0.0; 2]).with("level", 1.0),
            ],
            vec![
                Cable::new(1, "pitch", 2, "pitch", 1.0),
                Cable::new(2, "out", 3, "left", 1.0),
            ],
            4,
        )
    }

    /// Keys and an output at level 1, with these modules and cables between them.
    fn rig(nodes: Vec<Node>, cables: Vec<Cable>) -> Patch {
        let mut p = Patch::new(
            vec![
                Node::new(1, Kind::Keys, [0.0; 2]),
                Node::new(100, Kind::Output, [0.0; 2]).with("level", 1.0),
            ],
            cables,
            4,
        );
        p.nodes.extend(nodes);
        p
    }

    /// Play one note of a patch.
    fn play(p: &Patch, key: u8, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut m = Modular::new(p, SR);
        m.note_on(key, 1.0);
        run(&mut m, frames)
    }

    /// A constant signal from a math module's offset.
    fn dc(id: u32, v: f64) -> Node {
        Node::new(id, Kind::Math, [0.0; 2]).with("offset", v)
    }

    #[test]
    fn silent_until_a_note_then_plays_its_pitch() {
        let mut m = Modular::new(&sine_patch(), SR);
        let (l, _) = run(&mut m, 1024);
        assert_eq!(rms(&l), 0.0);
        m.note_on(69, 1.0);
        let (l, r) = run(&mut m, 48_000);
        assert!((freq(&l) - 440.0).abs() < 0.5, "{}", freq(&l));
        // right follows left when it has no cable
        assert_eq!(l, r);
        assert!((rms(&l) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn the_default_patch_rings_out_and_frees_its_voice() {
        let mut m = Modular::new(&Patch::default(), SR);
        m.note_on(60, 1.0);
        let (l, _) = run(&mut m, 4_800);
        assert!(rms(&l) > 0.05);
        m.note_off(60);
        // the release is 0.3 s
        let (l, _) = run(&mut m, 48_000);
        assert!(rms(&l[40_000..]) < 1e-4);
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn a_cable_into_a_parameter_modulates_it() {
        // an LFO on the pitch: the frequency moves; with amount 0 it does not
        let mut p = sine_patch();
        p.nodes
            .push(Node::new(4, Kind::Lfo, [0.0; 2]).with("rate", 1.0));
        p.cables.push(Cable::new(4, "out", 2, "tune", 0.25));
        let mut m = Modular::new(&p, SR);
        m.note_on(69, 1.0);
        // the first quarter of the LFO goes up: 12 semitones at the top
        let (l, _) = run(&mut m, 12_000);
        let late = freq(&l[9_000..12_000]);
        assert!(late > 700.0, "{late}");
        p.cables.last_mut().unwrap().amount = 0.0;
        let mut m = Modular::new(&p, SR);
        m.note_on(69, 1.0);
        let (l, _) = run(&mut m, 12_000);
        assert!((freq(&l[9_000..12_000]) - 440.0).abs() < 2.0);
    }

    #[test]
    fn the_envelope_opens_the_filter() {
        let mut p = Patch::default();
        // more envelope on the cutoff gives a brighter attack than none
        let mut bright = |amount: f64| {
            p.cables[3].amount = amount;
            let mut m = Modular::new(&p, SR);
            m.note_on(48, 1.0);
            let (l, _) = run(&mut m, 2_400);
            // the high end: the difference of neighbouring samples
            let d: Vec<f32> = l.windows(2).map(|w| w[1] - w[0]).collect();
            rms(&d)
        };
        let (none, much) = (bright(0.0), bright(0.6));
        assert!(much > none * 1.5, "{none} {much}");
    }

    #[test]
    fn a_loop_of_cables_runs_and_stays_finite() {
        let mut p = sine_patch();
        // the oscillator frequency modulates itself through a math module
        p.nodes
            .push(Node::new(4, Kind::Math, [0.0; 2]).with("scale", 4.0));
        p.cables.push(Cable::new(2, "out", 4, "a", 1.0));
        p.cables.push(Cable::new(4, "out", 2, "fm", 1.0));
        p.cables.push(Cable::new(4, "out", 4, "b", 1.0));
        let mut m = Modular::new(&p, SR);
        m.note_on(60, 1.0);
        let (l, _) = run(&mut m, 48_000);
        assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 4.0));
        assert!(rms(&l) > 0.0);
    }

    #[test]
    fn voices_add_up_and_the_oldest_is_taken_when_all_play() {
        let mut m = Modular::new(&sine_patch(), SR);
        m.note_on(60, 1.0);
        let (one, _) = run(&mut m, 4_800);
        m.note_on(67, 1.0);
        let (two, _) = run(&mut m, 4_800);
        assert!(rms(&two) > rms(&one) * 1.2);
        for k in 70..80 {
            m.note_on(k, 1.0);
        }
        assert_eq!(m.active_voices(), 4);
        // the first key's voice went to a newer note
        assert!(!m.voices.iter().any(|v| v.active && v.key == 60));
    }

    #[test]
    fn the_voice_count_changes_live_at_any_time_while_keys_play() {
        // every count from 0 to past the most, up and down, mono and back, with keys
        // held and keys coming and going: nothing panics and the voices that may take
        // notes never pass the count
        for (_, base) in factory() {
            let mut m = Modular::new(&base, SR);
            let (mut l, mut r) = (vec![0.0; 97], vec![0.0; 97]);
            let mut key = 40u8;
            for step in 0..80usize {
                let mut p = m.patch().clone();
                p.voices = [0, 1, 2, 7, 32, 33, 3, 1, 100, 5][step % 10];
                p.mono = step % 7 == 3;
                drop(m.apply_live(Box::new(p.clone())));
                assert_eq!(m.polyphony(), poly_of(&p));
                for _ in 0..3 {
                    m.note_on(key, 0.8);
                    key = 40 + (key - 39) % 40;
                }
                m.note_off(key.wrapping_sub(2));
                m.render(&mut l, &mut r);
                let gated = m.voices.iter().filter(|v| v.active && v.gate).count();
                assert!(gated <= m.polyphony(), "{gated} > {}", m.polyphony());
                assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            }
        }
    }

    #[test]
    fn fewer_voices_let_the_extra_ones_ring_out_and_more_take_new_keys() {
        let mut p = sine_patch();
        p.voices = 8;
        let mut m = Modular::new(&p, SR);
        for k in 60..68 {
            m.note_on(k, 1.0);
        }
        run(&mut m, 480);
        assert_eq!(m.voices.iter().filter(|v| v.gate).count(), 8);
        p.voices = 2;
        drop(m.apply_live(Box::new(p.clone())));
        // two still held, the rest let go (and still sounding, for now)
        assert_eq!(m.voices.iter().filter(|v| v.gate).count(), 2);
        assert!(m.active_voices() > 2);
        m.note_on(80, 1.0);
        assert_eq!(m.voices.iter().filter(|v| v.gate).count(), 2);
        p.voices = 6;
        drop(m.apply_live(Box::new(p)));
        for k in 81..85 {
            m.note_on(k, 1.0);
        }
        assert_eq!(m.voices.iter().filter(|v| v.gate).count(), 6);
    }

    #[test]
    fn the_patch_settings_change_the_sound_live() {
        // volume, glide, legato, mono, voices and a cable's amount, each taken by a
        // playing synth (no rebuild) and heard
        fn take(base: &Patch, f: impl Fn(&mut Patch)) -> Vec<f32> {
            let mut m = Modular::new(base, SR);
            let mut p = base.clone();
            f(&mut p);
            let back = m.apply_live(Box::new(p.clone()));
            assert_eq!(*back, *base, "the old patch comes back");
            assert_eq!(m.patch(), &p);
            m.note_on(60, 1.0);
            let (mut a, _) = run(&mut m, 2_400);
            m.note_on(72, 1.0);
            let (b, _) = run(&mut m, 4_800);
            a.extend(b);
            a
        }
        let base = Patch::default();
        let plain = take(&base, |_| {});
        let differs = |x: &[f32]| x.iter().zip(&plain).any(|(a, b)| (a - b).abs() > 1e-4);
        assert!(differs(&take(&base, |p| p.volume = 0.3)), "volume");
        assert!(differs(&take(&base, |p| p.voices = 1)), "voices");
        assert!(differs(&take(&base, |p| p.mono = true)), "mono");
        let mono = |p: &mut Patch| p.mono = true;
        let mono_plain = take(&base, mono);
        let glide = take(&base, |p| {
            p.mono = true;
            p.glide = 0.3;
        });
        assert!(
            glide
                .iter()
                .zip(&mono_plain)
                .any(|(a, b)| (a - b).abs() > 1e-4),
            "glide"
        );
        let legato = take(&base, |p| {
            p.mono = true;
            p.legato = true;
        });
        assert!(
            legato
                .iter()
                .zip(&mono_plain)
                .any(|(a, b)| (a - b).abs() > 1e-4),
            "legato"
        );
        assert!(
            differs(&take(&base, |p| p.cables[0].amount = 0.25)),
            "a cable's amount"
        );
        // another shape is not taken
        let mut m = Modular::new(&base, SR);
        let mut other = base.clone();
        other.cables.pop();
        let back = m.apply_live(Box::new(other.clone()));
        assert_eq!(*back, other);
        assert_eq!(m.patch(), &base);
    }

    #[test]
    fn a_single_input_keeps_one_cable_and_a_summing_input_keeps_all() {
        let mut p = Patch::default();
        let lfo = p.free_id();
        p.nodes.push(Node::new(lfo, Kind::Lfo, [0.0; 2]));
        // a second cable into the filter's single input, and into the output's summing one
        p.cables.push(Cable::new(lfo, "out", 4, "in", 1.0));
        p.cables.push(Cable::new(lfo, "out", 6, "left", 1.0));
        let back = Patch::from_json(&p.to_json()).unwrap();
        let into = |q: &Patch, to: u32, input: &str| {
            q.cables
                .iter()
                .filter(|c| c.to == to && c.input == input)
                .map(|c| c.from)
                .collect::<Vec<_>>()
        };
        assert_eq!(into(&back, 4, "in"), vec![2]);
        assert_eq!(into(&back, 6, "left"), vec![5, lfo]);
        assert!(!Kind::Filter.accepts_many("in"));
        assert!(Kind::Filter.accepts_many("cutoff"));
        assert!(Kind::Mix.accepts_many("a"));
        // every factory patch already keeps to one cable per single input
        for (name, f) in factory() {
            let mut g = f.clone();
            g.one_cable_per_input();
            assert_eq!(g.cables, f.cables, "{name}");
        }
    }

    /// A ramp from 0 to 1 over a second, at 48 kHz, as a sample.
    fn ramp() -> SampleBuf {
        let n = 48_000;
        let v: std::sync::Arc<[f32]> = (0..n).map(|i| i as f32 / n as f32).collect();
        SampleBuf {
            left: v.clone(),
            right: v,
            rate: 48_000.0,
        }
    }

    fn sampler_patch(params: &[(&str, f64)]) -> Patch {
        let mut n = Node::new(2, Kind::Sampler, [0.0; 2]);
        n.sample = Some("smp".into());
        for (k, v) in params {
            n.params.insert((*k).to_string(), *v);
        }
        Patch::new(
            vec![
                Node::new(1, Kind::Keys, [0.0; 2]),
                n,
                Node::new(3, Kind::Output, [0.0; 2]).with("level", 1.0),
            ],
            vec![
                Cable::new(2, "left", 3, "left", 1.0),
                Cable::new(2, "right", 3, "right", 1.0),
            ],
            1,
        )
    }

    fn play_sampler(p: &Patch, key: u8, frames: usize) -> Vec<f32> {
        let mut m = Modular::with_samples(p, SR, vec![("smp".into(), ramp())]);
        m.note_on(key, 1.0);
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        m.render(&mut l, &mut r);
        l
    }

    #[test]
    fn a_sampler_plays_its_sample_at_the_key_pitch() {
        let p = sampler_patch(&[]);
        // at its root key it reads the sample as it is: a rising ramp
        let l = play_sampler(&p, 60, 4800);
        let (a, b) = (l[1000], l[3000]);
        assert!(b > a && a > 0.0, "{a} {b}");
        // an octave up it goes twice as fast
        let up = play_sampler(&p, 72, 4800);
        assert!(
            (up[3000] / l[3000] - 2.0).abs() < 0.1,
            "{} {}",
            up[3000],
            l[3000]
        );
        // fixed pitch: the key does not matter
        let fixed = sampler_patch(&[("follow", 0.0)]);
        let f = play_sampler(&fixed, 72, 4800);
        assert!((f[3000] - l[3000]).abs() < 1e-3);
        // reversed, it starts at the end and falls
        let rev = play_sampler(&sampler_patch(&[("reverse", 1.0)]), 60, 4800);
        assert!(rev[100] > 0.9 && rev[3000] < rev[100]);
        // start and gain
        let late = play_sampler(&sampler_patch(&[("start", 0.5), ("gain", 0.5)]), 60, 4800);
        assert!((late[10] - 0.25).abs() < 0.02, "{}", late[10]);
        // no sample: silence, not a crash
        let mut m = Modular::new(&p, SR);
        m.note_on(60, 1.0);
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        m.render(&mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn a_sampler_loops_or_stops_and_a_gate_cuts_it() {
        // a tenth of the sample, played for half a second
        let once = play_sampler(&sampler_patch(&[("end", 0.1)]), 60, 24_000);
        assert!(once[20_000].abs() < 1e-6, "one-shot stops at its end");
        let looped = play_sampler(&sampler_patch(&[("end", 0.1), ("loop", 1.0)]), 60, 24_000);
        assert!(looped[20_000] > 0.0, "a loop goes round");
        // gate mode: silent once the key is let go
        let p = sampler_patch(&[("trigger", 1.0), ("loop", 1.0)]);
        let mut m = Modular::with_samples(&p, SR, vec![("smp".into(), ramp())]);
        m.note_on(60, 1.0);
        let (mut l, mut r) = (vec![0.0; 2000], vec![0.0; 2000]);
        m.render(&mut l, &mut r);
        assert!(l[1500] > 0.0);
        m.note_off(60);
        m.render(&mut l, &mut r);
        assert!(l[1500].abs() < 1e-6);
        // a reroute passes the signal through
        let mut q = sampler_patch(&[]);
        q.nodes.push(Node::new(4, Kind::Reroute, [0.0; 2]));
        q.cables = vec![
            Cable::new(2, "left", 4, "in", 1.0),
            Cable::new(4, "out", 3, "left", 1.0),
        ];
        let through = play_sampler(&q, 60, 4800);
        assert!(through[3000] > 0.0);
        // the sample id is part of the shape: a new one builds the player again
        let mut other = p.clone();
        other.nodes[1].sample = Some("else".into());
        assert!(!p.same_shape(&other));
        // and it survives a round trip
        assert_eq!(Patch::from_json(&p.to_json()).unwrap(), p);
    }

    #[test]
    fn the_math_module_does_its_operation() {
        // a = 0.5, b = 0.2
        let ops = [0.7, 0.3, 0.1, 0.2, 0.5, 0.35];
        for (op, want) in ops.into_iter().enumerate() {
            let p = Patch::new(
                vec![
                    Node::new(1, Kind::Macro, [0.0; 2])
                        .with("knob_1", 0.5)
                        .with("knob_2", 0.2),
                    Node::new(2, Kind::Math, [0.0; 2]).with("op", op as f64),
                    Node::new(3, Kind::Output, [0.0; 2]).with("level", 1.0),
                ],
                vec![
                    Cable::new(1, "out_1", 2, "a", 1.0),
                    Cable::new(1, "out_2", 2, "b", 1.0),
                    Cable::new(2, "out", 3, "left", 1.0),
                ],
                1,
            );
            let mut m = Modular::new(&p, SR);
            m.note_on(60, 1.0);
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            m.render(&mut l, &mut r);
            let first = {
                let mut q = p.clone();
                q.nodes[1].params.insert("op".into(), 0.0);
                let mut m = Modular::new(&q, SR);
                m.note_on(60, 1.0);
                let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
                m.render(&mut l, &mut r);
                l[200]
            };
            // the output may scale the level: compare against the add result
            let got = l[200] / first * 0.7;
            assert!((got - want as f32).abs() < 1e-3, "op {op}: {got} vs {want}");
        }
        assert_eq!(Kind::Math.choices("op").unwrap()[2], "multiply");
    }

    #[test]
    fn patches_round_trip_and_bad_cables_are_left_out() {
        let mut p = Patch::default();
        p.cables.push(Cable::new(2, "nothing", 4, "in", 1.0));
        p.cables.push(Cable::new(99, "out", 4, "in", 1.0));
        let back = Patch::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p);
        // it plays like the default patch
        let mut a = Modular::new(&p, SR);
        let mut b = Modular::new(&Patch::default(), SR);
        a.note_on(60, 1.0);
        b.note_on(60, 1.0);
        assert_eq!(run(&mut a, 2_000), run(&mut b, 2_000));
        // a patch from the editor may leave things out
        let p = Patch::from_json(&serde_json::json!({
            "nodes": [{ "id": 1, "kind": "osc" }]
        }))
        .unwrap();
        assert_eq!(p.voices, 8);
        assert_eq!(p.nodes[0].param("wave"), 2.0);
        assert!(
            Patch::from_json(&serde_json::json!({ "nodes": [{ "id": 1, "kind": "bogus" }] }))
                .is_err()
        );
    }

    #[test]
    fn an_old_patch_loads_with_the_new_settings_at_their_defaults() {
        // a patch saved before unison, mono and volume
        let old = serde_json::json!({
            "nodes": [
                { "id": 1, "kind": "keys", "pos": [0, 0] },
                { "id": 2, "kind": "osc", "params": { "wave": 0.0 } },
                { "id": 3, "kind": "output", "params": { "level": 1.0 } }
            ],
            "cables": [
                { "from": 1, "output": "pitch", "to": 2, "input": "pitch" },
                { "from": 2, "output": "out", "to": 3, "input": "left" }
            ],
            "voices": 4
        });
        let p = Patch::from_json(&old).unwrap();
        assert!(!p.mono && !p.legato);
        assert_eq!((p.glide, p.volume), (0.0, 1.0));
        assert_eq!(p.nodes[1].param("voices"), 1.0);
        assert_eq!(p.nodes[2].param("width"), 1.0);
        let (l, _) = play(&p, 69, 24_000);
        assert!((freq(&l) - 440.0).abs() < 0.5);
        assert!((rms(&l) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn moving_a_parameter_keeps_the_voices_playing() {
        let mut m = Modular::new(&sine_patch(), SR);
        m.note_on(69, 1.0);
        run(&mut m, 1_000);
        let mut p = m.patch().clone();
        p.nodes[1].params.insert("tune".into(), 12.0);
        m.set_patch(&p);
        assert_eq!(m.active_voices(), 1);
        let (l, _) = run(&mut m, 24_000);
        assert!((freq(&l) - 880.0).abs() < 1.0, "{}", freq(&l));
        // the volume is a setting, which keeps them playing too
        p.volume = 0.5;
        m.set_patch(&p);
        assert_eq!(m.active_voices(), 1);
        // a new cable starts again
        p.cables.pop();
        m.set_patch(&p);
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn there_are_many_factory_patches_and_each_plays() {
        let all = factory();
        assert!(all.len() >= 25, "{}", all.len());
        let names: std::collections::HashSet<_> = all.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.len(), all.len(), "names are unique");
        for (name, p) in all {
            assert!(p.cables.iter().all(|c| p.is_valid(c)), "{name}");
            let mut m = Modular::new(&p, SR);
            m.note_on(57, 0.8);
            let (l, r) = run(&mut m, 48_000);
            for x in [&l, &r] {
                assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 4.0), "{name}");
                let peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
                assert!(rms(x) > 0.003 && peak > 0.02, "{name} {} {peak}", rms(x));
            }
            // and it goes quiet once the key is let go
            m.note_off(57);
            for _ in 0..10 {
                if m.active_voices() == 0 {
                    break;
                }
                run(&mut m, 48_000);
            }
            assert_eq!(m.active_voices(), 0, "{name}");
        }
    }

    #[test]
    fn parameters_map_their_ranges() {
        let c = Kind::Filter.param("cutoff").unwrap();
        assert!((c.denormalize(c.normalize(440.0)) - 440.0).abs() < 1e-9);
        assert!((c.denormalize(0.5) - (20.0f64 * 20_000.0).sqrt()).abs() < 1e-6);
        let w = Kind::Osc.param("wave").unwrap();
        assert_eq!(w.denormalize(0.3), 1.0);
        assert_eq!(Kind::Vca.input_index("gain"), Some(2));
        assert_eq!(Kind::Vca.input_index("cv"), Some(1));
        // a whole number keeps its own range
        let v = Kind::Osc.param("voices").unwrap();
        assert_eq!((v.denormalize(0.0), v.denormalize(1.0)), (1.0, 8.0));
        assert_eq!(v.normalize(1.0), 0.0);
    }

    #[test]
    fn every_kind_has_a_category_and_its_choices_fit() {
        for k in Kind::ALL {
            assert!(k.category() < CATEGORIES.len());
            for p in k.spec().params {
                if let Some(c) = k.choices(p.name) {
                    assert_eq!(c.len() as u32, p.steps, "{k:?} {}", p.name);
                }
            }
            // automation ids have room for every parameter
            assert!(k.spec().params.len() < 64);
        }
        // every kind is in the list once
        let set: std::collections::HashSet<_> = Kind::ALL.iter().collect();
        assert_eq!(set.len(), Kind::ALL.len());
    }

    // ----- the oscillator

    #[test]
    fn unison_spreads_the_voices_and_keeps_the_pitch() {
        let mut p = sine_patch();
        p.nodes[1] = Node::new(2, Kind::Osc, [0.0; 2])
            .with("wave", 0.0)
            .with("voices", 4.0)
            .with("detune", 4.0)
            .with("spread", 1.0);
        p.cables.push(Cable::new(2, "right", 3, "right", 1.0));
        let (l, r) = play(&p, 69, 48_000);
        assert!((freq(&l) - 440.0).abs() < 3.0, "{}", freq(&l));
        assert!(l != r, "spread makes it stereo");
        assert!(rms(&l) > 0.2 && rms(&r) > 0.2);
        // with no spread both sides are the same
        p.nodes[1].params.insert("spread".into(), 0.0);
        let (l, r) = play(&p, 69, 4_800);
        assert_eq!(l, r);
        // one voice puts the same wave on both outputs
        p.nodes[1].params.insert("voices".into(), 1.0);
        let (l, r) = play(&p, 69, 4_800);
        assert_eq!(l, r);
    }

    #[test]
    fn hard_sync_makes_the_slave_repeat_with_the_master() {
        // a master at 480 Hz repeats every 100 samples
        let pitch = (480.0 / C4).log2();
        let make = |sync: bool| {
            let mut cables = vec![
                Cable::new(2, "out", 3, "pitch", 1.0),
                Cable::new(2, "out", 4, "pitch", 1.0),
                Cable::new(4, "out", 100, "left", 1.0),
            ];
            if sync {
                cables.push(Cable::new(3, "out", 4, "sync", 1.0));
            }
            rig(
                vec![
                    dc(2, pitch),
                    Node::new(3, Kind::Osc, [0.0; 2]).with("wave", 0.0),
                    Node::new(4, Kind::Osc, [0.0; 2]).with("tune", 5.3),
                ],
                cables,
            )
        };
        let err = |x: &[f32]| {
            (10_000..20_000)
                .map(|i| (x[i] - x[i + 100]).abs())
                .sum::<f32>()
                / 10_000.0
        };
        let (synced, _) = play(&make(true), 60, 24_000);
        let (free, _) = play(&make(false), 60, 24_000);
        assert!(err(&synced) < 0.02, "{}", err(&synced));
        assert!(err(&free) > 0.2, "{}", err(&free));
    }

    #[test]
    fn linear_and_pitch_fm_move_the_frequency() {
        // an octave into pitch fm, and a signal of 1 into linear fm, both double it
        for input in ["pfm", "fm"] {
            let mut p = sine_patch();
            p.nodes.push(dc(4, 1.0));
            p.cables.push(Cable::new(4, "out", 2, input, 1.0));
            let (l, _) = play(&p, 69, 24_000);
            assert!((freq(&l) - 880.0).abs() < 1.0, "{input} {}", freq(&l));
        }
    }

    #[test]
    fn morph_blends_the_waves_in_order() {
        let shape = |wave: f64, morph: f64| {
            let mut p = sine_patch();
            p.nodes[1] = Node::new(2, Kind::Osc, [0.0; 2])
                .with("wave", wave)
                .with("morph", morph);
            play(&p, 57, 4_800).0
        };
        // the whole numbers are the basic waves
        let same = |a: Vec<f32>, b: Vec<f32>| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(same(shape(4.0, 0.0), shape(0.0, 0.0)));
        assert!(same(shape(4.0, 1.0), shape(1.0, 0.0)));
        assert!(same(shape(4.0, 3.0), shape(3.0, 0.0)));
        // halfway is the mean of its two neighbours
        let (a, b, half) = (shape(0.0, 0.0), shape(1.0, 0.0), shape(4.0, 0.5));
        for k in (0..4_800).step_by(97) {
            assert!((half[k] - (a[k] + b[k]) * 0.5).abs() < 1e-4);
        }
    }

    #[test]
    fn the_phase_resets_on_each_note_or_runs_free() {
        let mut p = sine_patch();
        p.nodes[1] = Node::new(2, Kind::Osc, [0.0; 2])
            .with("wave", 0.0)
            .with("phase", 0.25);
        // a sine a quarter in is at its top
        let (l, _) = play(&p, 69, 16);
        assert!((l[0] - 1.0).abs() < 1e-3, "{}", l[0]);
        p.nodes[1].params.insert("retrig".into(), 0.0);
        let mut m = Modular::new(&p, SR);
        m.note_on(60, 1.0);
        m.note_on(64, 1.0);
        let (l, _) = run(&mut m, 2);
        // two free voices start at different places, so they do not add to 2
        assert!((l[0] - 2.0).abs() > 0.01);
    }

    // ----- sources

    #[test]
    fn an_fm_operator_follows_its_ratio_and_feedback_adds_harmonics() {
        let op = |ratio: f64, fb: f64| {
            rig(
                vec![Node::new(2, Kind::Fm, [0.0; 2])
                    .with("ratio", ratio)
                    .with("feedback", fb)],
                vec![
                    Cable::new(1, "pitch", 2, "pitch", 1.0),
                    Cable::new(2, "out", 100, "left", 1.0),
                ],
            )
        };
        let (l, _) = play(&op(2.0, 0.0), 69, 24_000);
        assert!((freq(&l) - 880.0).abs() < 1.0, "{}", freq(&l));
        let hi = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
        let (pure, _) = play(&op(1.0, 0.0), 57, 24_000);
        let (rich, _) = play(&op(1.0, 0.8), 57, 24_000);
        assert!(hi(&rich) > hi(&pure) * 1.3, "{} {}", hi(&pure), hi(&rich));
        assert!(rich.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    }

    #[test]
    fn macro_knobs_are_outputs() {
        let p = rig(
            vec![Node::new(2, Kind::Macro, [0.0; 2]).with("knob_3", 0.4)],
            vec![Cable::new(2, "out_3", 100, "left", 1.0)],
        );
        let (l, _) = play(&p, 60, 64);
        assert!((l[10] - 0.4).abs() < 1e-6);
        // and a knob can be moved like any parameter
        let mut m = Modular::new(&p, SR);
        m.set_param(2, "knob_3", 0.9);
        m.note_on(60, 1.0);
        assert!((run(&mut m, 8).0[4] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn keys_put_out_velocity_and_the_performance_controls() {
        let p = rig(
            vec![],
            vec![
                Cable::new(1, "velocity", 100, "left", 1.0),
                Cable::new(1, "wheel", 100, "right", 1.0),
            ],
        );
        let mut m = Modular::new(&p, SR);
        m.set_controls(Controls {
            wheel: 0.25,
            bend: 0.0,
            pressure: 0.0,
        });
        m.note_on(60, 0.5);
        let (l, r) = run(&mut m, 8);
        assert_eq!((l[4], r[4]), (0.5, 0.25));
        // a full bend up with a range of 12 is an octave
        let mut p = sine_patch();
        p.nodes[0] = Node::new(1, Kind::Keys, [0.0; 2]).with("bend_range", 12.0);
        let mut m = Modular::new(&p, SR);
        m.set_controls(Controls {
            wheel: 0.0,
            bend: 1.0,
            pressure: 0.0,
        });
        m.note_on(69, 1.0);
        let (l, _) = run(&mut m, 24_000);
        assert!((freq(&l) - 880.0).abs() < 1.0, "{}", freq(&l));
    }

    // ----- modulators

    #[test]
    fn the_envelope_curve_bends_the_attack_and_a_loop_repeats() {
        let env = |curve: f64, looped: f64| {
            let p = rig(
                vec![Node::new(2, Kind::Env, [0.0; 2])
                    .with("attack", 0.01)
                    .with("decay", 0.01)
                    .with("sustain", 0.0)
                    .with("curve", curve)
                    .with("loop", looped)],
                vec![
                    Cable::new(1, "gate", 2, "gate", 1.0),
                    Cable::new(2, "out", 100, "left", 1.0),
                ],
            );
            play(&p, 60, 24_000).0
        };
        // halfway up the 10 ms attack
        let (lin, slow, fast) = (env(0.0, 0.0), env(1.0, 0.0), env(-1.0, 0.0));
        assert!((lin[240] - 0.5).abs() < 0.02, "{}", lin[240]);
        assert!(slow[240] < 0.2 && fast[240] > 0.8);
        // without a loop it falls and stays; with one it keeps coming back up
        let peaks = |x: &[f32]| {
            (1..x.len())
                .filter(|&i| x[i] > 0.9 && x[i - 1] <= 0.9)
                .count()
        };
        assert_eq!(peaks(&lin), 1);
        assert!(peaks(&env(0.0, 1.0)) > 10);
    }

    #[test]
    fn the_lfo_starts_at_its_phase_and_has_a_random_step() {
        let lfo = |wave: f64| {
            rig(
                vec![Node::new(2, Kind::Lfo, [0.0; 2])
                    .with("wave", wave)
                    .with("rate", 10.0)
                    .with("phase", 0.25)],
                vec![Cable::new(2, "out", 100, "left", 1.0)],
            )
        };
        let (l, _) = play(&lfo(0.0), 60, 8);
        assert!((l[0] - 1.0).abs() < 1e-3);
        // the random wave holds a value for each cycle (4800 samples at 10 Hz)
        let (l, _) = play(&lfo(4.0), 60, 48_000);
        let changes = (1..l.len()).filter(|&i| l[i] != l[i - 1]).count();
        assert!((8..=11).contains(&changes), "{changes}");
        // a held step stays put inside its cycle
        assert_eq!(l[100], l[1_000]);
    }

    #[test]
    fn sample_and_hold_takes_its_input_at_a_rate_or_on_a_trigger() {
        // at its own rate, on noise
        let p = rig(
            vec![Node::new(2, Kind::Hold, [0.0; 2]).with("rate", 10.0)],
            vec![Cable::new(2, "out", 100, "left", 1.0)],
        );
        let (l, _) = play(&p, 60, 48_000);
        let changes = (1..l.len()).filter(|&i| l[i] != l[i - 1]).count();
        assert!((8..=11).contains(&changes), "{changes}");
        // on a trigger, taking a ramp: it holds the ramp's value at each rising edge
        let p = rig(
            vec![
                Node::new(2, Kind::Hold, [0.0; 2]),
                Node::new(3, Kind::Lfo, [0.0; 2])
                    .with("wave", 2.0)
                    .with("rate", 1.0),
                Node::new(4, Kind::Lfo, [0.0; 2])
                    .with("wave", 3.0)
                    .with("rate", 4.0),
            ],
            vec![
                Cable::new(3, "out", 2, "in", 1.0),
                Cable::new(4, "uni", 2, "trig", 1.0),
                Cable::new(2, "out", 100, "left", 1.0),
            ],
        );
        let (l, _) = play(&p, 60, 48_000);
        let changes = (1..l.len()).filter(|&i| l[i] != l[i - 1]).count();
        assert!((3..=5).contains(&changes), "{changes}");
        // the second edge is a quarter into the ramp, which is at -0.5
        assert!((l[12_100] + 0.5).abs() < 0.01, "{}", l[12_100]);
    }

    #[test]
    fn the_sequencer_steps_through_its_values_and_gates() {
        let mut seq = Node::new(2, Kind::Seq, [0.0; 2])
            .with("rate", 10.0)
            .with("length", 4.0);
        for (k, v) in [0.1, 0.2, 0.3, 0.4].iter().enumerate() {
            seq = seq.with(&format!("step_{}", k + 1), *v);
        }
        let p = rig(
            vec![seq.clone()],
            vec![
                Cable::new(2, "out", 100, "left", 1.0),
                Cable::new(2, "gate", 100, "right", 1.0),
            ],
        );
        let (l, r) = play(&p, 60, 48_000);
        // 4800 samples a step, four steps around
        for (k, want) in [0.1, 0.2, 0.3, 0.4, 0.1].iter().enumerate() {
            assert!((l[k * 4_800 + 100] - want).abs() < 1e-6, "{k}");
        }
        // the gate is open for the first half of each step
        assert_eq!((r[100], r[3_000]), (1.0, 0.0));
        // a clock steps it instead; the first clock plays the first step
        let p = rig(
            vec![
                seq,
                Node::new(3, Kind::Lfo, [0.0; 2])
                    .with("wave", 3.0)
                    .with("rate", 5.0),
            ],
            vec![
                Cable::new(3, "uni", 2, "clock", 1.0),
                Cable::new(2, "out", 100, "left", 1.0),
            ],
        );
        let (l, _) = play(&p, 60, 48_000);
        assert!((l[100] - 0.1).abs() < 1e-6);
        assert!((l[9_700] - 0.2).abs() < 1e-6, "{}", l[9_700]);
    }

    #[test]
    fn slew_and_glide_smooth_a_jump() {
        // the gate is 0 on a note's first sample, then 1
        let p = rig(
            vec![Node::new(3, Kind::Slew, [0.0; 2]).with("rise", 0.1)],
            vec![
                Cable::new(1, "gate", 3, "in", 1.0),
                Cable::new(3, "out", 100, "left", 1.0),
            ],
        );
        let (l, _) = play(&p, 60, 48_000);
        assert!(l[480] > 0.2 && l[480] < 0.9, "{}", l[480]);
        assert!((l[47_000] - 1.0).abs() < 1e-3);
        // a glide on keys slides from one note to the next
        let mut p = sine_patch();
        p.nodes[0] = Node::new(1, Kind::Keys, [0.0; 2]).with("glide", 0.2);
        p = mono(p, 0.0, false);
        let mut m = Modular::new(&p, SR);
        m.note_on(57, 1.0);
        run(&mut m, 24_000);
        m.note_on(69, 1.0);
        let (l, _) = run(&mut m, 24_000);
        let early = freq(&l[0..2_400]);
        assert!(early > 225.0 && early < 400.0, "{early}");
        assert!((freq(&l[20_000..]) - 440.0).abs() < 2.0);
    }

    // ----- filters

    #[test]
    fn the_ladder_darkens_and_stays_bounded_at_full_resonance() {
        let ladder = |cutoff: f64, res: f64, drive: f64| {
            rig(
                vec![
                    Node::new(2, Kind::Osc, [0.0; 2]),
                    Node::new(3, Kind::Ladder, [0.0; 2])
                        .with("cutoff", cutoff)
                        .with("res", res)
                        .with("drive", drive),
                ],
                vec![
                    Cable::new(1, "pitch", 2, "pitch", 1.0),
                    Cable::new(2, "out", 3, "in", 1.0),
                    Cable::new(3, "out", 100, "left", 1.0),
                ],
            )
        };
        let hi = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
        let (dark, _) = play(&ladder(300.0, 0.0, 1.0), 48, 24_000);
        let (open, _) = play(&ladder(10_000.0, 0.0, 1.0), 48, 24_000);
        assert!(hi(&open) > hi(&dark) * 4.0);
        assert!(rms(&dark) > 0.05);
        let (wild, _) = play(&ladder(2_000.0, 1.0, 10.0), 48, 48_000);
        assert!(wild.iter().all(|v| v.is_finite() && v.abs() <= 1.5));
        assert!(rms(&wild) > 0.05);
    }

    // ----- effects

    fn through(fx: Node, input: Node) -> Patch {
        rig(
            vec![input, fx],
            vec![
                Cable::new(1, "pitch", 2, "pitch", 1.0),
                Cable::new(2, "out", 3, "in", 1.0),
                Cable::new(3, "out", 100, "left", 1.0),
                Cable::new(3, "left", 100, "left", 1.0),
                Cable::new(3, "right", 100, "right", 1.0),
            ],
        )
    }

    fn sine() -> Node {
        Node::new(2, Kind::Osc, [0.0; 2]).with("wave", 0.0)
    }

    #[test]
    fn the_shaper_folds_clips_and_mixes() {
        let shaper = |shape: f64, drive: f64, mix: f64| {
            let fx = Node::new(3, Kind::Shaper, [0.0; 2])
                .with("shape", shape)
                .with("drive", drive)
                .with("mix", mix);
            play(&through(fx, sine()), 57, 9_600).0
        };
        let (dry, _) = play(&sine_patch(), 57, 9_600);
        assert_eq!(shaper(1.0, 4.0, 0.0), dry);
        let folded = shaper(1.0, 4.0, 1.0);
        assert!(folded.iter().all(|v| v.abs() <= 1.0 + 1e-6));
        // folding a sine four times over crosses zero far more often
        assert!(freq(&folded) > freq(&dry) * 2.0);
        // hard clipping a sine makes nearly a square
        assert!(rms(&shaper(2.0, 40.0, 1.0)) > 0.95);
        let soft = shaper(0.0, 3.0, 1.0);
        assert!(rms(&soft) > rms(&dry) && soft.iter().all(|v| v.abs() <= 1.0 + 1e-6));
    }

    #[test]
    fn ring_modulation_multiplies() {
        let p = rig(
            vec![sine(), dc(3, 0.5), Node::new(4, Kind::Ring, [0.0; 2])],
            vec![
                Cable::new(1, "pitch", 2, "pitch", 1.0),
                Cable::new(2, "out", 4, "a", 1.0),
                Cable::new(3, "out", 4, "b", 1.0),
                Cable::new(4, "out", 100, "left", 1.0),
            ],
        );
        let (l, _) = play(&p, 69, 4_800);
        let (dry, _) = play(&sine_patch(), 69, 4_800);
        for k in (0..4_800).step_by(37) {
            assert!((l[k] - dry[k] * 0.5).abs() < 1e-5);
        }
    }

    /// A short burst of a sine: an envelope that decays in about 20 ms.
    fn burst(fx: Node) -> Patch {
        let mut p = through(fx, sine());
        p.nodes.push(
            Node::new(4, Kind::Env, [0.0; 2])
                .with("attack", 0.001)
                .with("decay", 0.005)
                .with("sustain", 0.0),
        );
        p.nodes.push(Node::new(5, Kind::Vca, [0.0; 2]));
        p.cables[1] = Cable::new(2, "out", 5, "in", 1.0);
        p.cables.push(Cable::new(5, "out", 3, "in", 1.0));
        p.cables.push(Cable::new(4, "out", 5, "cv", 1.0));
        p.cables.push(Cable::new(1, "gate", 4, "gate", 1.0));
        p
    }

    #[test]
    fn the_delay_repeats_after_its_time() {
        let fx = Node::new(3, Kind::Delay, [0.0; 2])
            .with("time", 0.2)
            .with("feedback", 0.5)
            .with("mix", 1.0)
            .with("tone", 1.0);
        let (l, _) = play(&burst(fx), 69, 48_000);
        let at = |t: f64| rms(&l[(t * SR) as usize..((t + 0.02) * SR) as usize]);
        assert!(at(0.1) < 1e-4);
        assert!(at(0.2) > 0.05, "{}", at(0.2));
        // the second echo is half the first
        assert!(
            (at(0.4) / at(0.2) - 0.5).abs() < 0.1,
            "{}",
            at(0.4) / at(0.2)
        );
        assert!(at(0.3) < 1e-4);
    }

    #[test]
    fn chorus_and_reverb_make_stereo_and_reverb_has_a_tail() {
        let fx = Node::new(3, Kind::Chorus, [0.0; 2]).with("depth", 1.0);
        let (l, r) = play(&through(fx, sine()), 57, 24_000);
        assert!(l != r && rms(&l) > 0.2 && rms(&r) > 0.2);
        let fx = Node::new(3, Kind::Reverb, [0.0; 2])
            .with("size", 0.8)
            .with("mix", 1.0);
        let (l, r) = play(&burst(fx), 57, 48_000);
        let tail = &l[24_000..30_000];
        assert!(rms(tail) > 1e-3, "{}", rms(tail));
        assert!(l != r);
        assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 2.0));
    }

    // ----- utilities

    #[test]
    fn the_quantizer_snaps_to_its_scale() {
        assert_eq!(quantize(0.03, 0), 0.0);
        assert!((quantize(0.07, 0) - 1.0 / 12.0).abs() < 1e-12);
        // a C sharp is not in C major: it goes to C or D
        let q = quantize(1.0 / 12.0, 1) * 12.0;
        assert!(q == 0.0 || q == 2.0);
        assert!((quantize(-1.0 / 12.0, 1) * 12.0 + 1.0).abs() < 1e-12);
        // in a module
        let p = rig(
            vec![dc(2, 0.07), Node::new(3, Kind::Quant, [0.0; 2])],
            vec![
                Cable::new(2, "out", 3, "in", 1.0),
                Cable::new(3, "out", 100, "left", 1.0),
            ],
        );
        assert!((play(&p, 60, 8).0[4] - 1.0 / 12.0).abs() < 1e-6);
    }

    #[test]
    fn the_comparator_is_one_above_and_zero_below() {
        let cmp = |a: f64, threshold: f64| {
            let p = rig(
                vec![
                    dc(2, a),
                    Node::new(3, Kind::Compare, [0.0; 2]).with("threshold", threshold),
                ],
                vec![
                    Cable::new(2, "out", 3, "a", 1.0),
                    Cable::new(3, "out", 100, "left", 1.0),
                    Cable::new(3, "inv", 100, "right", 1.0),
                ],
            );
            let (l, r) = play(&p, 60, 8);
            (l[4], r[4])
        };
        assert_eq!(cmp(0.5, 0.0), (1.0, 0.0));
        assert_eq!(cmp(-0.5, 0.0), (0.0, 1.0));
        assert_eq!(cmp(0.5, 0.6), (0.0, 1.0));
    }

    #[test]
    fn pan_and_width_place_the_sound() {
        let pan = |v: f64| {
            let p = rig(
                vec![sine(), Node::new(3, Kind::Pan, [0.0; 2]).with("pan", v)],
                vec![
                    Cable::new(1, "pitch", 2, "pitch", 1.0),
                    Cable::new(2, "out", 3, "in", 1.0),
                    Cable::new(3, "left", 100, "left", 1.0),
                    Cable::new(3, "right", 100, "right", 1.0),
                ],
            );
            play(&p, 57, 4_800)
        };
        let (l, r) = pan(-1.0);
        assert!(rms(&r) < 1e-6 && rms(&l) > 0.9);
        let (l, r) = pan(0.0);
        assert!(l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-5));
        // width 0 on the output folds a stereo sound to mono
        let mut p = sine_patch();
        p.nodes[1] = Node::new(2, Kind::Osc, [0.0; 2])
            .with("voices", 3.0)
            .with("spread", 1.0);
        p.nodes[2] = Node::new(3, Kind::Output, [0.0; 2]).with("width", 0.0);
        p.cables.push(Cable::new(2, "right", 3, "right", 1.0));
        let (l, r) = play(&p, 57, 4_800);
        assert_eq!(l, r);
    }

    // ----- how the patch plays

    #[test]
    fn mono_takes_the_newest_key_and_goes_back_when_it_is_let_go() {
        let p = mono(sine_patch(), 0.0, false);
        let mut m = Modular::new(&p, SR);
        m.note_on(57, 1.0);
        m.note_on(69, 1.0);
        assert_eq!(m.active_voices(), 1);
        let (l, _) = run(&mut m, 24_000);
        assert!((freq(&l) - 440.0).abs() < 1.0);
        m.note_off(69);
        let (l, _) = run(&mut m, 24_000);
        assert!((freq(&l) - 220.0).abs() < 1.0);
        m.note_off(57);
        assert!(!m.voices[0].gate);
    }

    #[test]
    fn legato_does_not_start_the_envelope_again() {
        // an envelope that decays to nothing, held: a new key either starts it again
        // (not legato) or slides on in silence (legato)
        let p = Patch {
            voices: 1,
            ..Patch::default()
        };
        let mut p = p;
        p.nodes[2] = Node::new(3, Kind::Env, [0.0; 2])
            .with("decay", 0.05)
            .with("sustain", 0.0);
        let level = |legato: bool| {
            let mut m = Modular::new(&mono(p.clone(), 0.0, legato), SR);
            m.note_on(57, 1.0);
            run(&mut m, 24_000);
            m.note_on(60, 1.0);
            rms(&run(&mut m, 2_400).0)
        };
        assert!(level(false) > 0.05);
        assert!(level(true) < 1e-3);
    }

    #[test]
    fn the_volume_scales_everything() {
        let mut p = sine_patch();
        let (full, _) = play(&p, 69, 4_800);
        p.volume = 0.5;
        let (half, _) = play(&p, 69, 4_800);
        assert!((rms(&half) * 2.0 - rms(&full)).abs() < 1e-4);
    }

    #[test]
    fn arrange_puts_each_module_after_what_feeds_it() {
        let mut p = Patch::default();
        p.arrange();
        let x = |id: u32| p.node(id).unwrap().pos[0];
        assert!(x(1) < x(2) && x(2) < x(4) && x(4) < x(5) && x(5) < x(6));
        // modules in one column do not overlap
        assert_ne!(p.node(2).unwrap().pos, p.node(3).unwrap().pos);
    }
}

#[cfg(test)]
mod control_audit {
    //! Every control of every module, turned while a note plays, changes the sound.
    use super::*;

    const SR: f64 = 48_000.0;

    /// A patch that puts module `k` (id 3) where it is heard: an oscillator into its
    /// audio inputs, the keys into its pitch and gate, its outputs to the output.
    fn rig(k: Kind) -> Patch {
        let mut nodes = vec![
            Node::new(1, Kind::Keys, [0.0; 2]),
            Node::new(2, Kind::Osc, [0.0; 2]),
            Node::new(100, Kind::Output, [0.0; 2]).with("level", 1.0),
        ];
        let mut cables = vec![Cable::new(1, "pitch", 2, "pitch", 1.0)];
        if k == Kind::Output {
            cables.push(Cable::new(2, "out", 100, "left", 1.0));
            cables.push(Cable::new(2, "right", 100, "right", 1.0));
            return Patch::new(nodes, cables, 4);
        }
        if k == Kind::Keys {
            // the keys' pitch is heard through the oscillator, their gate as it is
            cables.push(Cable::new(2, "out", 100, "left", 1.0));
            cables.push(Cable::new(1, "gate", 100, "right", 1.0));
            return Patch::new(nodes, cables, 4);
        }
        nodes.push(Node::new(3, k, [0.0; 2]));
        let spec = k.spec();
        for inp in spec.inputs {
            let from = match (k, inp.name) {
                // a sequencer plugged into a clock follows it, not its own rate
                // and a sample and hold plugged into a trigger follows it, not its rate
                (Kind::Seq, _) | (Kind::Compare, "b") | (Kind::Hold, "trig") => None,
                (_, "pitch") => Some((1, "pitch")),
                (_, "gate" | "trig") => Some((1, "gate")),
                (_, "in" | "a" | "b" | "c" | "d") => Some((2, "out")),
                _ => None,
            };
            if let Some((n, o)) = from {
                cables.push(Cable::new(n, o, 3, inp.name, 1.0));
            }
        }
        // every output is heard, on one side or the other
        for (j, o) in spec.outputs.iter().enumerate() {
            let side = if j % 2 == 0 { "left" } else { "right" };
            cables.push(Cable::new(3, o, 100, side, 1.0));
        }
        if spec.outputs.len() == 1 {
            cables.push(Cable::new(3, spec.outputs[0], 100, "right", 1.0));
        }
        Patch::new(nodes, cables, 4)
    }

    /// What else a control needs set to be heard (as the editor says when it is not).
    fn context(k: Kind, name: &str) -> &'static [(&'static str, f64)] {
        match (k, name) {
            (Kind::Osc, "pw") => &[("wave", 3.0)],
            (Kind::Osc, "morph") => &[("wave", 4.0)],
            (Kind::Osc, "detune" | "spread") => &[("voices", 3.0)],
            // every step comes round within the notes, each at its own pitch
            (Kind::Seq, "length") => &[
                ("rate", 50.0),
                ("step_1", 0.1),
                ("step_2", 0.2),
                ("step_3", 0.3),
                ("step_4", 0.4),
                ("step_5", 0.5),
                ("step_6", 0.6),
                ("step_7", 0.7),
                ("step_8", 0.8),
            ],
            (Kind::Seq, _) => &[("rate", 50.0)],
            // a and b are the same wave: a scaled tells min from max
            (Kind::Math, "op") => &[("scale", 0.5)],
            _ => &[],
        }
    }

    /// Two notes (the second a fifth up, while the first is held), then both let go,
    /// with the pitch bent and the wheel up.
    fn play(p: &Patch) -> Vec<f32> {
        let mut m = Modular::new(p, SR);
        m.set_controls(Controls {
            wheel: 0.5,
            bend: 0.5,
            pressure: 0.5,
        });
        let mut out = Vec::new();
        let mut chunk = |m: &mut Modular, n: usize| {
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            m.render(&mut l, &mut r);
            for (a, b) in l.iter().zip(&r) {
                out.push(*a);
                out.push(*b);
            }
        };
        m.note_on(60, 0.7);
        chunk(&mut m, 12_000);
        m.note_on(67, 0.9);
        chunk(&mut m, 12_000);
        m.note_off(60);
        m.note_off(67);
        chunk(&mut m, 24_000);
        out
    }

    /// The values a control is tried at: every step, or its ends and middle.
    fn tries(ps: &ParamSpec) -> Vec<f64> {
        if ps.steps > 0 {
            (0..ps.steps).map(|i| ps.min + f64::from(i)).collect()
        } else {
            vec![ps.min, ps.denormalize(0.5), ps.max]
        }
    }

    #[test]
    fn every_control_of_every_module_changes_the_sound() {
        let mut dead = Vec::new();
        for k in Kind::ALL {
            let mut base = rig(k);
            let id = match k {
                Kind::Output => 100,
                Kind::Keys => 1,
                _ => 3,
            };
            // a wide oscillator, so that stereo controls have stereo to work on
            if let Some(n) = base.nodes.iter_mut().find(|n| n.id == 2) {
                n.params.insert("voices".into(), 3.0);
            }
            // a sampler plays nothing without a sample: its own test covers it
            if k == Kind::Sampler {
                continue;
            }
            for ps in k.spec().params {
                let sounds: Vec<Vec<f32>> = tries(ps)
                    .into_iter()
                    .map(|v| {
                        let mut p = base.clone();
                        if let Some(n) = p.nodes.iter_mut().find(|n| n.id == id) {
                            for (c, cv) in context(k, ps.name) {
                                n.params.insert((*c).to_string(), *cv);
                            }
                            n.params.insert(ps.name.to_string(), v);
                        }
                        play(&p)
                    })
                    .collect();
                let differs = sounds
                    .windows(2)
                    .all(|w| w[0].iter().zip(&w[1]).any(|(a, b)| (a - b).abs() > 1e-4));
                if !differs {
                    dead.push(format!("{k:?}.{}", ps.name));
                }
            }
        }
        assert!(dead.is_empty(), "controls that change nothing: {dead:?}");
    }
}
