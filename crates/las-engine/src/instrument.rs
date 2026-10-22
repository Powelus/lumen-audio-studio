// SPDX-License-Identifier: GPL-3.0-or-later
//! A channel's instrument: the synth engines of the web version, its
//! arpeggiator and its effects rack. Port of `instruments/synth.ts` and
//! `instruments/engineSynth.ts`.

use crate::arp::{ArpState, Arpeggiator, Transport};
use crate::audioclip::{AudioClipper, ClipPlayback};
use crate::external::{plugin_host, Hosted, PLUGIN_ID};
use crate::factory::Audio;
use crate::fxauto::{densify, map_automation, SlotAuto};
use crate::param::Param;
use crate::pluginparams::plugin_param_def;
use crate::sampler::{PadBank, Sampler};
use las_dsp::engine::{prepare_patch, Cc, Event, PatchError};
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
    /// The layer's pan before the width macro scales it.
    base_pan: f64,
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
    /// A hosted plugin.
    Hosted(Box<Hosted>),
    /// An instrument that makes no sound here (yet).
    Silent,
}

pub struct Instrument {
    core: Core,
    arp: Option<Arpeggiator>,
    /// The chord each note becomes (semitones above it; just 0 for none).
    chord: &'static [i32],
    rack: Vec<RackSlot>,
    sr: f64,
    scratch: [Vec<f32>; 2],
    gains: [Vec<f32>; 2],
    plugin_id: String,
    /// Nebula's master level.
    master: f64,
    gate: Gate,
    /// The channel's pitch offset (semitones) and its automation: notes that sound
    /// follow it (see [`Instrument::bends_live`]).
    tune_base: f64,
    tune_line: crate::param::Timeline,
    tune_origin: usize,
}

/// Frames between the points where a moving channel pitch is passed on.
const TUNE_CHUNK: usize = 64;

/// Silence under this (-120 dB) counts as nothing.
pub const QUIET: f32 = 1e-6;

/// How long an instrument or an insert keeps running after it went quiet, seconds.
pub const SMART_HOLD: f64 = 2.0;

/// FL Studio's "smart disable": an instrument with no notes to play whose output has
/// been quiet for [`SMART_HOLD`] is not run until a note comes.
pub(crate) struct Gate {
    pub enabled: bool,
    /// Times (seconds) of the notes and clips to come.
    wake: Vec<f64>,
    /// Keys held live.
    held: u32,
    /// Frames the output has been quiet.
    quiet: usize,
    hold: usize,
}

impl Gate {
    fn new(sr: f64) -> Self {
        Self {
            enabled: true,
            wake: Vec::with_capacity(256),
            held: 0,
            quiet: 0,
            hold: (SMART_HOLD * sr) as usize,
        }
    }

    fn at(&mut self, time: f64) {
        self.wake.push(time);
    }

    /// Whether the block up to `end` seconds can be skipped.
    fn sleeps(&mut self, end: f64) -> bool {
        let before = self.wake.len();
        self.wake.retain(|&t| t >= end);
        if self.wake.len() != before || self.held > 0 {
            self.quiet = 0;
        }
        self.enabled && self.quiet >= self.hold
    }

    fn heard(&mut self, l: &[f32], r: &[f32]) {
        if l.iter().chain(r).all(|s| s.abs() < QUIET) {
            self.quiet = self.quiet.saturating_add(l.len());
        } else {
            self.quiet = 0;
        }
    }
}

/// How much a note's release (FL's note release, 0 to 1, 0.5 when not set) stretches
/// the instrument's release time: a quarter at 0, as it is at 0.5, four times at 1.
pub fn release_scale(release: f64) -> f64 {
    if !release.is_finite() {
        return 1.0;
    }
    16f64.powf(release.clamp(0.0, 1.0) - 0.5)
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
    base_pan: f64,
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
            base_pan: 0.0,
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
    // drive adds filter drive; width and detune scale the stereo spread and the
    // unison detune (at 0.5 they leave the layers as they are)
    let (drive, width, detune) = (
        m("drive", 0.0).clamp(0.0, 1.0),
        (m("width", 0.5) * 2.0).clamp(0.0, 2.0),
        (m("detune", 0.5) * 2.0).clamp(0.0, 2.0),
    );
    let master = num(state.get("master"), 0.8);
    let mut out = Vec::new();
    // a new Nebula has no layers yet: it starts with one default layer
    let one = vec![serde_json::json!({ "on": true, "level": 1.0 })];
    let layers = state.get("layers").and_then(Value::as_array).or(Some(&one));
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
                if drive > 0.0 {
                    f["drive"] = (num(f.get("drive"), 0.0) + drive).into();
                }
            }
        }
        if width != 1.0 || detune != 1.0 {
            if let Some(os) = p.get_mut("osc").and_then(Value::as_array_mut) {
                for o in os {
                    o["spread"] = (num(o.get("spread"), 0.6) * width).clamp(0.0, 1.0).into();
                    o["detune"] = (num(o.get("detune"), 0.2) * detune).clamp(0.0, 1.0).into();
                }
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
            pan: (num(l.get("pan"), 0.0) * width).clamp(-1.0, 1.0),
            base_pan: num(l.get("pan"), 0.0),
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
                        base_pan: spec.base_pan,
                    });
                }
                Core::Layers(layers)
            }
            crate::weave::WEAVE_ID => Core::Hosted(Box::new(Hosted::new(
                Box::new(crate::weave::Weave::new(state, sr)),
                sr,
            ))),
            "sampler" | las_project::slicer::SLICER_ID => {
                Core::Sampler(Box::new(Sampler::new(state, sample, sr)))
            }
            "lumapad" => Core::Pads(Box::new(PadBank::new(state, pad_samples, sr))),
            "audioclip" => Core::AudioClip(Box::new(AudioClipper::new(state, sample, sr))),
            PLUGIN_ID => plugin_host()
                .and_then(|h| h.instrument(state, sr))
                .map_or(Core::Silent, |p| Core::Hosted(Box::new(Hosted::new(p, sr)))),
            _ => Core::Silent,
        };
        let arp_state = ArpState::from_json(state.get("arp"));
        let chord = crate::arp::chord_intervals(&arp_state.chord);
        // the arpeggiator plays an instrument's notes (not the pads' or an audio clip's)
        let arp = (arp_state.on && !matches!(core, Core::Pads(_) | Core::AudioClip(_)))
            .then(|| Arpeggiator::new(arp_state));
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
            chord,
            rack,
            sr,
            scratch: [Vec::new(), Vec::new()],
            gains: [Vec::new(), Vec::new()],
            plugin_id: plugin_id.to_string(),
            master: num(state.get("master"), 0.8),
            gate: Gate::new(sr),
            tune_base: 0.0,
            tune_line: crate::param::Timeline::default(),
            tune_origin: 0,
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
            Core::Sampler(_)
            | Core::Pads(_)
            | Core::AudioClip(_)
            | Core::Hosted(_)
            | Core::Silent => {}
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
                                Event::OffScaled { time, key, scale } => Event::OffScaled {
                                    time,
                                    key: key + l.transpose,
                                    scale,
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
            Core::Hosted(h) => {
                for e in evs {
                    match *e {
                        Event::On { time, key, vel, .. } => h.on(time, key, vel),
                        Event::Off { time, key } | Event::OffScaled { time, key, .. } => {
                            h.off(time, key)
                        }
                        _ => {}
                    }
                }
            }
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
        release: f64,
    ) {
        self.gate.at(time);
        let rel_scale = release_scale(release);
        if let Core::Pads(p) = &mut self.core {
            p.play_note(key as i32, vel, time, fine);
            return;
        }
        if let Core::AudioClip(c) = &mut self.core {
            c.play_note(time);
            return;
        }
        for &semis in self.chord {
            let key = key + f64::from(semis);
            if let Some(arp) = &mut self.arp {
                arp.play_note(key, vel, time, duration);
                continue;
            }
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
                    rel_scale,
                );
                continue;
            }
            self.direct(key, vel, time, duration, fine, pan, rel_scale);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn direct(
        &mut self,
        key: f64,
        vel: f64,
        time: f64,
        duration: f64,
        fine: f64,
        pan: f64,
        rel_scale: f64,
    ) {
        self.gate.at(time);
        if let Core::Sampler(s) = &mut self.core {
            s.play_note(key as i32, vel, time, Some(duration), fine, pan, rel_scale);
            return;
        }
        let off = time + duration.max(0.005);
        let evs = [
            Event::On {
                time,
                key,
                vel,
                fine,
                pan,
            },
            if rel_scale == 1.0 {
                Event::Off { time: off, key }
            } else {
                Event::OffScaled {
                    time: off,
                    key,
                    scale: rel_scale,
                }
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
        self.gate.at(time);
        if let Core::AudioClip(c) = &mut self.core {
            c.play_clip(time, offset, duration, clip);
        }
    }

    /// Stop at `time`: notes that have not happened yet are forgotten and what sounds is
    /// released (or faded).
    pub fn stop(&mut self, time: f64) {
        self.gate.at(time);
        self.gate.held = 0;
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
            Core::Hosted(h) => h.stop(time),
            Core::Silent => {}
        }
    }

    /// A key played live: sounds from `time` until [`Instrument::note_off`].
    pub fn note_on(&mut self, key: f64, vel: f64, time: f64) {
        self.gate.at(time);
        self.gate.held += 1;
        match &mut self.core {
            Core::Sampler(s) => s.play_note(key as i32, vel, time, None, 0.0, 0.0, 1.0),
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
        self.gate.at(time);
        self.gate.held = self.gate.held.saturating_sub(1);
        match &mut self.core {
            Core::Sampler(s) => s.note_off(key as i32, time),
            Core::Pads(_) | Core::AudioClip(_) | Core::Silent => {}
            _ => {
                let evs = [Event::Off { time, key }];
                self.queue_all(&evs, true);
            }
        }
    }

    /// A performance controller moved at `time`: the mod wheel, pitch bend or
    /// aftertouch, from live MIDI.
    pub fn controller(&mut self, cc: Cc, value: f64, time: f64) {
        match &mut self.core {
            Core::Hosted(h) => h.controller(cc, value, time),
            Core::Layers(_) | Core::Facet(_) | Core::Mist(_) => {
                self.queue_all(&[Event::Cc { time, cc, value }], false)
            }
            _ => {}
        }
    }

    /// Take a Weave patch live (see [`crate::external::Instrument::weave_patch`]);
    /// the patch that comes back is to be freed off the audio thread.
    pub fn weave_patch(
        &mut self,
        patch: Box<las_dsp::modular::Patch>,
    ) -> Box<las_dsp::modular::Patch> {
        match &mut self.core {
            Core::Hosted(h) => h.weave_patch(patch),
            _ => patch,
        }
    }

    /// Whether notes already sounding follow the channel's pitch as it moves (the
    /// synths and the sampler); otherwise only new notes take it.
    pub fn bends_live(&self) -> bool {
        matches!(
            self.core,
            Core::Layers(_) | Core::Facet(_) | Core::Mist(_) | Core::Sampler(_)
        )
    }

    /// The channel's pitch offset with no automation, semitones.
    pub fn set_tune_base(&mut self, semis: f64) {
        self.tune_base = semis;
    }

    /// Channel pitch automation (engine time, semitones), as for a parameter timeline.
    pub fn automate_tune(&mut self, points: &[(f64, f64)], first: bool) {
        self.tune_line.set(points, first);
    }

    /// Forget the pitch automation before `time`.
    pub fn prune_tune(&mut self, time: f64) {
        self.tune_line.prune(time);
    }

    /// The frames in `[f0, f0 + n)` where a moving channel pitch is passed on: on a
    /// fixed grid, so that playing live and rendering agree whatever the block size.
    fn tune_points(&self, f0: usize, n: usize) -> std::iter::StepBy<std::ops::Range<usize>> {
        let o = self.tune_origin % TUNE_CHUNK;
        let first = (f0 + TUNE_CHUNK - o).div_ceil(TUNE_CHUNK) * TUNE_CHUNK + o - TUNE_CHUNK;
        (first..f0 + n).step_by(TUNE_CHUNK)
    }

    /// Where a run starts (engine time): the grid the channel pitch moves on starts
    /// there, so a song plays live as it renders.
    pub fn set_tune_origin(&mut self, time: f64) {
        self.tune_origin = (time * self.sr).round().max(0.0) as usize;
    }

    /// Pass the channel pitch over `n` frames from `t0` on to the synths.
    fn follow_tune(&mut self, t0: f64, n: usize) {
        if self.tune_line.is_empty() {
            return;
        }
        let sr = self.sr;
        let f0 = (t0 * sr).round() as usize;
        for f in self.tune_points(f0, n) {
            let t = f as f64 / sr;
            let semis = self.tune_line.get(t, self.tune_base);
            let ev = [Event::Tune { time: t, semis }];
            match &mut self.core {
                Core::Layers(ls) => ls.iter_mut().for_each(|l| l.engine.queue(&ev)),
                Core::Facet(e) => e.queue(&ev),
                Core::Mist(e) => e.queue(&ev),
                _ => return,
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
            self.direct(k, v, time, d, 0.0, 0.0, 1.0);
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
        if self.gate.sleeps(t0 + n as f64 / self.sr) {
            // keep up with the channel pitch, so the next note is not bent twice
            self.follow_tune(t0, n);
            let f0 = (t0 * self.sr).round() as usize;
            if let (Some(f), Core::Sampler(s)) = (self.tune_points(f0, n).last(), &mut self.core) {
                s.retune(self.tune_line.get(f as f64 / self.sr, self.tune_base), f);
            }
            return;
        }
        self.render_awake(t0, l, r);
        self.gate.heard(l, r);
    }

    /// Smart disable: whether this instrument may sleep while it has nothing to play.
    pub fn set_smart_disable(&mut self, on: bool) {
        self.gate.enabled = on;
    }

    /// Whether the instrument is asleep (smart disable), for tests and meters.
    pub fn is_asleep(&self) -> bool {
        self.gate.enabled && self.gate.quiet >= self.gate.hold
    }

    fn render_awake(&mut self, t0: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        self.follow_tune(t0, n);
        let f0 = (t0 * self.sr).round() as usize;
        let points = self.tune_points(f0, n);
        if let (Core::Sampler(s), false) = (&mut self.core, self.tune_line.is_empty()) {
            // the sampler retunes its voices between short stretches
            let mut k = 0;
            for f in points.chain(std::iter::once(f0 + n)) {
                let e = f - f0;
                if e > k {
                    s.render(f0 + k, &mut l[k..e], &mut r[k..e]);
                    k = e;
                }
                if f < f0 + n {
                    s.retune(self.tune_line.get(f as f64 / self.sr, self.tune_base), f);
                }
            }
        } else {
            self.render_core(t0, l, r);
        }
        self.run_rack(t0, l, r);
    }

    fn render_core(&mut self, t0: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
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
            Core::Hosted(h) => h.render(t0, l, r),
            Core::Silent => {}
        }
    }

    fn run_rack(&mut self, t0: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
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
        if let Core::Hosted(h) = &mut self.core {
            return path
                .strip_prefix("p.")
                .and_then(|id| id.parse::<u32>().ok())
                .is_some_and(|id| h.automate(id, normalised, first));
        }
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
                if !matches!(
                    m,
                    "cutoff" | "res" | "attack" | "release" | "drive" | "width" | "detune"
                ) {
                    return false;
                }
                // a macro scales each layer's own values: one ramp per layer
                let pts = densify(points, 0.02);
                for l in layers {
                    if m == "drive" {
                        // drive adds to each filter's own drive
                        let filters = l.base.get("filters").and_then(Value::as_array).cloned();
                        for (fi, f) in filters.into_iter().flatten().enumerate() {
                            let d0 = num(f.get("drive"), 0.0);
                            let values: Vec<(f64, f64)> = pts
                                .iter()
                                .map(|&(t, v)| (t, d0 + v.clamp(0.0, 1.0)))
                                .collect();
                            l.engine
                                .ramp(&format!("filters.{fi}.drive"), &values, first);
                        }
                    } else if m == "width" || m == "detune" {
                        // width and detune scale each oscillator's spread and detune (0.5
                        // leaves them as they are); width also narrows the layer's pan
                        let (key, d) = if m == "width" {
                            ("spread", 0.6)
                        } else {
                            ("detune", 0.2)
                        };
                        let oscs = l.base.get("osc").and_then(Value::as_array).cloned();
                        for (oi, o) in oscs.into_iter().flatten().enumerate() {
                            let b = num(o.get(key), d);
                            let values: Vec<(f64, f64)> = pts
                                .iter()
                                .map(|&(t, v)| (t, (b * (v * 2.0).clamp(0.0, 2.0)).clamp(0.0, 1.0)))
                                .collect();
                            l.engine.ramp(&format!("osc.{oi}.{key}"), &values, first);
                        }
                        if m == "width" {
                            let p0 = l.base_pan;
                            let values: Vec<(f64, f64)> = pts
                                .iter()
                                .map(|&(t, v)| {
                                    (t, (p0 * (v * 2.0).clamp(0.0, 2.0)).clamp(-1.0, 1.0))
                                })
                                .collect();
                            l.pan.ramp(&values, first);
                        }
                    } else if m == "cutoff" || m == "res" {
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
            Core::Hosted(_) | Core::Silent => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn nebula(state: &Value) -> Instrument {
        let mut tables = TableLibrary::new();
        Instrument::new("nebula", state, 24000.0, &mut tables, None, Vec::new()).unwrap()
    }

    /// A held middle C for `frames` frames.
    fn hold(state: &Value, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut inst = nebula(state);
        inst.set_bpm(120.0);
        inst.note_on(60.0, 0.9, 0.0);
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (i, (cl, cr)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
            inst.render(i as f64 * 256.0 / 24000.0, cl, cr);
        }
        (l, r)
    }

    #[test]
    fn the_new_macros_change_the_layers() {
        let st = json!({
            "layers": [{ "on": true, "level": 1.0, "pan": 0.5, "patch": {
                "osc": [{ "on": true, "table": "saw", "voices": 4, "detune": 0.3, "spread": 0.8 }],
                "filters": [{ "on": true, "type": "lp24", "cutoff": 2000, "drive": 0.1 }],
            }}],
            "macros": { "drive": 0.5, "width": 0.25, "detune": 1.0 },
        });
        let l = &layers_from_state("nebula", &st)[0];
        assert!((l.patch["filters"][0]["drive"].as_f64().unwrap() - 0.6).abs() < 1e-9);
        assert!((l.patch["osc"][0]["spread"].as_f64().unwrap() - 0.4).abs() < 1e-9);
        assert!((l.patch["osc"][0]["detune"].as_f64().unwrap() - 0.6).abs() < 1e-9);
        assert!((l.pan - 0.25).abs() < 1e-9);
        // the defaults leave the patch as it is
        let mut plain = st.clone();
        plain["macros"] = json!({});
        let l = &layers_from_state("nebula", &plain)[0];
        assert_eq!(l.patch["osc"][0]["spread"], 0.8);
        assert_eq!(l.patch["filters"][0]["drive"], 0.1);
        assert!((l.pan - 0.5).abs() < 1e-9);
    }

    /// A held note with `path` automated flat at `v` (0 to 1) from the start.
    fn hold_automated(state: &Value, path: &str, v: f64, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut inst = nebula(state);
        inst.set_bpm(120.0);
        assert!(
            inst.automate(path, &[(0.0, v), (10.0, v)], true, 128),
            "{path}"
        );
        inst.note_on(60.0, 0.9, 0.0);
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (i, (cl, cr)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
            inst.render(i as f64 * 256.0 / 24000.0, cl, cr);
        }
        (l, r)
    }

    #[test]
    fn drive_width_and_detune_macros_follow_automation() {
        let st = json!({
            "layers": [{ "on": true, "level": 1.0, "pan": 0.6, "patch": {
                "osc": [{ "on": true, "table": "saw", "voices": 5, "detune": 0.4, "spread": 1.0 }],
                "filters": [{ "on": true, "type": "lp24", "cutoff": 1500, "drive": 0.0 }],
            }}],
        });
        let side = |(l, r): (Vec<f32>, Vec<f32>)| {
            l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum::<f32>()
        };
        let diff = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>();
        // width automated to 0 plays in mono
        assert!(side(hold_automated(&st, "macros.width", 0.0, 4096)) < 1e-3);
        assert!(side(hold_automated(&st, "macros.width", 0.5, 4096)) > 1.0);
        // drive and detune change the sound
        let plain = hold(&st, 4096).0;
        let driven = hold_automated(&st, "macros.drive", 1.0, 4096).0;
        assert!(diff(&plain, &driven) > 1.0, "{}", diff(&plain, &driven));
        let wide = hold_automated(&st, "macros.detune", 1.0, 4096).0;
        assert!(diff(&plain, &wide) > 1.0, "{}", diff(&plain, &wide));
        let same = hold_automated(&st, "macros.detune", 0.5, 4096).0;
        assert!(diff(&plain, &same) < 1e-3, "{}", diff(&plain, &same));
    }

    #[test]
    fn a_notes_release_stretches_the_instruments_release() {
        assert!((release_scale(0.5) - 1.0).abs() < 1e-12);
        assert!((release_scale(0.0) - 0.25).abs() < 1e-12);
        assert!((release_scale(1.0) - 4.0).abs() < 1e-12);
        assert_eq!(release_scale(f64::NAN), 1.0);
        // a synth note of 0.1 s: the tail after it rings longer with a long release
        let tail = |plugin: &str, state: &Value, release: f64| {
            let mut tables = TableLibrary::new();
            let mut inst =
                Instrument::new(plugin, state, 24000.0, &mut tables, None, Vec::new()).unwrap();
            inst.set_bpm(120.0);
            inst.play_note(60.0, 0.9, 0.0, 0.1, 0.0, 0.0, false, release);
            let n = 24000;
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            for (i, (cl, cr)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
                inst.render(i as f64 * 256.0 / 24000.0, cl, cr);
            }
            // energy from 0.2 s on
            l[4800..].iter().map(|s| s * s).sum::<f32>()
        };
        let synth = json!({ "patch": { "env": [{ "a": 0.001, "d": 0.1, "s": 1.0, "r": 0.3 }] } });
        let (short, mid, long) = (
            tail("polaris", &synth, 0.0),
            tail("polaris", &synth, 0.5),
            tail("polaris", &synth, 1.0),
        );
        assert!(short < mid && mid < long, "{short} {mid} {long}");
        assert!(long > mid * 2.0);
    }

    #[test]
    fn width_at_zero_plays_in_mono() {
        let st = |w: f64| {
            json!({
                "layers": [{ "on": true, "level": 1.0, "patch": {
                    "osc": [{ "on": true, "table": "saw", "voices": 5, "detune": 0.4, "spread": 1.0 }],
                }}],
                "macros": { "width": w },
            })
        };
        let side = |w: f64| {
            let (l, r) = hold(&st(w), 4096);
            l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum::<f32>()
        };
        assert!(side(0.0) < 1e-3, "{}", side(0.0));
        assert!(side(0.5) > 1.0);
    }

    #[test]
    fn every_nebula_preset_plays_a_held_note() {
        let b = las_dsp::presets::bank("nebula").unwrap();
        assert!(b.presets.len() >= 300);
        let mut silent = Vec::new();
        for p in &b.presets {
            // the reverb is left out: building its impulse response for every
            // preset makes a debug build slow, and it has tests of its own
            let mut st = p.state.clone();
            if let Some(fx) = st.get_mut("fx").and_then(Value::as_array_mut) {
                fx.retain(|f| f["type"] != "las-reverb");
            }
            let mut inst = nebula(&st);
            inst.set_bpm(120.0);
            inst.note_on(60.0, 0.9, 0.0);
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            let mut peak = 0f32;
            // up to a second: some pads and effects swell in slowly
            for block in 0..94 {
                if block >= 24 && peak >= 1e-3 {
                    break;
                }
                inst.render(block as f64 * 256.0 / 24000.0, &mut l, &mut r);
                for v in l.iter().chain(&r) {
                    assert!(v.is_finite(), "{}", p.name);
                    peak = peak.max(v.abs());
                }
            }
            if peak < 1e-3 {
                silent.push(format!("{} ({peak})", p.name));
            }
        }
        assert!(silent.is_empty(), "silent: {silent:?}");
    }
}
