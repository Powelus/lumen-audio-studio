// SPDX-License-Identifier: GPL-3.0-or-later
//! A channel's instrument: the synth engines of the web version, its
//! arpeggiator and its effects rack. Port of `instruments/synth.ts` and
//! `instruments/engineSynth.ts`.

use crate::arp::{ArpState, Arpeggiator, Transport};
use crate::audioclip::{AudioClipper, ClipPlayback};
use crate::factory::Audio;
use crate::fxauto::{densify, map_automation, SlotAuto};
use crate::param::Param;
use crate::pluginparams::plugin_param_def;
use crate::sampler::{PadBank, Sampler};
use las_dsp::engine::{prepare_patch, Event, PatchError};
use las_dsp::fm::{fm_engine, Fm, FmEngine};
use las_dsp::fx::registry::{full_state, parse_params, EffectUnit, FxSlot};
use las_dsp::mist::{mist_engine, Mist, MistEngine};
use las_dsp::patch::merge_patch;
use las_dsp::synth::{default_synth_json, prepare_synth_patch, synth_engine, SynthEngine};
use las_dsp::wavetable::TableLibrary;
use serde_json::Value;
use std::sync::Arc;

/// What a Web Audio `StereoPannerNode` does to a stereo signal.
pub fn stereo_pan(l: &mut [f32], r: &mut [f32], pan: f64) {
    let pan = pan.clamp(-1.0, 1.0);
    if pan == 0.0 {
        // cos(pi/2) is about 6e-17: nothing to do at f32 precision
        return;
    }
    let x = if pan <= 0.0 { pan + 1.0 } else { pan };
    let gl = (x * std::f64::consts::FRAC_PI_2).cos() as f32;
    let gr = (x * std::f64::consts::FRAC_PI_2).sin() as f32;
    for i in 0..l.len() {
        let (a, b) = (l[i], r[i]);
        if pan <= 0.0 {
            l[i] = a + b * gl;
            r[i] = b * gr;
        } else {
            l[i] = a * gl;
            r[i] = b + a * gr;
        }
    }
}

/// A mono signal through a `StereoPannerNode`: equal power, so a centred signal
/// is 3 dB down in each side.
pub fn mono_pan_a(src: &[f32], pans: &[f32], l: &mut [f32], r: &mut [f32]) {
    for i in 0..src.len() {
        let x = ((f64::from(pans[i]).clamp(-1.0, 1.0) + 1.0) / 2.0) as f32;
        l[i] = src[i] * (x * std::f32::consts::FRAC_PI_2).cos();
        r[i] = src[i] * (x * std::f32::consts::FRAC_PI_2).sin();
    }
}

/// The same with a pan that changes from frame to frame.
pub fn stereo_pan_a(l: &mut [f32], r: &mut [f32], pans: &[f32]) {
    let (mut last, mut gl, mut gr) = (f32::NAN, 0f32, 0f32);
    for i in 0..l.len() {
        let p = pans[i];
        if p != last {
            last = p;
            let pan = f64::from(p).clamp(-1.0, 1.0);
            let x = if pan <= 0.0 { pan + 1.0 } else { pan };
            gl = (x * std::f64::consts::FRAC_PI_2).cos() as f32;
            gr = (x * std::f64::consts::FRAC_PI_2).sin() as f32;
        }
        let (a, b) = (l[i], r[i]);
        if p <= 0.0 {
            l[i] = a + b * gl;
            r[i] = b * gr;
        } else {
            l[i] = a * gl;
            r[i] = b + a * gr;
        }
    }
}

/// One synth voice engine with its place in the instrument (Nebula stacks layers).
struct Layer {
    engine: SynthEngine,
    gain: Param,
    pan: Param,
    transpose: f64,
    /// Place in the plugin state's list of layers (disabled ones are left out).
    index: usize,
    /// The layer's own level, before the master.
    level: f64,
    /// The layer's patch before the macros scale it.
    base: Value,
}

/// An effect in the instrument's own rack, and where it sits in the state's list.
struct RackSlot {
    index: usize,
    fx: FxSlot,
    auto: SlotAuto,
}

enum Core {
    Layers(Vec<Layer>),
    Facet(Box<FmEngine>),
    Mist(Box<MistEngine>),
    Sampler(Box<Sampler>),
    Pads(Box<PadBank>),
    AudioClip(Box<AudioClipper>),
    /// An instrument that makes no sound here (yet).
    Silent,
}

pub struct Instrument {
    core: Core,
    arp: Option<Arpeggiator>,
    rack: Vec<RackSlot>,
    sr: f64,
    scratch: [Vec<f32>; 2],
    gains: [Vec<f32>; 2],
    plugin_id: String,
    /// Nebula's master level.
    master: f64,
}

fn cut_mul(m: f64) -> f64 {
    2f64.powf((m - 0.5) * 8.0)
}

fn env_mul(m: f64) -> f64 {
    2f64.powf((m - 0.5) * 6.0)
}

fn num(v: Option<&Value>, default: f64) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(default)
}

struct LayerSpec {
    patch: Value,
    gain: f64,
    pan: f64,
    transpose: f64,
    index: usize,
    level: f64,
    base: Value,
}

/// The layers a plugin state describes.
fn layers_from_state(plugin_id: &str, state: &Value) -> Vec<LayerSpec> {
    let defaults = default_synth_json();
    if plugin_id != "nebula" {
        let patch = merge_patch(&defaults, state.get("patch").unwrap_or(&Value::Null));
        return vec![LayerSpec {
            base: patch.clone(),
            patch,
            gain: 1.0,
            pan: 0.0,
            transpose: 0.0,
            index: 0,
            level: 1.0,
        }];
    }
    let macros = state.get("macros");
    let m = |k: &str, d: f64| num(macros.and_then(|m| m.get(k)), d);
    let (cutoff, res, attack, release) = (
        m("cutoff", 0.5),
        m("res", 0.0),
        m("attack", 0.5),
        m("release", 0.5),
    );
    let master = num(state.get("master"), 0.8);
    let mut out = Vec::new();
    let layers = state.get("layers").and_then(Value::as_array);
    for (index, l) in layers.into_iter().flatten().enumerate() {
        if l.get("on") == Some(&Value::Bool(false)) || !l.is_object() {
            continue;
        }
        let mut p = merge_patch(&defaults, l.get("patch").unwrap_or(&Value::Null));
        let base = p.clone();
        if let Some(fs) = p.get_mut("filters").and_then(Value::as_array_mut) {
            for f in fs {
                let c = (num(f.get("cutoff"), 1000.0) * cut_mul(cutoff)).clamp(20.0, 20000.0);
                let r = (num(f.get("res"), 0.0) + res).min(1.0);
                f["cutoff"] = c.into();
                f["res"] = r.into();
            }
        }
        if let Some(e) = p.pointer_mut("/env/0") {
            let a = num(e.get("a"), 0.01) * env_mul(attack);
            let r = num(e.get("r"), 0.2) * env_mul(release);
            e["a"] = a.into();
            e["r"] = r.into();
        }
        out.push(LayerSpec {
            patch: p,
            base,
            index,
            level: num(l.get("level"), 1.0),
            gain: num(l.get("level"), 1.0) * master,
            pan: num(l.get("pan"), 0.0),
            transpose: num(l.get("transpose"), 0.0),
        });
    }
    out
}

impl Instrument {
    /// Build the instrument for a channel. Unknown or not yet ported
    /// instruments are silent.
    pub fn new(
        plugin_id: &str,
        state: &Value,
        sr: f64,
        tables: &mut TableLibrary,
        sample: Option<Arc<Audio>>,
        pad_samples: Vec<Option<Arc<Audio>>>,
    ) -> Result<Self, PatchError> {
        let core = match plugin_id {
            "facet" => {
                let mut e = fm_engine(sr, 1);
                let mut p = prepare_patch::<Fm>(state.get("patch").unwrap_or(&Value::Null))?;
                e.apply_patch(&mut p);
                Core::Facet(Box::new(e))
            }
            "mist" => {
                let mut e = mist_engine(sr, 1);
                if let Some(a) = &sample {
                    // the channel's own sample replaces the built-in texture
                    let l: Arc<[f32]> = Arc::from(a.channels[0].clone());
                    let r: Arc<[f32]> = a
                        .channels
                        .get(1)
                        .map_or_else(|| l.clone(), |c| Arc::from(c.clone()));
                    e.sound.set_source(l, r, a.sample_rate);
                }
                let mut p = prepare_patch::<Mist>(state.get("patch").unwrap_or(&Value::Null))?;
                e.apply_patch(&mut p);
                Core::Mist(Box::new(e))
            }
            "polaris" | "vortex" | "monolith" | "zenith" | "triosc" | "nebula" => {
                let mut layers = Vec::new();
                for spec in layers_from_state(plugin_id, state) {
                    let mut e = synth_engine(sr, 1);
                    e.set_tables(tables.clone());
                    let mut p = prepare_synth_patch(tables, &spec.patch)?;
                    e.apply_patch(&mut p);
                    layers.push(Layer {
                        engine: e,
                        gain: Param::new(f64::from(spec.gain as f32)),
                        pan: Param::new(spec.pan),
                        transpose: spec.transpose,
                        index: spec.index,
                        level: spec.level,
                        base: spec.base,
                    });
                }
                Core::Layers(layers)
            }
            "sampler" => Core::Sampler(Box::new(Sampler::new(state, sample, sr))),
            "lumapad" => Core::Pads(Box::new(PadBank::new(state, pad_samples, sr))),
            "audioclip" => Core::AudioClip(Box::new(AudioClipper::new(state, sample, sr))),
            _ => Core::Silent,
        };
        let arp_state = ArpState::from_json(state.get("arp"));
        let arp = arp_state.on.then(|| Arpeggiator::new(arp_state));
        let mut rack = Vec::new();
        let items = state.get("fx").and_then(Value::as_array);
        for (index, item) in items.into_iter().flatten().enumerate() {
            let Some(id) = item
                .get("type")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let Some(mut unit) = EffectUnit::new(id, sr) else {
                continue;
            };
            let st = full_state(id, item.get("state").unwrap_or(&Value::Null));
            if let Ok(p) = parse_params(id, &st, sr) {
                unit.apply(p);
            }
            let mut slot = FxSlot::new(unit, sr);
            slot.enabled = item.get("on") != Some(&Value::Bool(false));
            slot.mix = 1.0;
            let auto = SlotAuto::new(
                id,
                item.get("state").unwrap_or(&Value::Null),
                slot.enabled,
                1.0,
                sr,
            );
            rack.push(RackSlot {
                index,
                fx: slot,
                auto,
            });
        }
        Ok(Self {
            core,
            arp,
            rack,
            sr,
            scratch: [Vec::new(), Vec::new()],
            gains: [Vec::new(), Vec::new()],
            plugin_id: plugin_id.to_string(),
            master: num(state.get("master"), 0.8),
        })
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    pub fn is_silent(&self) -> bool {
        matches!(self.core, Core::Silent)
    }

    pub fn has_arp(&self) -> bool {
        self.arp.is_some()
    }

    /// Mist plays the channel's sample; give it the decoded audio.
    pub fn set_mist_source(
        &mut self,
        left: std::sync::Arc<[f32]>,
        right: std::sync::Arc<[f32]>,
        rate: f64,
    ) {
        if let Core::Mist(e) = &mut self.core {
            e.sound.set_source(left, right, rate);
        }
    }

    /// Tempo for synced LFOs and effects.
    pub fn set_bpm(&mut self, bpm: f64) {
        match &mut self.core {
            Core::Layers(ls) => {
                for l in ls {
                    l.engine.set_transport(bpm, 0.0, 0.0);
                }
            }
            Core::Facet(e) => e.set_bpm(bpm),
            Core::Mist(e) => e.set_bpm(bpm),
            Core::Sampler(_) | Core::Pads(_) | Core::AudioClip(_) | Core::Silent => {}
        }
    }

    fn queue_all(&mut self, evs: &[Event], transpose_layers: bool) {
        match &mut self.core {
            Core::Layers(ls) => {
                for l in ls {
                    if transpose_layers && l.transpose != 0.0 {
                        let shifted: Vec<Event> = evs
                            .iter()
                            .map(|e| match *e {
                                Event::On {
                                    time,
                                    key,
                                    vel,
                                    fine,
                                    pan,
                                } => Event::On {
                                    time,
                                    key: key + l.transpose,
                                    vel,
                                    fine,
                                    pan,
                                },
                                Event::Off { time, key } => Event::Off {
                                    time,
                                    key: key + l.transpose,
                                },
                                other => other,
                            })
                            .collect();
                        l.engine.queue(&shifted);
                    } else {
                        l.engine.queue(evs);
                    }
                }
            }
            Core::Facet(e) => e.queue(evs),
            Core::Mist(e) => e.queue(evs),
            Core::Sampler(_) | Core::Pads(_) | Core::AudioClip(_) | Core::Silent => {}
        }
    }

    /// Play a note of known length (seconds on the engine clock).
    #[allow(clippy::too_many_arguments)]
    pub fn play_note(
        &mut self,
        key: f64,
        vel: f64,
        time: f64,
        duration: f64,
        fine: f64,
        pan: f64,
        one_shot: bool,
    ) {
        if let Core::Sampler(s) = &mut self.core {
            // step notes ring out like FL; piano roll notes are gated by their length
            let ring = one_shot && s.state_loop() != Some(true);
            s.play_note(
                key as i32,
                vel,
                time,
                (!ring).then_some(duration),
                fine,
                pan,
            );
            return;
        }
        if let Core::Pads(p) = &mut self.core {
            p.play_note(key as i32, vel, time, fine);
            return;
        }
        if let Core::AudioClip(c) = &mut self.core {
            c.play_note(time);
            return;
        }
        if let Some(arp) = &mut self.arp {
            arp.play_note(key, vel, time, duration);
            return;
        }
        self.direct(key, vel, time, duration, fine, pan);
    }

    fn direct(&mut self, key: f64, vel: f64, time: f64, duration: f64, fine: f64, pan: f64) {
        let evs = [
            Event::On {
                time,
                key,
                vel,
                fine,
                pan,
            },
            Event::Off {
                time: time + duration.max(0.005),
                key,
            },
        ];
        self.queue_all(&evs, true);
    }

    /// Play an audio clip (audio clip channels only).
    pub fn play_clip(
        &mut self,
        time: f64,
        offset: f64,
        duration: f64,
        clip: Option<&ClipPlayback>,
    ) {
        if let Core::AudioClip(c) = &mut self.core {
            c.play_clip(time, offset, duration, clip);
        }
    }

    /// Stop at `time`: notes that have not happened yet are forgotten and what sounds is
    /// released (or faded).
    pub fn stop(&mut self, time: f64) {
        if let Some(arp) = &mut self.arp {
            arp.clear();
        }
        match &mut self.core {
            Core::Layers(ls) => ls.iter_mut().for_each(|l| l.engine.stop()),
            Core::Facet(e) => e.stop(),
            Core::Mist(e) => e.stop(),
            Core::Sampler(s) => s.stop(time),
            Core::Pads(p) => p.stop(time),
            Core::AudioClip(c) => c.stop(time),
            Core::Silent => {}
        }
    }

    /// A key played live: sounds from `time` until [`Instrument::note_off`].
    pub fn note_on(&mut self, key: f64, vel: f64, time: f64) {
        match &mut self.core {
            Core::Sampler(s) => s.play_note(key as i32, vel, time, None, 0.0, 0.0),
            Core::Pads(p) => p.play_note(key as i32, vel, time, 0.0),
            Core::AudioClip(c) => c.play_note(time),
            Core::Silent => {}
            _ => {
                let evs = [Event::On {
                    time,
                    key,
                    vel,
                    fine: 0.0,
                    pan: 0.0,
                }];
                self.queue_all(&evs, true);
            }
        }
    }

    /// A key played live is let go.
    pub fn note_off(&mut self, key: f64, time: f64) {
        match &mut self.core {
            Core::Sampler(s) => s.note_off(key as i32, time),
            Core::Pads(_) | Core::AudioClip(_) | Core::Silent => {}
            _ => {
                let evs = [Event::Off { time, key }];
                self.queue_all(&evs, true);
            }
        }
    }

    /// Whether the output is a mono signal (a channel then pans it with the
    /// mono law, which is 3 dB down in the middle).
    pub fn is_mono(&self) -> bool {
        matches!(&self.core, Core::AudioClip(c) if c.is_mono())
    }

    /// Let the arpeggiator make its steps for `[from, to)` seconds.
    pub fn tick(&mut self, from: f64, to: f64, t: Transport) {
        let Some(mut arp) = self.arp.take() else {
            return;
        };
        let mut steps = Vec::new();
        arp.generate(from, to, t, &mut |k, v, time, d| {
            steps.push((k, v, time, d))
        });
        self.arp = Some(arp);
        for (k, v, time, d) in steps {
            self.direct(k, v, time, d, 0.0, 0.0);
        }
    }

    /// Effects that follow the beat: where the song is, at block start.
    pub fn set_position(&mut self, beat: f64, bpm: f64) {
        for s in &mut self.rack {
            s.fx.unit
                .set_position(las_dsp::fx::stepfx::Position { beat, bpm });
        }
    }

    /// Delay of the effects rack, seconds.
    pub fn latency(&self) -> f64 {
        self.rack.iter().map(|s| s.fx.latency()).sum()
    }

    /// Render a block into `l` and `r` (which it overwrites).
    pub fn render(&mut self, t0: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        l.fill(0.0);
        r.fill(0.0);
        match &mut self.core {
            Core::Layers(layers) => {
                for s in &mut self.scratch {
                    s.resize(n, 0.0);
                }
                for layer in layers {
                    let [sl, sr] = &mut self.scratch;
                    sl.fill(0.0);
                    sr.fill(0.0);
                    layer.engine.process(sl, sr, t0);
                    let frame = (t0 * self.sr).round() as usize;
                    let [gain, pan] = &mut self.gains;
                    gain.resize(n, 0.0);
                    pan.resize(n, 0.0);
                    layer.gain.fill(frame, gain, self.sr);
                    layer.pan.fill(frame, pan, self.sr);
                    if gain.iter().any(|&g| g != 1.0) {
                        for i in 0..n {
                            sl[i] *= gain[i];
                            sr[i] *= gain[i];
                        }
                    }
                    if pan.iter().any(|&p| p != 0.0) {
                        stereo_pan_a(sl, sr, pan);
                    }
                    for i in 0..n {
                        l[i] += sl[i];
                        r[i] += sr[i];
                    }
                }
            }
            Core::Facet(e) => e.process(l, r, t0),
            Core::Mist(e) => e.process(l, r, t0),
            Core::Sampler(s) => s.render((t0 * self.sr).round() as usize, l, r),
            Core::Pads(p) => p.render((t0 * self.sr).round() as usize, l, r),
            Core::AudioClip(c) => c.render((t0 * self.sr).round() as usize, l, r),
            Core::Silent => {}
        }
        let frame = (t0 * self.sr).round() as usize;
        for slot in &mut self.rack {
            slot.auto.begin_block(frame, &mut slot.fx.unit);
            let [wet, dry] = &mut self.gains;
            wet.resize(n, 0.0);
            dry.resize(n, 0.0);
            slot.auto.wet.fill(frame, wet, self.sr);
            slot.auto.dry.fill(frame, dry, self.sr);
            slot.fx.process_gains(l, r, None, wet, dry);
        }
    }

    /// Automate an instrument parameter through `normalised` points (time, 0 to 1).
    /// Returns whether the instrument handles it.
    pub fn automate(
        &mut self,
        path: &str,
        normalised: &[(f64, f64)],
        first: bool,
        length: usize,
    ) -> bool {
        let def = plugin_param_def(&self.plugin_id, path);
        let mapped = map_automation(Some(&def), normalised);
        let points = &mapped[..];
        if let Some(rest) = path.strip_prefix("fx.") {
            let Some((index, sub)) = rest.split_once(".state.") else {
                return false;
            };
            let Ok(index) = index.parse::<usize>() else {
                return false;
            };
            // the rack's effects use the ranges the synth panels use for their names
            let Some(slot) = self.rack.iter_mut().find(|s| s.index == index) else {
                return false;
            };
            if sub != "mix" && slot.fx.unit.automate(sub, points, first) {
                return true;
            }
            slot.auto
                .automate_with(sub, normalised, first, length, Some(&def));
            return true;
        }
        match &mut self.core {
            Core::Layers(layers) if self.plugin_id != "nebula" => {
                let Some(sub) = path.strip_prefix("patch.") else {
                    return false;
                };
                for l in layers {
                    l.engine.ramp(sub, points, first);
                }
                true
            }
            Core::Layers(layers) => {
                let master = self.master;
                if path == "master" {
                    for l in layers {
                        let pts: Vec<_> = points.iter().map(|&(t, v)| (t, v * l.level)).collect();
                        l.gain.ramp(&pts, first);
                    }
                    return true;
                }
                if let Some(rest) = path.strip_prefix("layers.") {
                    let Some((i, what)) = rest.split_once('.') else {
                        return false;
                    };
                    let Ok(i) = i.parse::<usize>() else {
                        return false;
                    };
                    let known = what == "level" || what == "pan" || what.starts_with("patch.");
                    if !known {
                        return false;
                    }
                    // a layer that is switched off has nothing to automate
                    let Some(l) = layers.iter_mut().find(|l| l.index == i) else {
                        return true;
                    };
                    if what == "level" {
                        let pts: Vec<_> = points.iter().map(|&(t, v)| (t, v * master)).collect();
                        l.gain.ramp(&pts, first);
                    } else if what == "pan" {
                        l.pan.ramp(points, first);
                    } else {
                        l.engine.ramp(&what[6..], points, first);
                    }
                    return true;
                }
                let Some(m) = path.strip_prefix("macros.") else {
                    return false;
                };
                if !matches!(m, "cutoff" | "res" | "attack" | "release") {
                    return false;
                }
                // a macro scales each layer's own values: one ramp per layer
                let pts = densify(points, 0.02);
                for l in layers {
                    if m == "cutoff" || m == "res" {
                        let filters = l.base.get("filters").and_then(Value::as_array).cloned();
                        for (fi, f) in filters.into_iter().flatten().enumerate() {
                            let values: Vec<(f64, f64)> = pts
                                .iter()
                                .map(|&(t, v)| {
                                    let x = if m == "cutoff" {
                                        (num(f.get("cutoff"), 1000.0) * cut_mul(v))
                                            .clamp(20.0, 20000.0)
                                    } else {
                                        (num(f.get("res"), 0.0) + v).min(1.0)
                                    };
                                    (t, x)
                                })
                                .collect();
                            l.engine.ramp(&format!("filters.{fi}.{m}"), &values, first);
                        }
                    } else {
                        let key = if m == "attack" { "a" } else { "r" };
                        let base = num(
                            l.base.pointer("/env/0").and_then(|e| e.get(key)),
                            if key == "a" { 0.01 } else { 0.2 },
                        );
                        let values: Vec<(f64, f64)> =
                            pts.iter().map(|&(t, v)| (t, base * env_mul(v))).collect();
                        l.engine.ramp(&format!("env.0.{key}"), &values, first);
                    }
                }
                true
            }
            Core::Facet(e) => path.strip_prefix("patch.").is_some_and(|sub| {
                e.ramp(sub, points, first);
                true
            }),
            Core::Mist(e) => path.strip_prefix("patch.").is_some_and(|sub| {
                e.ramp(sub, points, first);
                true
            }),
            Core::Sampler(sm) => sm.automate(path, points, first),
            Core::Pads(p) => p.automate(path, points, first),
            Core::AudioClip(c) => {
                if path == "gain" {
                    c.automate_gain(points, first);
                    true
                } else {
                    false
                }
            }
            Core::Silent => false,
        }
    }
}
