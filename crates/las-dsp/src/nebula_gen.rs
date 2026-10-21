// SPDX-License-Identifier: GPL-3.0-or-later
//! Nebula's generated presets. Each family is one sound design (a supersaw lead, an
//! acid bass, a chord stack) with a few parameters, and every name of the family
//! plays a variation of it: other wavetables, filter settings, envelopes, layers and
//! effects. They are added after the presets exported from the web version.

use crate::presets::Preset;
use serde_json::{json, Value};

/// One oscillator of a layer's patch.
#[derive(Clone, Copy)]
struct Osc {
    table: &'static str,
    pos: f64,
    voices: u32,
    detune: f64,
    spread: f64,
    oct: i32,
    semi: i32,
    level: f64,
}

fn osc(table: &'static str, level: f64) -> Osc {
    Osc {
        table,
        pos: 0.0,
        voices: 1,
        detune: 0.2,
        spread: 0.6,
        oct: 0,
        semi: 0,
        level,
    }
}

impl Osc {
    fn pos(mut self, pos: f64) -> Self {
        self.pos = pos;
        self
    }

    fn uni(mut self, voices: u32, detune: f64) -> Self {
        self.voices = voices;
        self.detune = detune;
        self.spread = 1.0;
        self
    }

    fn oct(mut self, oct: i32) -> Self {
        self.oct = oct;
        self
    }

    fn semi(mut self, semi: i32) -> Self {
        self.semi = semi;
        self
    }

    fn json(&self) -> Value {
        json!({
            "on": true, "table": self.table, "pos": self.pos, "voices": self.voices,
            "detune": self.detune, "spread": self.spread, "oct": self.oct,
            "semi": self.semi, "level": self.level,
        })
    }
}

/// A layer's synth patch: oscillators, one filter, the amp and filter envelopes,
/// and optional modulation.
#[derive(Clone)]
struct Patch {
    osc: Vec<Osc>,
    /// Filter type, cutoff, resonance, envelope amount (octaves), key tracking.
    filter: Option<(&'static str, f64, f64, f64, f64)>,
    /// Attack, decay, sustain, release of the amp.
    amp: [f64; 4],
    /// The filter envelope (env 2).
    fenv: [f64; 4],
    /// A pitch envelope (env 3) and its depth (1 is two octaves).
    pitch: Option<([f64; 4], f64)>,
    /// An LFO: shape, rate in Hz (or beats when synced), synced, destination, amount.
    lfo: Option<(&'static str, f64, bool, &'static str, f64)>,
    noise: f64,
    sub: f64,
    mono: bool,
    glide: f64,
    poly: u32,
}

fn patch(osc: Vec<Osc>, amp: [f64; 4]) -> Patch {
    Patch {
        osc,
        filter: None,
        amp,
        fenv: [0.003, 0.4, 0.0, 0.3],
        pitch: None,
        lfo: None,
        noise: 0.0,
        sub: 0.0,
        mono: false,
        glide: 0.0,
        poly: 8,
    }
}

impl Patch {
    fn filter(mut self, kind: &'static str, cutoff: f64, res: f64, env: f64) -> Self {
        self.filter = Some((kind, cutoff, res, env, 0.0));
        self
    }

    fn keytrack(mut self, k: f64) -> Self {
        if let Some(f) = &mut self.filter {
            f.4 = k;
        }
        self
    }

    fn fenv(mut self, e: [f64; 4]) -> Self {
        self.fenv = e;
        self
    }

    fn pitch(mut self, e: [f64; 4], amt: f64) -> Self {
        self.pitch = Some((e, amt));
        self
    }

    fn lfo(
        mut self,
        shape: &'static str,
        rate: f64,
        sync: bool,
        dest: &'static str,
        amt: f64,
    ) -> Self {
        self.lfo = Some((shape, rate, sync, dest, amt));
        self
    }

    fn noise(mut self, level: f64) -> Self {
        self.noise = level;
        self
    }

    fn sub(mut self, level: f64) -> Self {
        self.sub = level;
        self
    }

    fn mono(mut self, glide: f64) -> Self {
        self.mono = true;
        self.glide = glide;
        self
    }

    fn json(&self) -> Value {
        let mut osc: Vec<Value> = self.osc.iter().take(4).map(Osc::json).collect();
        while osc.len() < 4 {
            osc.push(json!({ "on": false }));
        }
        let filter = match self.filter {
            Some((kind, cutoff, res, env, key)) => json!({
                "on": true, "type": kind, "cutoff": cutoff, "res": res,
                "envAmt": env, "keytrack": key,
            }),
            None => json!({ "on": false }),
        };
        let env = |e: [f64; 4]| json!({ "a": e[0], "h": 0, "d": e[1], "s": e[2], "r": e[3] });
        let mut envs = vec![env(self.amp), env(self.fenv)];
        let mut mods = Vec::new();
        if let Some((e, amt)) = self.pitch {
            envs.push(env(e));
            mods.push(json!({ "src": "env3", "dest": "pitch", "amt": amt }));
        }
        let mut p = json!({
            "osc": osc,
            "filters": [filter, { "on": false }],
            "env": envs,
            "voice": { "poly": self.poly, "mono": self.mono, "legato": self.mono, "glide": self.glide },
        });
        if let Some((shape, rate, sync, dest, amt)) = self.lfo {
            p["lfo"] = json!([{ "shape": shape, "rate": rate, "sync": sync, "div": rate }]);
            mods.push(json!({ "src": "lfo1", "dest": dest, "amt": amt }));
        }
        if !mods.is_empty() {
            p["mod"] = json!(mods);
        }
        if self.noise > 0.0 {
            p["noise"] = json!({ "on": true, "color": "white", "level": self.noise, "route": "A" });
        }
        if self.sub > 0.0 {
            p["sub"] = json!({ "on": true, "wave": "sine", "oct": -1, "level": self.sub, "route": "direct" });
        }
        p
    }
}

/// A layer: its name, level, pan, transpose and patch.
fn layer(name: &str, level: f64, pan: f64, transpose: i32, p: &Patch) -> Value {
    json!({
        "on": true, "name": name, "level": level, "pan": pan,
        "transpose": transpose, "patch": p.json(),
    })
}

/// The built-in effects of a preset: chorus mix, delay wet and time (beats),
/// reverb wet and decay. A zero wet leaves the effect out.
#[derive(Clone, Copy, Default)]
struct Fx {
    chorus: f64,
    delay: f64,
    time: f64,
    reverb: f64,
    decay: f64,
}

fn fx(chorus: f64, delay: f64, time: f64, reverb: f64, decay: f64) -> Fx {
    Fx {
        chorus,
        delay,
        time,
        reverb,
        decay,
    }
}

impl Fx {
    fn json(&self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.chorus > 0.0 {
            out.push(json!({ "type": "las-chorus", "on": true, "state": { "mix": self.chorus } }));
        }
        if self.delay > 0.0 {
            out.push(json!({ "type": "las-delay", "on": true, "state": { "wet": self.delay, "time": self.time, "feedback": 0.35 } }));
        }
        if self.reverb > 0.0 {
            out.push(json!({ "type": "las-reverb", "on": true, "state": { "wet": self.reverb, "decay": self.decay } }));
        }
        out
    }
}

/// A whole preset state.
fn state(layers: Vec<Value>, fx: Fx, macros: Value) -> Value {
    json!({ "layers": layers, "fx": fx.json(), "macros": macros, "master": 0.8 })
}

fn macros(cutoff: f64, res: f64, attack: f64, release: f64) -> Value {
    json!({ "cutoff": cutoff, "res": res, "attack": attack, "release": release,
            "drive": 0.0, "width": 0.5, "detune": 0.5 })
}

fn pick<T: Copy>(xs: &[T], i: usize) -> T {
    xs[i % xs.len()]
}

/// A family: a category, a name prefix, the names and the sound for each index.
fn family(
    out: &mut Vec<Preset>,
    category: &str,
    prefix: &str,
    names: &[&str],
    make: impl Fn(usize) -> Value,
) {
    for (i, n) in names.iter().enumerate() {
        let state = make(i);
        out.push(Preset {
            name: format!("{prefix} {n}"),
            category: category.to_string(),
            patch: Value::Object(Default::default()),
            state,
        });
    }
}

/// Every generated preset, in browser order.
pub fn presets() -> Vec<Preset> {
    let mut out = Vec::new();
    basses(&mut out);
    leads(&mut out);
    pads(&mut out);
    plucks(&mut out);
    keys(&mut out);
    pianos(&mut out);
    strings(&mut out);
    brass(&mut out);
    bells(&mut out);
    arps(&mut out);
    synths(&mut out);
    chords(&mut out);
    effects(&mut out);
    drums(&mut out);
    gates(&mut out);
    out
}

const DRY: Fx = Fx {
    chorus: 0.0,
    delay: 0.0,
    time: 0.5,
    reverb: 0.0,
    decay: 1.0,
};

fn basses(out: &mut Vec<Preset>) {
    let c = "Bass";
    family(
        out,
        c,
        "BS",
        &[
            "Velvet Sub",
            "Rubber Floor",
            "Midnight Mover",
            "Basement Pulse",
            "Analog Thump",
            "Smooth Operator",
            "Low Rider",
            "Round Bottom",
        ],
        |i| {
            let t = pick(&["saw", "square", "analog", "pulse25"], i);
            let p = patch(
                vec![osc(t, 0.7).uni(pick(&[1, 2, 3], i), 0.12)],
                [0.002, 0.5, 0.8, 0.12],
            )
            .filter(
                pick(&["lp24", "ladder", "lp12"], i),
                pick(&[220.0, 320.0, 450.0, 600.0], i),
                0.2 + 0.05 * (i % 3) as f64,
                3.0,
            )
            .fenv([0.001, pick(&[0.18, 0.3, 0.45], i), 0.15, 0.15])
            .sub(0.6)
            .mono(0.03);
            state(
                vec![layer("Bass", 0.85, 0.0, -12, &p)],
                DRY,
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "BS",
        &[
            "Reese Tunnel",
            "Liquid Reese",
            "Steel Reese",
            "Wide Reese Cloud",
            "Neuro Grind",
            "Reese Hurricane",
        ],
        |i| {
            let p = patch(
                vec![
                    osc("reese", 0.7)
                        .pos(0.1 * i as f64)
                        .uni(2, 0.25 + 0.05 * i as f64),
                    osc("saw", 0.4).uni(2, 0.3).oct(-1),
                ],
                [0.003, 0.4, 0.9, 0.2],
            )
            .filter("lp24", 700.0 + 250.0 * i as f64, 0.25, 1.0)
            .lfo("sine", pick(&[0.2, 0.35, 0.5], i), false, "osc0.pos", 0.3)
            .mono(0.05);
            let sub = patch(vec![osc("sine", 0.8)], [0.003, 0.3, 1.0, 0.15]);
            let mut s = state(
                vec![
                    layer("Reese", 0.75, 0.0, -12, &p),
                    layer("Sub", 0.6, 0.0, -24, &sub),
                ],
                fx(if i % 2 == 0 { 0.2 } else { 0.0 }, 0.0, 0.5, 0.08, 1.2),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            s["macros"]["drive"] = json!(0.2 + 0.1 * (i % 3) as f64);
            s
        },
    );
    family(
        out,
        c,
        "BS",
        &[
            "Acid Rain",
            "Squelch Box",
            "Acid Serpent",
            "Rubber Acid",
            "Acid Fountain",
            "Night Acid",
        ],
        |i| {
            let p = patch(
                vec![osc(pick(&["saw", "square"], i), 0.75)],
                [0.001, 0.3, 0.7, 0.08],
            )
            .filter(
                "ladder",
                pick(&[300.0, 380.0, 500.0], i),
                0.7 + 0.04 * i as f64,
                5.0,
            )
            .fenv([0.001, pick(&[0.15, 0.22, 0.3], i), 0.0, 0.1])
            .mono(0.06);
            let mut s = state(
                vec![layer("Acid", 0.85, 0.0, -12, &p)],
                fx(0.0, pick(&[0.0, 0.18, 0.25], i), 0.75, 0.1, 1.5),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            s["macros"]["drive"] = json!(0.3);
            s
        },
    );
    family(
        out,
        c,
        "BS",
        &[
            "Organ Donor Bass",
            "Garage Bounce",
            "Funk Finger",
            "Disco Octave",
            "Electro Punch",
            "Tight Kick Bass",
        ],
        |i| {
            let t = pick(&["organ", "pulse25", "saw", "square", "digital", "fm"], i);
            let p = patch(
                vec![osc(t, 0.7), osc("sine", 0.4).oct(-1)],
                [0.001, pick(&[0.2, 0.3, 0.45], i), 0.0, 0.1],
            )
            .filter("lp12", 900.0, 0.15, 3.0)
            .fenv([0.001, 0.12, 0.0, 0.1]);
            let mut layers = vec![layer("Body", 0.8, 0.0, -12, &p)];
            if i == 3 {
                layers.push(layer("Octave", 0.45, 0.0, 0, &p));
            }
            state(
                layers,
                fx(0.0, 0.0, 0.5, 0.05, 1.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "BS",
        &[
            "Wobble Factory",
            "Dub Wobble",
            "Half Step Wub",
            "Triplet Shudder",
            "Lazy Wub",
            "Robot Jaw",
        ],
        |i| {
            let rate = pick(&[0.25, 0.5, 1.0, 0.333, 2.0, 0.125], i);
            let p = patch(
                vec![
                    osc(pick(&["growl", "saw", "fm", "digital"], i), 0.7).uni(3, 0.2),
                    osc("square", 0.4).oct(-1),
                ],
                [0.003, 0.3, 1.0, 0.15],
            )
            .filter("lp24", 500.0, 0.45, 0.0)
            .lfo("sine", rate, true, "f0.cutoff", 0.45)
            .sub(0.5)
            .mono(0.02);
            let mut s = state(
                vec![layer("Wobble", 0.8, 0.0, -12, &p)],
                fx(0.0, 0.0, 0.5, 0.08, 1.0),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            s["macros"]["drive"] = json!(0.35);
            s
        },
    );
}

fn leads(out: &mut Vec<Preset>) {
    let c = "Leads";
    family(
        out,
        c,
        "LD",
        &[
            "Stadium Lights",
            "Euphoria",
            "Neon Anthem",
            "Skyline Saw",
            "Hands Up",
            "Afterglow Lead",
        ],
        |i| {
            let saw = patch(
                vec![osc("saw", 0.7).uni(pick(&[7, 9, 11], i), 0.4 + 0.03 * i as f64)],
                [0.004, 0.5, 0.85, 0.35],
            )
            .filter("lp24", pick(&[6000.0, 8000.0, 10000.0], i), 0.1, 0.0);
            let top = patch(
                vec![osc(pick(&["harmonics", "square", "pulse25"], i), 0.6).uni(3, 0.2)],
                [0.004, 0.5, 0.8, 0.35],
            )
            .filter("lp12", 9000.0, 0.1, 0.0);
            state(
                vec![
                    layer("Supersaw", 0.75, 0.0, 0, &saw),
                    layer("Octave", 0.35, 0.0, 12, &top),
                ],
                fx(0.0, 0.22, pick(&[0.75, 0.5], i), 0.3, pick(&[3.0, 4.0], i)),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "LD",
        &[
            "Chip Hero",
            "Pulse Runner",
            "Retro Glide",
            "Gameboy Dream",
            "Square Talk",
            "Hollow Whistle",
        ],
        |i| {
            let p = patch(
                vec![osc(pick(&["square", "pulse25", "pulse10", "pwm"], i), 0.7)],
                [0.002, 0.3, 0.9, 0.15],
            )
            .filter("lp12", pick(&[3000.0, 5000.0, 7000.0], i), 0.1, 0.0)
            .lfo(
                "triangle",
                5.5,
                false,
                "osc0.pitch",
                if i % 2 == 0 { 0.008 } else { 0.0 },
            )
            .mono(pick(&[0.0, 0.08, 0.15], i));
            state(
                vec![layer("Mono", 0.8, 0.0, 0, &p)],
                fx(0.0, 0.25, 0.5, 0.15, 1.8),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "LD",
        &[
            "Sync Razor",
            "Screamer",
            "Hard Sync Siren",
            "Digital Bite",
            "Laser Hook",
            "Metal Tongue",
        ],
        |i| {
            let p = patch(
                vec![osc(pick(&["sync", "digital", "fold", "phasedist"], i), 0.7)
                    .pos(0.3 + 0.1 * (i % 4) as f64)
                    .uni(2, 0.15)],
                [0.002, 0.4, 0.85, 0.2],
            )
            .filter("lp24", 5000.0, 0.25, 2.0)
            .lfo("saw", 0.5, true, "osc0.pos", 0.35)
            .mono(0.04);
            let mut s = state(
                vec![layer("Edge", 0.75, 0.0, 0, &p)],
                fx(0.0, 0.2, 0.75, 0.18, 2.0),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            s["macros"]["drive"] = json!(0.25 + 0.05 * i as f64);
            s
        },
    );
    family(
        out,
        c,
        "LD",
        &[
            "Silk Flute",
            "Glass Whistle",
            "Ocarina Dream",
            "Soft Sine Lead",
            "Moonlight Lead",
            "Breathy Solo",
        ],
        |i| {
            let p = patch(
                vec![osc(pick(&["sine", "triangle", "formant"], i), 0.75)],
                [0.05 + 0.02 * i as f64, 0.4, 0.9, 0.4],
            )
            .lfo("sine", 5.0, false, "osc0.pitch", 0.006)
            .noise(0.04 + 0.02 * (i % 3) as f64)
            .mono(0.06);
            state(
                vec![layer("Breath", 0.85, 0.0, 12, &p)],
                fx(0.15, 0.2, 0.75, 0.35, 3.5),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn pads(out: &mut Vec<Preset>) {
    let c = "Pads";
    family(
        out,
        c,
        "PD",
        &[
            "Velvet Horizon",
            "Golden Hour",
            "Analog Dawn",
            "Woolen Sky",
            "Slow Tide Rising",
            "Haze",
            "Memory Lane",
        ],
        |i| {
            let p = patch(
                vec![osc(pick(&["saw", "analog", "pwm"], i), 0.6).uni(pick(&[4, 6, 8], i), 0.3)],
                [0.6 + 0.2 * (i % 3) as f64, 1.0, 0.85, 1.8],
            )
            .filter("lp24", pick(&[1200.0, 1800.0, 2500.0], i), 0.15, 1.0)
            .fenv([0.8, 2.0, 0.4, 1.5]);
            let air = patch(vec![osc("triangle", 0.5).oct(1)], [1.2, 1.0, 0.8, 2.0]);
            state(
                vec![
                    layer("Analog", 0.75, -0.15, 0, &p),
                    layer("Air", 0.3, 0.15, 0, &air),
                ],
                fx(0.3, 0.0, 0.5, 0.4, 5.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "PD",
        &[
            "Nebula Drift",
            "Polar Lights",
            "Tidal Glow",
            "Star Nursery",
            "Event Horizon",
            "Shifting Sands",
            "Deep Field",
        ],
        |i| {
            let t = pick(
                &[
                    "digital",
                    "harmonics",
                    "formant",
                    "fm",
                    "phasedist",
                    "bell",
                    "odd",
                ],
                i,
            );
            let a = patch(vec![osc(t, 0.6).uni(4, 0.25)], [1.0, 1.0, 0.9, 2.5])
                .filter("lp24", 3000.0, 0.15, 0.0)
                .lfo("triangle", 0.1 + 0.04 * i as f64, false, "osc0.pos", 0.5);
            let b = patch(
                vec![osc(pick(&["saw", "choir", "sine"], i), 0.5).uni(3, 0.3)],
                [1.5, 1.0, 0.8, 3.0],
            )
            .filter("lp12", 2000.0, 0.1, 0.0)
            .lfo("sine", 0.07, false, "f0.cutoff", 0.15);
            let mut layers = vec![
                layer("Motion", 0.7, -0.3, 0, &a),
                layer("Bed", 0.5, 0.3, pick(&[0, 12, -12, 7], i), &b),
            ];
            if i % 3 == 0 {
                let sub = patch(vec![osc("sine", 0.6)], [1.0, 1.0, 1.0, 2.0]);
                layers.push(layer("Low", 0.35, 0.0, -12, &sub));
            }
            state(
                layers,
                fx(0.2, 0.15, 0.75, 0.5, 7.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "PD",
        &[
            "Angel Hall",
            "Cathedral Air",
            "Whisper Choir",
            "Ghost Voices",
            "Vocal Mist",
        ],
        |i| {
            let p = patch(
                vec![osc("choir", 0.7).pos(0.2 * i as f64).uni(4, 0.2)],
                [0.8, 1.0, 0.9, 2.5],
            )
            .filter("formant", pick(&[700.0, 900.0, 500.0], i), 0.35, 0.0)
            .lfo("sine", 0.15, false, "osc0.pos", 0.25);
            let air = patch(vec![osc("sine", 0.4)], [1.0, 1.0, 0.8, 2.0])
                .noise(0.1)
                .filter("bp12", 4000.0, 0.2, 0.0);
            state(
                vec![
                    layer("Choir", 0.8, 0.0, 0, &p),
                    layer("Breath", 0.25, 0.0, 12, &air),
                ],
                fx(0.2, 0.0, 0.5, 0.55, 6.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "PD",
        &["Abyss", "Black Ice", "Undertow", "Cold Storage", "Iron Sky"],
        |i| {
            let p = patch(
                vec![
                    osc(pick(&["reese", "growl", "saw", "odd", "digital"], i), 0.6).uni(3, 0.35),
                    osc("sine", 0.5).oct(-1),
                ],
                [1.5, 1.0, 1.0, 3.0],
            )
            .filter("lp24", 600.0 + 150.0 * i as f64, 0.35, 0.0)
            .lfo("sine", 0.05, false, "f0.cutoff", 0.25);
            let mut s = state(
                vec![layer("Drone", 0.8, 0.0, -12, &p)],
                fx(0.0, 0.2, 1.0, 0.5, 8.0),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            s["macros"]["drive"] = json!(0.15);
            s
        },
    );
    family(
        out,
        c,
        "PD",
        &["Crystal Lake", "Ice Palace", "Glass Canopy", "Prism Air"],
        |i| {
            let a = patch(
                vec![osc(pick(&["bell", "harmonics"], i), 0.5).pos(0.5)],
                [0.4, 1.5, 0.6, 3.0],
            );
            let b = patch(vec![osc("saw", 0.5).uni(6, 0.3)], [1.0, 1.0, 0.8, 2.5])
                .filter("hp12", 600.0, 0.1, 0.0);
            state(
                vec![
                    layer("Glass", 0.55, -0.4, 12, &a),
                    layer("Shine", 0.5, 0.4, 0, &b),
                ],
                fx(0.3, 0.25, 0.75, 0.5, 6.0),
                macros(0.55, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn plucks(out: &mut Vec<Preset>) {
    let c = "Plucks";
    family(
        out,
        c,
        "PL",
        &[
            "Snap Pluck",
            "Club Pluck",
            "Rain Drops",
            "Firefly",
            "Pop Pluck",
            "Stab Pluck",
            "Shimmer Pluck",
        ],
        |i| {
            let p = patch(
                vec![osc(pick(&["saw", "square", "analog"], i), 0.7).uni(pick(&[3, 5, 7], i), 0.3)],
                [0.001, pick(&[0.25, 0.35, 0.5], i), 0.0, 0.3],
            )
            .filter("lp24", pick(&[400.0, 600.0, 900.0], i), 0.2, 5.0)
            .fenv([0.001, pick(&[0.15, 0.22, 0.3], i), 0.0, 0.2]);
            let mut layers = vec![layer("Pluck", 0.8, 0.0, 0, &p)];
            if i % 2 == 1 {
                layers.push(layer("Octave", 0.3, 0.0, 12, &p));
            }
            state(
                layers,
                fx(0.0, 0.3, pick(&[0.75, 0.5, 0.375], i), 0.3, 2.5),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "PL",
        &[
            "Wood Marimba",
            "Steel Drum Pluck",
            "Bubble Pluck",
            "Toy Box",
            "Kalimba Sun",
            "Pebble",
            "Water Drop",
        ],
        |i| {
            let p = patch(
                vec![
                    osc(pick(&["sine", "triangle", "bell"], i), 0.8),
                    osc("sine", 0.25).oct(2).semi(pick(&[0, 7, 4], i)),
                ],
                [0.001, 0.3 + 0.05 * i as f64, 0.0, 0.3],
            )
            .pitch(
                [0.001, 0.03, 0.0, 0.05],
                if i == 2 || i == 6 { 0.3 } else { 0.0 },
            );
            state(
                vec![layer("Mallet", 0.85, 0.0, 0, &p)],
                fx(0.0, 0.2, 0.5, 0.25, 2.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "PL",
        &[
            "Koto Garden",
            "Nylon Pick",
            "Harp Glide",
            "Pizzicato Synth",
            "Mandolin Dream",
            "Banjo Bot",
        ],
        |i| {
            let p = patch(
                vec![
                    osc(pick(&["saw", "pulse25", "triangle", "pulse10"], i), 0.7),
                    osc("triangle", 0.3).oct(1),
                ],
                [0.001, 0.6, 0.0, 0.5],
            )
            .filter("lp12", 1500.0, 0.2, 3.0)
            .keytrack(0.5)
            .fenv([0.001, 0.2, 0.1, 0.3]);
            state(
                vec![layer("String", 0.85, 0.0, 0, &p)],
                fx(0.0, 0.0, 0.5, 0.3, 2.5),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn keys(out: &mut Vec<Preset>) {
    let c = "Keys";
    family(
        out,
        c,
        "KY",
        &[
            "Velvet EP",
            "Suitcase Keys",
            "Tine Dream",
            "Lo-Fi Rhodes",
            "Wurly Shine",
            "Jazz Club EP",
        ],
        |i| {
            let body = patch(
                vec![osc("sine", 0.8), osc("triangle", 0.2).oct(1)],
                [0.002, 1.8, 0.2, 0.5],
            );
            let tine = patch(
                vec![osc("bell", 0.4).pos(0.3 + 0.1 * (i % 3) as f64)],
                [0.001, 0.5, 0.0, 0.4],
            );
            state(
                vec![
                    layer("Body", 0.8, 0.0, 0, &body),
                    layer("Tine", pick(&[0.3, 0.4, 0.5], i), 0.0, 12, &tine),
                ],
                fx(pick(&[0.35, 0.2, 0.45], i), 0.0, 0.5, 0.2, 2.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "KY",
        &[
            "Gospel Drawbar",
            "Rock Organ",
            "Church Pipes",
            "Combo Organ",
            "Percussive Organ",
        ],
        |i| {
            let p = patch(
                vec![
                    osc("organ", 0.7).pos(0.2 * i as f64),
                    osc("sine", 0.3).oct(1),
                ],
                [0.005, 0.2, 1.0, 0.08],
            )
            .lfo("sine", 6.0, false, "osc0.level", 0.08);
            let mut layers = vec![layer("Organ", 0.8, 0.0, 0, &p)];
            if i == 4 {
                let click = patch(vec![osc("sine", 0.5)], [0.001, 0.15, 0.0, 0.1]);
                layers.push(layer("Perc", 0.4, 0.0, 19, &click));
            }
            let mut s = state(
                layers,
                fx(0.3, 0.0, 0.5, if i == 2 { 0.5 } else { 0.15 }, 3.0),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            if i == 1 {
                s["macros"]["drive"] = json!(0.35);
            }
            s
        },
    );
    family(
        out,
        c,
        "KY",
        &[
            "Funky Clav",
            "Digital Keys 86",
            "FM Glass Keys",
            "Bright Synth Keys",
            "Hollow Keys",
        ],
        |i| {
            let p = patch(
                vec![osc(
                    pick(&["pulse10", "fm", "digital", "saw", "odd"], i),
                    0.7,
                )],
                [0.001, 0.8, 0.3, 0.3],
            )
            .filter("lp12", 2500.0, 0.2, 2.0)
            .fenv([0.001, 0.3, 0.2, 0.2]);
            state(
                vec![layer("Keys", 0.8, 0.0, 0, &p)],
                fx(0.2, 0.15, 0.5, 0.2, 2.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn pianos(out: &mut Vec<Preset>) {
    family(
        out,
        "Piano",
        "PN",
        &[
            "Grand Hall",
            "Upright Room",
            "Felt Piano",
            "Ballad Piano",
            "Bright Pop Piano",
            "Dark Concert",
            "Honky Tonk",
            "Piano Pad Layer",
            "Dusty Keys Piano",
            "Toy Piano",
            "Stage Piano",
            "House Piano",
            "Lullaby Piano",
            "Cinematic Piano",
        ],
        |i| {
            let bright = pick(&[0.5, 0.35, 0.2, 0.4, 0.7, 0.25, 0.55], i);
            let hammer = patch(
                vec![osc("harmonics", 0.6).pos(bright), osc("triangle", 0.4)],
                [0.001, 2.5, 0.0, 0.6],
            )
            .filter("lp12", 1500.0 + 4000.0 * bright, 0.05, 2.0)
            .keytrack(0.6)
            .fenv([0.001, 0.6, 0.2, 0.5]);
            let body = patch(vec![osc("sine", 0.7)], [0.001, 3.0, 0.0, 0.7]);
            let mut layers = vec![
                layer("Hammer", 0.7, 0.0, 0, &hammer),
                layer("Body", 0.4, 0.0, 0, &body),
            ];
            if i == 6 {
                layers.push(layer("Detuned", 0.4, 0.2, 0, &hammer.clone()));
                layers[2]["patch"]["osc"][0]["detune"] = json!(0.3);
                layers[2]["patch"]["osc"][0]["voices"] = json!(2);
            }
            if i == 7 || i == 13 {
                let pad = patch(vec![osc("saw", 0.4).uni(5, 0.3)], [0.8, 1.0, 0.8, 2.0])
                    .filter("lp24", 1500.0, 0.1, 0.0);
                layers.push(layer("Pad", 0.35, 0.0, 0, &pad));
            }
            if i == 9 {
                let bell = patch(vec![osc("bell", 0.5)], [0.001, 0.8, 0.0, 0.4]);
                layers.push(layer("Toy", 0.4, 0.0, 24, &bell));
            }
            let room = pick(&[0.35, 0.2, 0.25, 0.3, 0.2, 0.4, 0.15], i);
            state(
                layers,
                fx(0.0, 0.0, 0.5, room, 1.5 + 2.0 * room),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn strings(out: &mut Vec<Preset>) {
    let c = "Strings";
    family(
        out,
        c,
        "ST",
        &[
            "Silk Ensemble",
            "Chamber Strings",
            "Movie Strings",
            "Slow Violins",
            "Warm Cellos",
            "Hollywood Swell",
        ],
        |i| {
            let p = patch(
                vec![osc("saw", 0.6).uni(6, 0.25), osc("pwm", 0.3).uni(2, 0.2)],
                [0.3 + 0.15 * (i % 3) as f64, 1.0, 0.9, 1.2],
            )
            .filter("lp24", pick(&[2500.0, 3500.0, 1800.0], i), 0.1, 0.0)
            .lfo("sine", 5.0, false, "osc0.pitch", 0.004);
            let mut layers = vec![layer("Ensemble", 0.8, 0.0, 0, &p)];
            if i == 4 {
                layers.push(layer("Low", 0.5, 0.0, -12, &p));
            }
            if i == 2 || i == 5 {
                layers.push(layer("High", 0.35, 0.2, 12, &p));
            }
            state(
                layers,
                fx(0.25, 0.0, 0.5, 0.45, 4.5),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "ST",
        &[
            "Solina Dream",
            "String Machine",
            "Eighties Strings",
            "Glide Strings",
            "Disco Strings",
        ],
        |i| {
            let mut p = patch(
                vec![osc(pick(&["saw", "pwm", "analog"], i), 0.7).uni(4, 0.35)],
                [0.15, 1.0, 1.0, 0.8],
            )
            .filter("lp12", 4000.0, 0.1, 0.0);
            if i == 3 {
                p = p.mono(0.12);
            }
            state(
                vec![layer("Machine", 0.8, 0.0, 0, &p)],
                fx(0.55, 0.0, 0.5, 0.3, 3.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
    family(
        out,
        c,
        "ST",
        &["Spiccato Synth", "Staccato Drive", "Shiver Bows"],
        |i| {
            let mut p = patch(vec![osc("saw", 0.7).uni(5, 0.25)], [0.005, 0.25, 0.0, 0.2])
                .filter("lp24", 2500.0, 0.15, 2.0)
                .fenv([0.001, 0.2, 0.0, 0.2]);
            if i == 2 {
                p.amp = [0.1, 1.0, 1.0, 0.6];
                p = p.lfo("sine", 0.0625, true, "amp", 0.5);
            }
            let mut s = state(
                vec![layer("Bow", 0.85, 0.0, 0, &p)],
                fx(0.0, 0.0, 0.5, 0.35, 3.0),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            if i == 1 {
                s["macros"]["drive"] = json!(0.4);
            }
            s
        },
    );
}

fn brass(out: &mut Vec<Preset>) {
    family(
        out,
        "Brass",
        "BR",
        &[
            "Big Band Stab",
            "Horn Section",
            "Soft French Horn",
            "Royal Herald",
            "Brass Swell",
            "Swing Section",
            "Trumpet Lead",
            "Epic Horns",
            "Funk Horns",
            "Low Brass",
            "Synth Horns 84",
            "Muted Trumpet",
        ],
        |i| {
            let soft = matches!(i, 2 | 4 | 11);
            let p = patch(
                vec![osc("saw", 0.7).uni(pick(&[3, 4, 2], i), 0.18)],
                [if i == 4 { 0.6 } else { 0.03 }, 0.5, 0.8, 0.25],
            )
            .filter(
                if soft { "lp12" } else { "lp24" },
                if soft { 700.0 } else { 1100.0 },
                0.1,
                3.0,
            )
            .fenv([0.06, 0.4, 0.4, 0.3]);
            let mut layers = vec![layer("Horns", 0.8, 0.0, 0, &p)];
            if matches!(i, 0 | 3 | 7 | 8) {
                layers.push(layer("Fifth", 0.45, -0.2, 7, &p));
                layers.push(layer("Octave", 0.4, 0.2, 12, &p));
            }
            if i == 9 {
                layers.push(layer("Tuba", 0.6, 0.0, -12, &p));
            }
            state(
                layers,
                fx(0.0, 0.0, 0.5, if i == 7 { 0.5 } else { 0.25 }, 3.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn bells(out: &mut Vec<Preset>) {
    family(
        out,
        "Bells",
        "BL",
        &[
            "Chime Garden",
            "Temple Bell",
            "Pocket Glock",
            "Celesta Dream",
            "Crystal Bells",
            "Porch Chimes",
            "Tubular Bell",
            "Ice Bell",
            "Gamelan",
            "Velvet Vibes",
            "Fairy Dust",
            "Carillon",
            "Magic Box",
            "Starlight Bells",
        ],
        |i| {
            let decay = pick(&[1.5, 4.0, 0.8, 1.2, 2.5, 3.0, 5.0], i);
            let a = patch(
                vec![osc("bell", 0.6).pos(0.1 * (i % 8) as f64)],
                [0.001, decay, 0.0, decay * 0.8],
            );
            let b = patch(
                vec![
                    osc("sine", 0.5),
                    osc("sine", 0.2)
                        .oct(pick(&[1, 2], i))
                        .semi(pick(&[0, 7, 4, 0, 5], i)),
                ],
                [0.001, decay * 0.6, 0.0, decay * 0.5],
            );
            let mut layers = vec![
                layer("Bell", 0.65, -0.1, 12, &a),
                layer("Tone", 0.45, 0.1, 0, &b),
            ];
            if i == 9 {
                layers[1]["patch"] = patch(vec![osc("sine", 0.7)], [0.001, 2.0, 0.0, 1.0])
                    .lfo("sine", 4.0, false, "amp", 0.25)
                    .json();
            }
            state(
                layers,
                fx(0.0, pick(&[0.0, 0.25, 0.15], i), 0.75, 0.4, 4.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn arps(out: &mut Vec<Preset>) {
    family(
        out,
        "Arps",
        "AR",
        &[
            "Running Up",
            "Falling Stars",
            "Hypnotic Loop",
            "Octave Jumper",
            "Random Sparks",
            "Sixteenth Engine",
            "Trance Runner",
            "Bell Cascade",
            "Pluck Machine",
            "Dreamy Cycle",
            "Acid Steps",
            "Electric Rain",
            "Up And Down",
            "Neon Arcade",
            "Sequence Sunrise",
            "Pulse Train",
        ],
        |i| {
            let t = pick(
                &[
                    "saw",
                    "square",
                    "bell",
                    "pulse25",
                    "digital",
                    "triangle",
                    "harmonics",
                ],
                i,
            );
            let p = patch(
                vec![osc(t, 0.7).uni(pick(&[1, 3, 5], i), 0.25)],
                [0.001, 0.25, 0.2, 0.2],
            )
            .filter(
                if i == 10 { "ladder" } else { "lp24" },
                900.0,
                if i == 10 { 0.7 } else { 0.2 },
                4.0,
            )
            .fenv([0.001, 0.15, 0.0, 0.1]);
            let mut s = state(
                vec![layer("Seq", 0.8, 0.0, 0, &p)],
                fx(0.0, 0.3, pick(&[0.75, 0.5, 0.375], i), 0.25, 2.5),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            s["arp"] = json!({
                "on": true,
                "mode": pick(&["up", "down", "updown", "up", "random"], i),
                "div": pick(&[0.25, 0.125, 0.25, 0.5 / 3.0], i),
                "octaves": pick(&[1.0, 2.0, 3.0, 2.0], i),
                "gate": pick(&[0.5, 0.7, 0.35, 0.9], i),
            });
            s
        },
    );
}

fn synths(out: &mut Vec<Preset>) {
    family(
        out,
        "Synth",
        "SY",
        &[
            "Poly Sweep",
            "Polysynth Classic",
            "Eighties Poly",
            "Fat Unison",
            "Synthwave Chord",
            "Outrun",
            "Digital Poly",
            "Sync Poly",
            "Hoover",
            "Rave Stab",
            "Detroit Stab",
            "Wide Analog",
            "Soft Poly",
            "Chorus Poly",
            "Vintage Brass Synth",
            "Laser Poly",
        ],
        |i| {
            let t = pick(
                &[
                    "saw", "analog", "pwm", "saw", "square", "saw", "digital", "sync",
                ],
                i,
            );
            let mut p = patch(
                vec![
                    osc(t, 0.6).uni(pick(&[2, 4, 6], i), 0.25),
                    osc("saw", 0.35).oct(-1),
                ],
                [0.005, 0.6, 0.7, 0.4],
            )
            .filter("lp24", pick(&[1500.0, 2200.0, 3000.0], i), 0.2, 2.5)
            .fenv([0.01, pick(&[0.4, 0.8, 1.5], i), 0.3, 0.4]);
            if i == 8 {
                p = p.lfo("sine", 4.0, false, "osc0.pitch", 0.01);
                p.osc[0] = osc("pwm", 0.7).uni(5, 0.5);
            }
            let mut s = state(
                vec![layer("Poly", 0.8, 0.0, 0, &p)],
                fx(
                    pick(&[0.3, 0.0, 0.4], i),
                    pick(&[0.0, 0.2], i),
                    0.75,
                    0.25,
                    2.5,
                ),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            if (8..=10).contains(&i) {
                s["macros"]["drive"] = json!(0.3);
            }
            if i == 3 || i == 11 {
                s["macros"]["width"] = json!(0.8);
            }
            s
        },
    );
}

fn chords(out: &mut Vec<Preset>) {
    // the intervals of each chord, as layer transposes
    let shapes: [&[i32]; 16] = [
        &[0, 3, 7, 10],
        &[0, 4, 7, 12],
        &[0, 5, 7, 12],
        &[0, 3, 7, 10],
        &[0, 3, 10, 14],
        &[0, 3, 7, 12],
        &[0, 3, 7, 15],
        &[0, 4, 7, 10],
        &[0, 4, 7, 11],
        &[0, 7, 12],
        &[0, 3, 7],
        &[0, 4, 10, 14],
        &[0, 4, 7, 14],
        &[0, 5, 10],
        &[0, 4, 7, 11],
        &[0, 3, 7, 10],
    ];
    family(
        out,
        "Chords",
        "CH",
        &[
            "House Minor Chord",
            "Major Lift",
            "Sus Four Stab",
            "Minor Seventh Pad",
            "Deep House Ninth",
            "Dub Chord",
            "Rave Minor",
            "Garage Organ Chord",
            "Lush Major Seventh",
            "Power Fifth",
            "Trance Minor Stab",
            "Soul Chord",
            "Dream Add Nine",
            "Techno Chord",
            "Future Bass Chord",
            "Lofi Chord",
        ],
        |i| {
            let pad = matches!(i, 3 | 8 | 12);
            let t = pick(
                &[
                    "saw", "organ", "square", "saw", "pwm", "analog", "saw", "organ",
                ],
                i,
            );
            let amp = if pad {
                [0.4, 1.0, 0.8, 1.5]
            } else {
                [0.002, 0.45, 0.0, 0.3]
            };
            let p = patch(vec![osc(t, 0.6).uni(pick(&[1, 3, 5], i), 0.25)], amp)
                .filter(
                    "lp24",
                    pick(&[900.0, 1400.0, 2000.0], i),
                    0.2,
                    if pad { 0.5 } else { 3.5 },
                )
                .fenv([0.001, 0.25, 0.1, 0.3]);
            let shape = shapes[i];
            let layers = shape
                .iter()
                .enumerate()
                .map(|(k, &tr)| {
                    let pan = (k as f64 - (shape.len() - 1) as f64 / 2.0) * 0.25;
                    layer(&format!("Note {}", k + 1), 0.55, pan, tr, &p)
                })
                .collect();
            let dub = i == 5;
            state(
                layers,
                fx(
                    0.0,
                    if dub { 0.4 } else { 0.18 },
                    0.75,
                    if pad { 0.45 } else { 0.25 },
                    3.0,
                ),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn effects(out: &mut Vec<Preset>) {
    family(
        out,
        "FX",
        "FX",
        &[
            "Riser Storm",
            "Falling Floor",
            "Noise Sweep",
            "Laser Zap",
            "Space Wind",
            "Radio Static",
            "Siren Call",
            "Alien Signal",
            "Thunder Roll",
            "Ocean Waves",
            "Glitch Burst",
            "Sci-Fi Sweep",
            "Impact Boom",
            "White Out",
        ],
        |i| {
            let p = match i % 7 {
                // noise sweeps
                0 | 2 | 6 => patch(
                    vec![osc("noise", 0.6), osc("saw", 0.4).uni(5, 0.5)],
                    [1.5, 1.0, 1.0, 2.0],
                )
                .filter("bp12", 800.0, 0.5, 4.0)
                .fenv([3.0, 1.0, 1.0, 2.0])
                .lfo("sine", 0.25, false, "pitch", 0.2),
                // zaps and booms
                1 | 5 => patch(
                    vec![osc("sine", 0.8), osc("noise", 0.3)],
                    [0.001, 1.5, 0.0, 1.0],
                )
                .pitch([0.001, 0.8, 0.0, 0.5], -1.0),
                // slow textures
                _ => patch(
                    vec![osc(pick(&["fm", "digital", "growl"], i), 0.6).uni(3, 0.4)],
                    [0.8, 1.0, 1.0, 3.0],
                )
                .filter("lp24", 1500.0, 0.6, 0.0)
                .lfo("sh", 6.0, false, "f0.cutoff", 0.3),
            };
            state(
                vec![layer("FX", 0.75, 0.0, 0, &p)],
                fx(0.0, 0.3, 0.75, 0.55, 6.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn drums(out: &mut Vec<Preset>) {
    family(
        out,
        "Drums",
        "DR",
        &[
            "Deep Kick",
            "Punch Kick",
            "Long 808",
            "Analog Snare",
            "Noise Clap",
            "Closed Hat",
            "Open Hat",
            "Shaker",
            "Analog Tom",
            "Rim Click",
            "Cowbell",
            "Zap Perc",
            "Conga Synth",
            "Crash Noise",
        ],
        |i| {
            let tone = |decay: f64, drop: f64, pdecay: f64| {
                patch(vec![osc("sine", 0.9)], [0.001, decay, 0.0, decay * 0.5])
                    .pitch([0.001, pdecay, 0.0, 0.05], drop)
            };
            let noise = |kind: &'static str, cutoff: f64, decay: f64| {
                patch(vec![osc("noise", 0.8)], [0.001, decay, 0.0, decay * 0.6])
                    .filter(kind, cutoff, 0.15, 0.0)
            };
            let (layers, room) = match i {
                0 => (
                    vec![layer("Kick", 1.0, 0.0, -24, &tone(0.45, 1.5, 0.05))],
                    0.0,
                ),
                1 => (
                    vec![
                        layer("Kick", 1.0, 0.0, -24, &tone(0.3, 1.8, 0.03)),
                        layer("Click", 0.3, 0.0, 0, &noise("hp12", 3000.0, 0.02)),
                    ],
                    0.0,
                ),
                2 => (
                    vec![layer("808", 1.0, 0.0, -24, &tone(1.6, 0.5, 0.04))],
                    0.0,
                ),
                3 => (
                    vec![
                        layer("Shell", 0.7, 0.0, -5, &tone(0.15, 0.4, 0.03)),
                        layer("Wires", 0.6, 0.0, 0, &noise("hp12", 1500.0, 0.2)),
                    ],
                    0.15,
                ),
                4 => (
                    vec![layer("Clap", 0.9, 0.0, 0, &noise("bp12", 1200.0, 0.2))],
                    0.25,
                ),
                5 => (
                    vec![layer("Hat", 0.8, 0.0, 0, &noise("hp12", 8000.0, 0.05))],
                    0.0,
                ),
                6 => (
                    vec![layer("Hat", 0.8, 0.0, 0, &noise("hp12", 7000.0, 0.45))],
                    0.0,
                ),
                7 => (
                    vec![layer("Shaker", 0.7, 0.0, 0, &noise("bp12", 6000.0, 0.09))],
                    0.1,
                ),
                8 => (
                    vec![layer("Tom", 0.9, 0.0, -12, &tone(0.4, 0.3, 0.1))],
                    0.15,
                ),
                9 => (
                    vec![
                        layer("Rim", 0.8, 0.0, 12, &tone(0.04, 0.2, 0.01)),
                        layer("Snap", 0.4, 0.0, 0, &noise("bp12", 3000.0, 0.03)),
                    ],
                    0.1,
                ),
                10 => {
                    let bell = patch(
                        vec![osc("square", 0.5), osc("square", 0.5).semi(7)],
                        [0.001, 0.25, 0.0, 0.1],
                    )
                    .filter("bp12", 1000.0, 0.4, 0.0);
                    (vec![layer("Bell", 0.7, 0.0, 12, &bell)], 0.1)
                }
                11 => (
                    vec![layer("Zap", 0.9, 0.0, 12, &tone(0.15, 1.5, 0.08))],
                    0.15,
                ),
                12 => (
                    vec![layer("Conga", 0.9, 0.0, 0, &tone(0.3, 0.15, 0.02))],
                    0.15,
                ),
                _ => (
                    vec![layer("Crash", 0.7, 0.0, 0, &noise("hp12", 4000.0, 2.0))],
                    0.2,
                ),
            };
            state(
                layers,
                fx(0.0, 0.0, 0.5, room, 1.0),
                macros(0.5, 0.0, 0.5, 0.5),
            )
        },
    );
}

fn gates(out: &mut Vec<Preset>) {
    family(
        out,
        "Gates",
        "GT",
        &[
            "Trance Gate Choir",
            "Gated Strings",
            "Chopper Pad",
            "Stutter Saw",
            "Sixteenth Gate",
            "Gated Glass",
            "Pulse Gate Pad",
            "Gated Supersaw",
        ],
        |i| {
            let t = pick(
                &[
                    "choir", "saw", "pwm", "saw", "analog", "bell", "square", "saw",
                ],
                i,
            );
            let p = patch(
                vec![osc(t, 0.6).uni(pick(&[4, 6, 8], i), 0.35)],
                [0.05, 1.0, 1.0, 0.5],
            )
            .filter("lp24", 5000.0, 0.1, 0.0);
            let steps: Vec<u8> = (0..16)
                .map(|k| u8::from(k % pick(&[2, 3, 4, 2], i) != 1))
                .collect();
            let mut s = state(
                vec![layer("Gate", 0.8, 0.0, 0, &p)],
                fx(0.0, 0.2, 0.75, 0.3, 3.0),
                macros(0.5, 0.0, 0.5, 0.5),
            );
            let mut chain = vec![
                json!({ "type": "las-gate", "on": true, "state": { "steps": steps, "div": 0.25, "gate": 0.6 } }),
            ];
            chain.extend(s["fx"].as_array().cloned().unwrap_or_default());
            s["fx"] = json!(chain);
            s
        },
    );
}
