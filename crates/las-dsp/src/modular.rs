// SPDX-License-Identifier: GPL-3.0-or-later
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
    /// The voice's note: pitch (octaves from middle C), gate and velocity.
    Keys,
    Osc,
    Noise,
    Lfo,
    /// An ADSR envelope that follows its gate.
    Env,
    Filter,
    /// Scales its input by a control signal.
    Vca,
    /// Four inputs with a level each.
    Mix,
    /// `a * scale + b + offset`.
    Math,
    /// What the synth plays: left and right (right follows left when it has no cable).
    Output,
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
}

const fn i(name: &'static str) -> InputSpec {
    InputSpec {
        name,
        unplugged: 0.0,
    }
}

/// What a kind of module has.
#[derive(Debug)]
pub struct Spec {
    pub inputs: &'static [InputSpec],
    pub params: &'static [ParamSpec],
    pub outputs: &'static [&'static str],
}

/// The choices of an oscillator's or an LFO's `wave`.
pub const WAVES: [&str; 4] = ["sine", "triangle", "saw", "square"];
/// The choices of a filter's `mode`.
pub const FILTER_MODES: [&str; 3] = ["lowpass", "bandpass", "highpass"];

static KEYS: Spec = Spec {
    inputs: &[],
    params: &[],
    outputs: &["pitch", "gate", "velocity"],
};

static OSC: Spec = Spec {
    inputs: &[i("pitch"), i("fm")],
    params: &[
        pstep("wave", 4, 2.0),
        p("tune", -24.0, 24.0, 0.0),
        p("fine", -100.0, 100.0, 0.0),
        p("pw", 0.05, 0.95, 0.5),
        p("level", 0.0, 1.0, 1.0),
    ],
    outputs: &["out"],
};

static NOISE: Spec = Spec {
    inputs: &[],
    params: &[p("level", 0.0, 1.0, 1.0)],
    outputs: &["out"],
};

static LFO: Spec = Spec {
    inputs: &[],
    params: &[
        plog("rate", 0.01, 50.0, 2.0),
        pstep("wave", 4, 0.0),
        p("depth", 0.0, 1.0, 1.0),
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

static VCA: Spec = Spec {
    inputs: &[
        i("in"),
        InputSpec {
            name: "cv",
            unplugged: 1.0,
        },
    ],
    params: &[p("gain", 0.0, 1.0, 1.0)],
    outputs: &["out"],
};

static MIX: Spec = Spec {
    inputs: &[i("a"), i("b"), i("c"), i("d")],
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
    params: &[p("scale", -4.0, 4.0, 1.0), p("offset", -1.0, 1.0, 0.0)],
    outputs: &["out"],
};

static OUTPUT: Spec = Spec {
    inputs: &[i("left"), i("right")],
    params: &[p("level", 0.0, 1.0, 0.8)],
    outputs: &[],
};

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Keys,
        Kind::Osc,
        Kind::Noise,
        Kind::Lfo,
        Kind::Env,
        Kind::Filter,
        Kind::Vca,
        Kind::Mix,
        Kind::Math,
        Kind::Output,
    ];

    pub fn spec(self) -> &'static Spec {
        match self {
            Kind::Keys => &KEYS,
            Kind::Osc => &OSC,
            Kind::Noise => &NOISE,
            Kind::Lfo => &LFO,
            Kind::Env => &ENV,
            Kind::Filter => &FILTER,
            Kind::Vca => &VCA,
            Kind::Mix => &MIX,
            Kind::Math => &MATH,
            Kind::Output => &OUTPUT,
        }
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
}

impl Node {
    pub fn new(id: u32, kind: Kind, pos: [f32; 2]) -> Self {
        Self {
            id,
            kind,
            params: BTreeMap::new(),
            pos,
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

/// A whole patch: modules, cables and how many voices play.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Patch {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub cables: Vec<Cable>,
    #[serde(default = "voices")]
    pub voices: usize,
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

impl Default for Patch {
    /// Keys into a saw, through a lowpass and an amp that an envelope opens, which also
    /// moves the cutoff.
    fn default() -> Self {
        Self {
            nodes: vec![
                Node::new(1, Kind::Keys, [20.0, 140.0]),
                Node::new(2, Kind::Osc, [240.0, 20.0]),
                Node::new(3, Kind::Env, [240.0, 300.0]),
                Node::new(4, Kind::Filter, [460.0, 20.0]).with("cutoff", 800.0),
                Node::new(5, Kind::Vca, [680.0, 140.0]),
                Node::new(6, Kind::Output, [900.0, 140.0]),
            ],
            cables: vec![
                Cable::new(1, "pitch", 2, "pitch", 1.0),
                Cable::new(1, "gate", 3, "gate", 1.0),
                Cable::new(2, "out", 4, "in", 1.0),
                Cable::new(3, "out", 4, "cutoff", 0.35),
                Cable::new(4, "out", 5, "in", 1.0),
                Cable::new(3, "out", 5, "cv", 1.0),
                Cable::new(5, "out", 6, "left", 1.0),
            ],
            voices: voices(),
        }
    }
}

impl Patch {
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
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
    pub fn is_valid(&self, c: &Cable) -> bool {
        let (Some(a), Some(b)) = (self.node(c.from), self.node(c.to)) else {
            return false;
        };
        a.kind.output_index(&c.output).is_some() && b.kind.input_index(&c.input).is_some()
    }
}

/// The factory patches: a name and the patch.
pub fn factory() -> Vec<(&'static str, Patch)> {
    let n = Node::new;
    let c = Cable::new;
    let keys_env = |extra_nodes: Vec<Node>, extra: Vec<Cable>, voices: usize| {
        let mut p = Patch {
            nodes: extra_nodes,
            cables: extra,
            voices,
        };
        p.nodes.sort_by_key(|x| x.id);
        p
    };
    vec![
        ("Saw lead", Patch::default()),
        (
            "Wobble bass",
            keys_env(
                vec![
                    n(1, Kind::Keys, [20.0, 140.0]),
                    n(2, Kind::Osc, [240.0, 20.0]).with("tune", -12.0),
                    n(3, Kind::Osc, [240.0, 300.0])
                        .with("wave", 3.0)
                        .with("tune", -12.0)
                        .with("fine", 7.0),
                    n(4, Kind::Mix, [460.0, 20.0]),
                    n(5, Kind::Lfo, [460.0, 300.0]).with("rate", 3.0),
                    n(6, Kind::Filter, [680.0, 20.0])
                        .with("cutoff", 300.0)
                        .with("res", 0.6),
                    n(7, Kind::Env, [680.0, 300.0]).with("release", 0.1),
                    n(8, Kind::Vca, [900.0, 140.0]),
                    n(9, Kind::Output, [1120.0, 140.0]),
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
                4,
            ),
        ),
        (
            "Pluck",
            keys_env(
                vec![
                    n(1, Kind::Keys, [20.0, 140.0]),
                    n(2, Kind::Osc, [240.0, 20.0])
                        .with("wave", 3.0)
                        .with("pw", 0.3),
                    n(3, Kind::Env, [240.0, 300.0])
                        .with("decay", 0.25)
                        .with("sustain", 0.0)
                        .with("release", 0.2),
                    n(4, Kind::Filter, [460.0, 20.0])
                        .with("cutoff", 400.0)
                        .with("res", 0.3),
                    n(5, Kind::Vca, [680.0, 140.0]),
                    n(6, Kind::Output, [900.0, 140.0]),
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
                8,
            ),
        ),
        (
            "Slow pad",
            keys_env(
                vec![
                    n(1, Kind::Keys, [20.0, 140.0]),
                    n(2, Kind::Osc, [240.0, 20.0]).with("fine", -8.0),
                    n(3, Kind::Osc, [240.0, 300.0]).with("fine", 8.0),
                    n(4, Kind::Lfo, [460.0, 300.0]).with("rate", 0.2),
                    n(5, Kind::Filter, [460.0, 20.0]).with("cutoff", 1200.0),
                    n(6, Kind::Env, [680.0, 300.0])
                        .with("attack", 1.2)
                        .with("sustain", 0.9)
                        .with("release", 2.0),
                    n(7, Kind::Vca, [680.0, 20.0]),
                    n(8, Kind::Output, [900.0, 140.0]).with("level", 0.6),
                ],
                vec![
                    c(1, "pitch", 2, "pitch", 1.0),
                    c(1, "pitch", 3, "pitch", 1.0),
                    c(2, "out", 5, "in", 0.5),
                    c(3, "out", 5, "in", 0.5),
                    c(4, "out", 5, "cutoff", 0.1),
                    c(1, "gate", 6, "gate", 1.0),
                    c(5, "out", 7, "in", 1.0),
                    c(6, "out", 7, "cv", 1.0),
                    c(7, "out", 8, "left", 1.0),
                ],
                8,
            ),
        ),
        (
            "Noise sweep",
            keys_env(
                vec![
                    n(1, Kind::Keys, [20.0, 140.0]),
                    n(2, Kind::Noise, [240.0, 20.0]),
                    n(3, Kind::Env, [240.0, 300.0])
                        .with("attack", 0.5)
                        .with("decay", 1.5)
                        .with("sustain", 0.0)
                        .with("release", 0.5),
                    n(4, Kind::Filter, [460.0, 20.0])
                        .with("mode", 1.0)
                        .with("cutoff", 200.0)
                        .with("res", 0.7),
                    n(5, Kind::Vca, [680.0, 140.0]),
                    n(6, Kind::Output, [900.0, 140.0]),
                ],
                vec![
                    c(1, "gate", 3, "gate", 1.0),
                    c(2, "out", 4, "in", 1.0),
                    c(3, "out", 4, "cutoff", 0.7),
                    c(4, "out", 5, "in", 1.0),
                    c(3, "out", 5, "cv", 1.0),
                    c(5, "out", 6, "left", 1.0),
                ],
                4,
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
    /// For each input (plain inputs, then parameters): (output slot, amount).
    sources: Vec<Vec<(usize, f32)>>,
    /// The parameters' own values, from 0 to 1.
    base: Vec<f64>,
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
}

impl Graph {
    fn compile(patch: &Patch) -> Self {
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
            let base = spec
                .params
                .iter()
                .map(|p| p.normalize(node.param(p.name)))
                .collect();
            if node.kind == Kind::Output && output.is_none() {
                output = Some(nodes.len());
            }
            nodes.push(Compiled {
                kind: node.kind,
                sources,
                base,
                out: first[k],
            });
            ids.push(node.id);
        }
        Self {
            nodes,
            ids,
            slots,
            output,
        }
    }
}

// ---------------------------------------------------------------------------
// Voices
// ---------------------------------------------------------------------------

/// What a module keeps from one sample to the next.
#[derive(Clone, Copy, Default)]
struct NodeState {
    phase: f64,
    /// Envelope level, or the filter's first integrator.
    a: f64,
    /// The filter's second integrator.
    b: f64,
    /// Envelope stage: 0 idle, 1 attack, 2 decay/sustain, 3 release.
    stage: u8,
    gate: bool,
    rng: u32,
}

struct Voice {
    outs: Vec<f32>,
    states: Vec<NodeState>,
    key: u8,
    vel: f32,
    gate: bool,
    active: bool,
    /// When the voice started, to steal the oldest.
    age: u64,
    /// Samples in a row it has been quiet since its key was let go.
    quiet: usize,
}

impl Voice {
    fn new(graph: &Graph, seed: u32) -> Self {
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
            key: 0,
            vel: 0.0,
            gate: false,
            active: false,
            age: 0,
            quiet: 0,
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

/// Middle C, the pitch 0 of `Keys`.
const C4: f64 = 261.625_565_300_598_6;
/// How long a released voice must be quiet before it is free, in seconds.
const QUIET_TIME: f64 = 0.05;

/// The modular synth.
pub struct Modular {
    patch: Patch,
    graph: Graph,
    voices: Vec<Voice>,
    sr: f64,
    clock: u64,
}

impl Modular {
    pub fn new(patch: &Patch, sr: f64) -> Self {
        let graph = Graph::compile(patch);
        let count = patch.voices.clamp(1, MAX_VOICES);
        let voices = (0..count).map(|v| Voice::new(&graph, v as u32)).collect();
        Self {
            patch: patch.clone(),
            graph,
            voices,
            sr,
            clock: 0,
        }
    }

    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Take a new patch. A patch that only moved parameters keeps the voices playing;
    /// one with other modules or cables starts them again.
    pub fn set_patch(&mut self, patch: &Patch) {
        let same_shape = patch.cables == self.patch.cables
            && patch.voices == self.patch.voices
            && patch.nodes.len() == self.patch.nodes.len()
            && patch
                .nodes
                .iter()
                .zip(&self.patch.nodes)
                .all(|(a, b)| a.id == b.id && a.kind == b.kind);
        if same_shape {
            for node in &patch.nodes {
                for p in node.kind.spec().params {
                    self.set_param(node.id, p.name, node.param(p.name));
                }
            }
            self.patch = patch.clone();
        } else {
            *self = Self::new(patch, self.sr);
        }
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
        }
        if let Some(n) = self.patch.nodes.iter_mut().find(|n| n.id == node) {
            n.params.insert(name.to_string(), value);
        }
    }

    pub fn note_on(&mut self, key: u8, vel: f32) {
        self.clock += 1;
        // the same key again, a free voice, or the oldest
        let v = self
            .voices
            .iter()
            .position(|v| v.active && v.key == key)
            .or_else(|| self.voices.iter().position(|v| !v.active))
            .unwrap_or_else(|| {
                (0..self.voices.len())
                    .min_by_key(|&i| self.voices[i].age)
                    .unwrap_or(0)
            });
        let voice = &mut self.voices[v];
        if !voice.active {
            voice.outs.fill(0.0);
            for s in &mut voice.states {
                let rng = s.rng;
                *s = NodeState {
                    rng,
                    ..NodeState::default()
                };
            }
        }
        voice.key = key;
        voice.vel = vel.clamp(0.0, 1.0);
        voice.gate = true;
        voice.active = true;
        voice.age = self.clock;
        voice.quiet = 0;
    }

    pub fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.active && v.key == key) {
            v.gate = false;
        }
    }

    /// Let go of every key.
    pub fn release_all(&mut self) {
        for v in &mut self.voices {
            v.gate = false;
        }
    }

    /// Stop every voice at once.
    pub fn stop(&mut self) {
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
        let sr = self.sr;
        let graph = &self.graph;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            for (ol, or) in l.iter_mut().zip(r.iter_mut()) {
                let (a, b) = step(graph, v, sr, out_at);
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
    let spec = &c.kind.spec().params[p];
    let k = c.kind.spec().inputs.len() + p;
    let m = if c.sources[k].is_empty() {
        0.0
    } else {
        f64::from(input(c, outs, k, 0.0))
    };
    spec.denormalize(c.base[p] + m)
}

/// One sample of a voice: (left, right).
fn step(graph: &Graph, v: &mut Voice, sr: f64, out_at: usize) -> (f32, f32) {
    let mut result = (0.0, 0.0);
    for (k, c) in graph.nodes.iter().enumerate() {
        let s = &mut v.states[k];
        let outs = &mut v.outs;
        match c.kind {
            Kind::Keys => {
                outs[c.out] = (f32::from(v.key) - 60.0) / 12.0;
                outs[c.out + 1] = if v.gate { 1.0 } else { 0.0 };
                outs[c.out + 2] = v.vel;
            }
            Kind::Osc => {
                let pitch = f64::from(input(c, outs, 0, 0.0));
                let fm = f64::from(input(c, outs, 1, 0.0));
                let shape = param(c, outs, 0) as u32;
                let tune = param(c, outs, 1);
                let fine = param(c, outs, 2);
                let pw = param(c, outs, 3);
                let level = param(c, outs, 4);
                let f = (C4 * (pitch + tune / 12.0 + fine / 1200.0).exp2() * (1.0 + fm))
                    .clamp(-sr * 0.45, sr * 0.45);
                let dt = (f / sr).abs().max(1e-9);
                outs[c.out] = (wave(shape, s.phase, dt, pw) * level) as f32;
                s.phase = (s.phase + f / sr).rem_euclid(1.0);
            }
            Kind::Noise => {
                // xorshift
                let mut x = s.rng;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                s.rng = x;
                let level = param(c, outs, 0);
                outs[c.out] = ((f64::from(x) / f64::from(u32::MAX) * 2.0 - 1.0) * level) as f32;
            }
            Kind::Lfo => {
                let rate = param(c, outs, 0);
                let shape = param(c, outs, 1) as u32;
                let depth = param(c, outs, 2);
                // no correction: an LFO's steps are meant to be steps
                let w = wave(shape, s.phase, 0.0, 0.5) * depth;
                outs[c.out] = w as f32;
                outs[c.out + 1] = ((w + depth) * 0.5) as f32;
                s.phase = (s.phase + rate / sr).rem_euclid(1.0);
            }
            Kind::Env => {
                let gate = input(c, outs, 0, 0.0) > 0.5;
                let (att, dec, sus, rel) = (
                    param(c, outs, 0),
                    param(c, outs, 1),
                    param(c, outs, 2),
                    param(c, outs, 3),
                );
                if gate && !s.gate {
                    s.stage = 1;
                } else if !gate && s.gate {
                    s.stage = 3;
                }
                s.gate = gate;
                // exponential moves that get within 1% of where they go in their time
                let coef = |t: f64| (-4.6 / (t * sr).max(1.0)).exp();
                match s.stage {
                    1 => {
                        s.a += 1.0 / (att * sr).max(1.0);
                        if s.a >= 1.0 {
                            s.a = 1.0;
                            s.stage = 2;
                        }
                    }
                    2 => s.a = sus + (s.a - sus) * coef(dec),
                    3 => {
                        s.a *= coef(rel);
                        if s.a < 1e-6 {
                            s.a = 0.0;
                            s.stage = 0;
                        }
                    }
                    _ => s.a = 0.0,
                }
                outs[c.out] = s.a as f32;
            }
            Kind::Filter => {
                let x = f64::from(input(c, outs, 0, 0.0));
                let mode = param(c, outs, 0) as u32;
                let cutoff = param(c, outs, 1).min(sr * 0.45);
                let res = param(c, outs, 2);
                // a state variable filter in the topology preserving form
                let g = (PI * cutoff / sr).tan();
                let kq = 2.0 - 1.96 * res;
                let a1 = 1.0 / (1.0 + g * (g + kq));
                let v1 = a1 * (s.a + g * (x - s.b));
                let v2 = s.b + g * v1;
                s.a = 2.0 * v1 - s.a;
                s.b = 2.0 * v2 - s.b;
                let y = match mode {
                    0 => v2,
                    1 => v1,
                    _ => x - kq * v1 - v2,
                };
                outs[c.out] = if y.is_finite() {
                    y as f32
                } else {
                    s.a = 0.0;
                    s.b = 0.0;
                    0.0
                };
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
                outs[c.out] = a * param(c, outs, 0) as f32 + b + param(c, outs, 1) as f32;
            }
            Kind::Output => {
                if k == out_at {
                    let left = input(c, outs, 0, 0.0);
                    let right = if c.sources[1].is_empty() {
                        left
                    } else {
                        input(c, outs, 1, 0.0)
                    };
                    let level = param(c, outs, 0) as f32;
                    // a runaway loop is held back rather than blowing up
                    result = (
                        (left * level).clamp(-4.0, 4.0),
                        (right * level).clamp(-4.0, 4.0),
                    );
                }
            }
        }
        // a cable loop can grow without end; keep every signal finite and bounded
        for o in &mut v.outs[c.out..c.out + c.kind.spec().outputs.len()] {
            *o = if o.is_finite() {
                o.clamp(-64.0, 64.0)
            } else {
                0.0
            };
        }
    }
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
        Patch {
            nodes: vec![
                Node::new(1, Kind::Keys, [0.0; 2]),
                Node::new(2, Kind::Osc, [0.0; 2]).with("wave", 0.0),
                Node::new(3, Kind::Output, [0.0; 2]).with("level", 1.0),
            ],
            cables: vec![
                Cable::new(1, "pitch", 2, "pitch", 1.0),
                Cable::new(2, "out", 3, "left", 1.0),
            ],
            voices: 4,
        }
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
        // a new cable starts again
        p.cables.pop();
        m.set_patch(&p);
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn every_factory_patch_plays_and_has_only_good_cables() {
        for (name, p) in factory() {
            assert!(p.cables.iter().all(|c| p.is_valid(c)), "{name}");
            let mut m = Modular::new(&p, SR);
            m.note_on(57, 0.8);
            let (l, r) = run(&mut m, 48_000);
            assert!(rms(&l) > 0.01 && rms(&r) > 0.01, "{name} {}", rms(&l));
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 4.0));
        }
    }

    #[test]
    fn parameters_map_their_ranges() {
        let c = Kind::Filter.param("cutoff").unwrap();
        assert!((c.denormalize(c.normalize(440.0)) - 440.0).abs() < 1e-9);
        assert!((c.denormalize(0.5) - (20.0f64 * 20_000.0).sqrt()).abs() < 1e-6);
        let w = Kind::Osc.param("wave").unwrap();
        assert_eq!(w.denormalize(0.4), 1.0);
        assert_eq!(Kind::Vca.input_index("gain"), Some(2));
        assert_eq!(Kind::Vca.input_index("cv"), Some(1));
    }
}
