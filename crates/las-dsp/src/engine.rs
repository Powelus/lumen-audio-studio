// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! Shared polyphonic voice engine for the engine-based instruments (FM, granular).
//!
//! Ported from `src/audio/synth/engine.ts` of the web version, step by step, so
//! that the same patch and notes render the same audio (see `docs/PARITY.md`).
//!
//! The core owns what the instruments have in common: timed note events, voice
//! allocation, stealing, mono, legato and glide, the amp and modulation
//! envelopes, two LFOs (free or tempo-synced), one modulation matrix, an output
//! filter per voice, velocity, pan and the safety clip. An instrument only adds
//! its own sound generator (the [`Sound`] trait) and destinations.
//!
//! Not ported yet: parameter ramps (automation) and tempo changes over time.

use crate::patch::{
    default_engine_json, merge_patch, set_path, EnginePatch, EnvPatch, FilterKind, LfoShape, Ramps,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::f64::consts::PI;
use std::fmt;

/// A patch that cannot be used (wrong types, unknown names).
#[derive(Debug)]
pub struct PatchError(pub String);

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid patch: {}", self.0)
    }
}

impl std::error::Error for PatchError {}

/// Modulation sources available to the engine-based instruments, in matrix order.
pub const ENGINE_SOURCES: [&str; 13] = [
    "env2", "lfo1", "lfo2", "vel", "note", "mw", "at", "pb", "rand", "macro1", "macro2", "macro3",
    "macro4",
];

/// A modulation destination: its name and the value of amount 1.
#[derive(Clone, Copy, Debug)]
pub struct DestDef {
    pub id: &'static str,
    pub range: f64,
}

/// Destinations every engine has; instruments append their own.
pub const COMMON_DESTS: [DestDef; 5] = [
    DestDef {
        id: "pitch",
        range: 24.0,
    },
    DestDef {
        id: "amp",
        range: 1.0,
    },
    DestDef {
        id: "pan",
        range: 1.0,
    },
    DestDef {
        id: "cutoff",
        range: 8.0,
    },
    DestDef {
        id: "res",
        range: 1.0,
    },
];
const D_PITCH: usize = 0;
const D_AMP: usize = 1;
const D_PAN: usize = 2;
const D_CUT: usize = 3;
const D_RES: usize = 4;

/// A timed event for the engine. `time` is in seconds on the engine's clock.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    On {
        time: f64,
        key: f64,
        vel: f64,
        fine: f64,
        pan: f64,
    },
    Off {
        time: f64,
        key: f64,
    },
    /// A note off whose release takes `scale` times as long as the patch's (FL's note
    /// release: 1 leaves it as it is).
    OffScaled {
        time: f64,
        key: f64,
        scale: f64,
    },
    AllOff {
        time: f64,
    },
    /// Controller: modulation wheel, aftertouch or pitch bend.
    Cc {
        time: f64,
        cc: Cc,
        value: f64,
    },
    /// The channel's pitch offset (semitones) moved: notes already sounding follow
    /// by the change. New notes bring their own offset in `fine`.
    Tune {
        time: f64,
        semis: f64,
    },
}

impl Event {
    pub fn time(&self) -> f64 {
        match *self {
            Event::On { time, .. }
            | Event::Off { time, .. }
            | Event::OffScaled { time, .. }
            | Event::AllOff { time }
            | Event::Cc { time, .. }
            | Event::Tune { time, .. } => time,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cc {
    Mw,
    At,
    Pb,
}

/// Small deterministic PRNG (mulberry32): renders repeat exactly.
#[derive(Clone, Debug)]
pub struct Rng {
    a: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self { a: seed }
    }

    /// Next value in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        self.a = self.a.wrapping_add(0x6d2b_79f5);
        let mut t = self.a;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

fn flush(x: f64) -> f64 {
    if x.abs() < 1e-20 {
        0.0
    } else {
        x
    }
}

/// JavaScript's `Math.round`: halves round up.
pub(crate) fn js_round(x: f64) -> f64 {
    crate::floor(x + 0.5)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

/// ADSR with hold: linear attack, exponential decay and release.
#[derive(Clone, Debug)]
pub struct Env {
    stage: Stage,
    pub level: f64,
    hold_left: f64,
    /// How much longer than the patch's this note's release is.
    rel_scale: f64,
    /// The last decay or release coefficient, with the step and time it was made for:
    /// the amplitude envelope steps every sample, and `exp` each time is costly.
    coef: (f64, f64, f64),
}

impl Default for Env {
    fn default() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            hold_left: 0.0,
            rel_scale: 1.0,
            coef: (f64::NAN, f64::NAN, 0.0),
        }
    }
}

impl Env {
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    pub fn trigger(&mut self, from_zero: bool) {
        if from_zero {
            self.level = 0.0;
        }
        self.stage = Stage::Attack;
        self.rel_scale = 1.0;
    }

    pub fn release(&mut self) {
        self.release_scaled(1.0);
    }

    /// Release, taking `scale` times the patch's release time.
    pub fn release_scaled(&mut self, scale: f64) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
            self.rel_scale = if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            };
        }
    }

    /// `exp(-4.6 * n / time)`, remembered while `n` and `time` stay the same.
    fn coef(&mut self, n: f64, time: f64) -> f64 {
        if self.coef.0 != n || self.coef.1 != time {
            self.coef = (n, time, ((-4.6 * n) / time).exp());
        }
        self.coef.2
    }

    /// Advance by `n` samples; returns the level.
    pub fn step(&mut self, p: &EnvPatch, n: f64, sr: f64) -> f64 {
        match self.stage {
            Stage::Attack => {
                self.level += n / (p.a * sr).max(1.0);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = if p.h > 0.0 { Stage::Hold } else { Stage::Decay };
                    self.hold_left = p.h * sr;
                }
            }
            Stage::Hold => {
                self.hold_left -= n;
                if self.hold_left <= 0.0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let coef = self.coef(n, (p.d * sr).max(1.0));
                self.level = p.s + (self.level - p.s) * coef;
                if (self.level - p.s).abs() < 0.0005 {
                    self.level = p.s;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = p.s,
            Stage::Release => {
                let coef = self.coef(n, (p.r * self.rel_scale * sr).max(1.0));
                self.level *= coef;
                if self.level < 0.0002 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Idle => {}
        }
        self.level
    }
}

/// Zero-delay-feedback state variable filter (one per channel).
#[derive(Clone, Debug)]
struct Svf {
    ic1: [f64; 2],
    ic2: [f64; 2],
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    kind: FilterKind,
}

impl Default for Svf {
    fn default() -> Self {
        Self {
            ic1: [0.0; 2],
            ic2: [0.0; 2],
            k: 1.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            kind: FilterKind::Lp,
        }
    }
}

impl Svf {
    fn set(&mut self, cutoff: f64, res: f64, sr: f64) {
        let fc = (sr * 0.45).min(cutoff.max(20.0));
        let g = (PI * fc / sr).tan();
        self.k = 2.0 - 1.96 * res.clamp(0.0, 1.0);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    fn run(&mut self, c: usize, x: f64) -> f64 {
        let v3 = x - self.ic2[c];
        let v1 = self.a1 * self.ic1[c] + self.a2 * v3;
        let v2 = self.ic2[c] + self.a2 * self.ic1[c] + self.a3 * v3;
        self.ic1[c] = flush(2.0 * v1 - self.ic1[c]);
        self.ic2[c] = flush(2.0 * v2 - self.ic2[c]);
        match self.kind {
            FilterKind::Lp => v2,
            FilterKind::Bp => v1,
            FilterKind::Hp => x - self.k * v1 - v2,
        }
    }
}

/// The part of a voice every instrument shares.
#[derive(Clone, Debug)]
pub struct BaseVoice {
    pub active: bool,
    pub released: bool,
    /// Being stolen: fades out over a few milliseconds.
    killing: bool,
    kill_gain: f64,
    pub key: f64,
    pub vel: f64,
    pub fine: f64,
    pub note_pan: f64,
    age: u64,
    pub rand: f64,
    /// Current and target pitch in semitones (MIDI note numbers), for glide.
    pub pitch: f64,
    pub target_pitch: f64,
    amp: Env,
    mod_env: Env,
    lfo_phase: [f64; 2],
    lfo_hold: [f64; 2],
    lfo_prev: [f64; 2],
    lfo_time: f64,
    gain: f64,
    filter: Svf,
    filter_on: bool,
    /// Modulation sums per destination for the current control block.
    pub mods: Vec<f64>,
}

impl BaseVoice {
    fn new(num_dests: usize) -> Self {
        Self {
            active: false,
            released: false,
            killing: false,
            kill_gain: 1.0,
            key: 60.0,
            vel: 1.0,
            fine: 0.0,
            note_pan: 0.0,
            age: 0,
            rand: 0.0,
            pitch: 60.0,
            target_pitch: 60.0,
            amp: Env::default(),
            mod_env: Env::default(),
            lfo_phase: [0.0; 2],
            lfo_hold: [0.0; 2],
            lfo_prev: [0.0; 2],
            lfo_time: 0.0,
            gain: 0.0,
            filter: Svf::default(),
            filter_on: false,
            mods: vec![0.0; num_dests],
        }
    }
}

/// One voice: the shared part and the instrument's own.
pub struct Voice<V> {
    pub base: BaseVoice,
    pub sound: V,
}

/// The instrument-specific part of an engine.
pub trait Sound {
    /// The instrument's own patch fields (parsed from the same JSON as the common ones).
    type Patch: DeserializeOwned;
    /// Per-voice state of the sound generator.
    type Voice;

    /// Defaults of the whole patch (common and instrument fields) as JSON.
    fn default_json() -> Value;
    /// Destinations after the common ones.
    fn extra_dests() -> &'static [DestDef];
    /// Fixed output trim, so each instrument's default level sits with the others.
    fn output_trim() -> f64 {
        1.0
    }
    fn new_voice(&self) -> Self::Voice;
    /// The patch changed (validated and merged with the defaults).
    ///
    /// Returns the previous instrument patch, so the caller can free it off the
    /// audio thread.
    fn patch_changed(&mut self, patch: Self::Patch) -> Option<Self::Patch>;
    /// A note starts (`legato`: it continues the sounding voice instead).
    fn start(&mut self, v: &mut Self::Voice, base: &BaseVoice, legato: bool);
    /// Control-rate update. `pitch` is the final note pitch in semitones.
    fn control(&mut self, v: &mut Self::Voice, base: &BaseVoice, n: usize, pitch: f64, sr: f64);
    /// Render `n` samples of the voice's raw sound into `l` and `r` (already zeroed).
    ///
    /// `rng` is the engine's shared random stream (the same one the LFOs and
    /// voices draw from), so renders repeat exactly.
    fn render(
        &mut self,
        v: &mut Self::Voice,
        base: &BaseVoice,
        rng: &mut Rng,
        l: &mut [f32],
        r: &mut [f32],
        n: usize,
    );
}

const CONTROL: usize = 32;

/// A patch parsed and checked ahead of time, ready for [`VoiceEngine::apply_patch`].
pub struct PreparedPatch<S: Sound> {
    common: EnginePatch,
    own: Option<S::Patch>,
    json: Value,
    mod_src: Vec<usize>,
    mod_dst: Vec<usize>,
    mod_amt: Vec<f64>,
    want: usize,
}

/// Voices an engine keeps ready: 32 polyphony plus 4 for the release tails.
pub const MAX_VOICES: usize = 36;

/// Parse a patch (only the values that differ from the defaults need to be
/// present). Do this off the audio thread, then hand the result to the engine.
pub fn prepare_patch<S: Sound>(over: &Value) -> Result<PreparedPatch<S>, PatchError> {
    let defaults = merge_patch(&default_engine_json(), &S::default_json());
    let merged = merge_patch(&defaults, over);
    // parse both parts first, so a bad patch changes nothing
    let common: EnginePatch =
        serde_json::from_value(merged.clone()).map_err(|e| PatchError(e.to_string()))?;
    let own: S::Patch =
        serde_json::from_value(merged.clone()).map_err(|e| PatchError(e.to_string()))?;
    let mut dests: Vec<DestDef> = COMMON_DESTS.to_vec();
    dests.extend_from_slice(S::extra_dests());
    let mut mod_src = Vec::new();
    let mut mod_dst = Vec::new();
    let mut mod_amt = Vec::new();
    for m in &common.mods {
        let src = ENGINE_SOURCES.iter().position(|x| *x == m.src);
        let dst = dests.iter().position(|d| d.id == m.dest);
        if let (Some(src), Some(dst)) = (src, dst) {
            if m.amt != 0.0 {
                mod_src.push(src);
                mod_dst.push(dst);
                mod_amt.push(m.amt);
            }
        }
    }
    let voice = &common.voice;
    let want = (if voice.mono { 1.0 } else { voice.poly }).clamp(1.0, 32.0) as usize + 4;
    Ok(PreparedPatch {
        common,
        own: Some(own),
        json: merged,
        mod_src,
        mod_dst,
        mod_amt,
        want,
    })
}

/// What the app and the command line tool need from any playable instrument,
/// whatever its engine.
pub trait Playable {
    fn sample_rate(&self) -> f64;
    /// Set the patch from JSON (only what differs from the defaults).
    fn set_patch(&mut self, over: &Value) -> Result<(), PatchError>;
    fn queue(&mut self, evs: &[Event]);
    /// Render a block, adding to `l` and `r`, starting at engine time `t0` seconds.
    fn process(&mut self, l: &mut [f32], r: &mut [f32], t0: f64);
    fn active_voices(&self) -> usize;
    fn all_off(&mut self);
}

impl<S: Sound> Playable for VoiceEngine<S> {
    fn sample_rate(&self) -> f64 {
        VoiceEngine::sample_rate(self)
    }
    fn set_patch(&mut self, over: &Value) -> Result<(), PatchError> {
        VoiceEngine::set_patch(self, over)
    }
    fn queue(&mut self, evs: &[Event]) {
        VoiceEngine::queue(self, evs)
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32], t0: f64) {
        VoiceEngine::process(self, l, r, t0)
    }
    fn active_voices(&self) -> usize {
        VoiceEngine::active_voices(self)
    }
    fn all_off(&mut self) {
        VoiceEngine::all_off(self)
    }
}

/// A polyphonic engine around a [`Sound`].
pub struct VoiceEngine<S: Sound> {
    pub sound: S,
    pub patch: EnginePatch,
    patch_json: Value,
    ramps: Ramps,
    voices: Vec<Voice<S::Voice>>,
    dests: Vec<DestDef>,
    sr: f64,
    rng: Rng,
    events: Vec<Event>,
    held_keys: Vec<f64>,
    last_key: f64,
    age_counter: u64,
    mw: f64,
    pb: f64,
    at: f64,
    /// The channel pitch offset last heard (see [`Event::Tune`]).
    tune: Option<f64>,
    bpm: f64,
    anchor_time: f64,
    anchor_beat: f64,
    mod_src: Vec<usize>,
    mod_dst: Vec<usize>,
    mod_amt: Vec<f64>,
    src_vals: [f64; 13],
    buf_l: [f32; CONTROL],
    buf_r: [f32; CONTROL],
}

impl<S: Sound> VoiceEngine<S> {
    pub fn new(sound: S, sample_rate: f64, seed: u32) -> Self {
        let mut dests: Vec<DestDef> = COMMON_DESTS.to_vec();
        dests.extend_from_slice(S::extra_dests());
        let default = merge_patch(&default_engine_json(), &S::default_json());
        let mut e = Self {
            sound,
            patch: serde_json::from_value(default.clone()).expect("default engine patch"),
            patch_json: default,
            ramps: Ramps::default(),
            voices: Vec::new(),
            dests,
            sr: sample_rate,
            rng: Rng::new(seed),
            events: Vec::new(),
            held_keys: Vec::new(),
            last_key: -1.0,
            age_counter: 0,
            mw: 0.0,
            pb: 0.0,
            at: 0.0,
            tune: None,
            bpm: 120.0,
            anchor_time: 0.0,
            anchor_beat: 0.0,
            mod_src: Vec::new(),
            mod_dst: Vec::new(),
            mod_amt: Vec::new(),
            src_vals: [0.0; 13],
            buf_l: [0.0; CONTROL],
            buf_r: [0.0; CONTROL],
        };
        e.set_patch(&Value::Object(Default::default()))
            .expect("the defaults are a valid patch");
        // all the voices up front, so later patch changes never allocate
        while e.voices.len() < MAX_VOICES {
            let sound = e.sound.new_voice();
            e.voices.push(Voice {
                base: BaseVoice::new(e.dests.len()),
                sound,
            });
        }
        e
    }

    /// Set the patch from JSON. Only the values that differ from the defaults
    /// need to be present.
    pub fn set_patch(&mut self, over: &Value) -> Result<(), PatchError> {
        let mut prepared = prepare_patch::<S>(over)?;
        self.apply_patch(&mut prepared);
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
        if let Ok(mut p) = prepare_patch::<S>(&json) {
            self.apply_patch(&mut p);
        }
    }

    /// Switch to a patch made by [`prepare_patch`]. Does no parsing and, once
    /// the engine exists, no allocation and no freeing, so it is safe on the audio
    /// thread: the patch trades places with the current one, which stays in `p`
    /// for the caller to drop elsewhere (or to switch back to).
    pub fn apply_patch(&mut self, p: &mut PreparedPatch<S>) {
        let own = p
            .own
            .take()
            .expect("a prepared patch always holds its patch");
        std::mem::swap(&mut self.patch, &mut p.common);
        std::mem::swap(&mut self.patch_json, &mut p.json);
        std::mem::swap(&mut self.mod_src, &mut p.mod_src);
        std::mem::swap(&mut self.mod_dst, &mut p.mod_dst);
        std::mem::swap(&mut self.mod_amt, &mut p.mod_amt);
        while self.voices.len() < p.want {
            let sound = self.sound.new_voice();
            self.voices.push(Voice {
                base: BaseVoice::new(self.dests.len()),
                sound,
            });
        }
        p.own = self.sound.patch_changed(own);
    }

    /// Tempo for synced LFOs.
    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm;
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// Add timed events (kept in time order, equal times in the order given).
    pub fn queue(&mut self, evs: &[Event]) {
        self.events.extend_from_slice(evs);
        self.events.sort_by(|a, b| a.time().total_cmp(&b.time()));
    }

    /// Number of sounding voices.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.base.active).count()
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
            .filter(|v| v.base.active && !v.base.killing)
            .count() as f64;
        if playing >= limit {
            let mut victim: Option<usize> = None;
            for (i, v) in self.voices.iter().enumerate() {
                if v.base.active
                    && !v.base.killing
                    && v.base.released
                    && victim.is_none_or(|j| v.base.age < self.voices[j].base.age)
                {
                    victim = Some(i);
                }
            }
            if victim.is_none() {
                for (i, v) in self.voices.iter().enumerate() {
                    if v.base.active
                        && !v.base.killing
                        && victim.is_none_or(|j| v.base.age < self.voices[j].base.age)
                    {
                        victim = Some(i);
                    }
                }
            }
            if let Some(j) = victim {
                self.voices[j].base.killing = true;
            }
        }
        if let Some(i) = self.voices.iter().position(|v| !v.base.active) {
            return i;
        }
        let mut oldest = 0;
        for (i, v) in self.voices.iter().enumerate() {
            if v.base.age < self.voices[oldest].base.age {
                oldest = i;
            }
        }
        oldest
    }

    fn beat_at(&self, time: f64) -> f64 {
        self.anchor_beat + ((time - self.anchor_time) * self.bpm) / 60.0
    }

    #[allow(clippy::too_many_arguments)]
    fn start_voice(
        &mut self,
        idx: usize,
        key: f64,
        vel: f64,
        fine: f64,
        pan: f64,
        legato_from: Option<f64>,
        time: f64,
    ) {
        let glide = self.patch.voice.glide;
        let legato = self.patch.voice.legato;
        let beat = self.beat_at(time);
        let last_key = self.last_key;
        self.age_counter += 1;
        let age = self.age_counter;
        let rand = self.rng.next_f64() * 2.0 - 1.0;
        let v = &mut self.voices[idx];
        let b = &mut v.base;
        let was_active = b.active && !b.released;
        b.active = true;
        b.released = false;
        b.killing = false;
        b.kill_gain = 1.0;
        b.key = key;
        b.vel = vel;
        b.fine = fine;
        b.note_pan = pan;
        b.age = age;
        b.rand = rand;
        b.target_pitch = key;
        let from = legato_from.unwrap_or(if glide > 0.0 && last_key >= 0.0 {
            last_key
        } else {
            key
        });
        b.pitch = if glide > 0.0 { from } else { key };
        if legato_from.is_some() && was_active {
            return;
        }
        let from_zero = !legato || !was_active;
        b.amp.trigger(from_zero);
        b.mod_env.trigger(from_zero);
        b.lfo_time = 0.0;
        for i in 0..2 {
            let Some(l) = self.patch.lfo.get(i) else {
                continue;
            };
            if l.retrig {
                b.lfo_phase[i] = l.phase;
            } else {
                let x = if l.sync {
                    beat / l.div.max(1e-3)
                } else {
                    time * l.rate
                };
                b.lfo_phase[i] = (x + l.phase) % 1.0;
            }
            b.lfo_hold[i] = self.rng.next_f64() * 2.0 - 1.0;
            b.lfo_prev[i] = b.lfo_hold[i];
        }
        b.filter.reset();
        self.sound.start(&mut v.sound, &v.base, false);
    }

    pub fn note_on(&mut self, key: f64, vel: f64, fine: f64, pan: f64, time: f64) {
        self.held_keys.retain(|k| *k != key);
        self.held_keys.push(key);
        let voice = self.patch.voice;
        if voice.mono {
            let found = self
                .voices
                .iter()
                .position(|x| x.base.active && !x.base.killing && !x.base.released);
            match found {
                Some(i) if voice.legato => {
                    let from = self.voices[i].base.pitch;
                    self.start_voice(i, key, vel, fine, pan, Some(from), time);
                    let v = &mut self.voices[i];
                    self.sound.start(&mut v.sound, &v.base, true);
                }
                Some(i) => {
                    let from = if voice.glide > 0.0 {
                        Some(self.voices[i].base.pitch)
                    } else {
                        None
                    };
                    self.start_voice(i, key, vel, fine, pan, from, time);
                    let b = &mut self.voices[i].base;
                    b.amp.trigger(false);
                    b.mod_env.trigger(false);
                }
                None => {
                    for x in &mut self.voices {
                        if x.base.active {
                            x.base.killing = true;
                        }
                    }
                    let i = self.voices.iter().position(|x| !x.base.active).unwrap_or(0);
                    self.start_voice(i, key, vel, fine, pan, None, time);
                }
            }
        } else {
            let i = self.alloc_voice();
            self.start_voice(i, key, vel, fine, pan, None, time);
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
                .find(|x| x.base.active && !x.base.released && x.base.key == key)
            {
                let back = self.held_keys[self.held_keys.len() - 1];
                v.base.target_pitch = back;
                v.base.key = back;
                if voice.glide <= 0.0 {
                    v.base.pitch = back;
                }
            }
            return;
        }
        for v in &mut self.voices {
            if v.base.active && !v.base.released && v.base.key == key {
                v.base.released = true;
                v.base.amp.release_scaled(scale);
                v.base.mod_env.release_scaled(scale);
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
            if !v.base.active {
                continue;
            }
            v.base.released = true;
            v.base.amp.release();
            v.base.mod_env.release();
        }
    }

    // --- control rate ----------------------------------------------------------
    fn lfo_value(
        rng: &mut Rng,
        shape: LfoShape,
        ph: f64,
        b: &mut BaseVoice,
        i: usize,
        wrapped: bool,
    ) -> f64 {
        match shape {
            LfoShape::Sine => (2.0 * PI * ph).sin(),
            LfoShape::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
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
                    b.lfo_hold[i] = rng.next_f64() * 2.0 - 1.0;
                }
                b.lfo_hold[i]
            }
            LfoShape::Smooth => {
                if wrapped {
                    b.lfo_prev[i] = b.lfo_hold[i];
                    b.lfo_hold[i] = rng.next_f64() * 2.0 - 1.0;
                }
                let k = 0.5 - 0.5 * (PI * ph).cos();
                b.lfo_prev[i] + (b.lfo_hold[i] - b.lfo_prev[i]) * k
            }
        }
    }

    fn control(&mut self, idx: usize, n: usize) {
        let sr = self.sr;
        let nf = n as f64;
        let mut s = self.src_vals;
        {
            let b = &mut self.voices[idx].base;
            s[0] = b.mod_env.step(&self.patch.env[1], nf, sr);
            b.lfo_time += nf / sr;
            for i in 0..2 {
                let Some(l) = self.patch.lfo.get(i) else {
                    s[1 + i] = 0.0;
                    continue;
                };
                let rate = if l.sync {
                    self.bpm / 60.0 / l.div.max(1e-3)
                } else {
                    l.rate
                };
                let mut ph = b.lfo_phase[i] + (rate * nf) / sr;
                let wrapped = ph >= 1.0;
                ph -= ph.floor();
                b.lfo_phase[i] = ph;
                let mut val = Self::lfo_value(&mut self.rng, l.shape, ph, b, i, wrapped);
                if l.delay > 0.0 && b.lfo_time < l.delay {
                    val *= b.lfo_time / l.delay;
                }
                s[1 + i] = val;
            }
            s[3] = b.vel;
            s[4] = (b.key - 60.0) / 60.0;
            s[5] = self.mw;
            s[6] = self.at;
            s[7] = self.pb;
            s[8] = b.rand;
            for i in 0..4 {
                s[9 + i] = self.patch.macros.get(i).copied().unwrap_or(0.0);
            }
            b.mods.fill(0.0);
            for i in 0..self.mod_src.len() {
                let d = self.mod_dst[i];
                b.mods[d] += s[self.mod_src[i]] * self.mod_amt[i] * self.dests[d].range;
            }
        }
        let p = &self.patch;
        let b = &mut self.voices[idx].base;
        if p.voice.glide > 0.0 {
            b.pitch +=
                (b.target_pitch - b.pitch) * (1.0 - (-nf / (p.voice.glide * sr * 0.4)).exp());
        } else {
            b.pitch = b.target_pitch;
        }
        let pitch =
            b.pitch + p.tune + b.fine / 100.0 + self.pb * p.voice.pb_range + b.mods[D_PITCH];
        let f = p.filter;
        b.filter_on = f.on;
        if f.on {
            let oct = f.env_amt * s[0]
                + f.vel_amt * b.vel
                + (f.keytrack * (b.key - 60.0)) / 12.0
                + b.mods[D_CUT];
            b.filter.kind = f.kind;
            b.filter
                .set(f.cutoff * 2f64.powf(oct), f.res + b.mods[D_RES], sr);
        }
        let v = &mut self.voices[idx];
        self.sound.control(&mut v.sound, &v.base, n, pitch, sr);
    }

    // --- audio -------------------------------------------------------------------
    /// Render one block, adding to `l` and `r` (which must have the same length),
    /// starting at engine time `t0` seconds.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], t0: f64) {
        let frames = l.len();
        let sr = self.sr;
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
                    } => self.note_on(key, vel, fine, pan, t0 + pos as f64 / sr),
                    Event::Off { key, .. } => self.note_off(key),
                    Event::OffScaled { key, scale, .. } => self.note_off_scaled(key, scale),
                    Event::AllOff { .. } => self.all_off(),
                    Event::Cc { cc, value, .. } => match cc {
                        Cc::Mw => self.mw = value,
                        Cc::At => self.at = value,
                        Cc::Pb => self.pb = value,
                    },
                    Event::Tune { semis, .. } => {
                        let delta = semis - self.tune.unwrap_or(semis);
                        self.tune = Some(semis);
                        if delta != 0.0 {
                            for v in &mut self.voices {
                                if v.base.active {
                                    v.base.fine += delta * 100.0;
                                }
                            }
                        }
                    }
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
                if self.voices[i].base.active {
                    self.render_voice(i, l, r, pos, n);
                }
            }
            pos += n;
        }
        for x in l.iter_mut().chain(r.iter_mut()) {
            if *x > 1.5 || *x < -1.5 {
                *x = f64::from(*x).tanh() as f32;
            }
        }
    }

    fn render_voice(&mut self, idx: usize, l: &mut [f32], r: &mut [f32], start: usize, n: usize) {
        self.control(idx, n);
        let sr = self.sr;
        let p = &self.patch;
        let v = &mut self.voices[idx];
        let env = v.base.amp.step(&p.env[0], n as f64, sr);
        let m = &v.base.mods;
        let vs = p.voice.vel_sens.clamp(0.0, 1.0);
        let target = env
            * (1.0 - vs + vs * v.base.vel)
            * p.level
            * S::output_trim()
            * (1.0 + m[D_AMP]).max(0.0);
        let g0 = v.base.gain;
        let kill0 = v.base.kill_gain;
        if v.base.killing {
            v.base.kill_gain = (v.base.kill_gain - n as f64 / (0.004 * sr)).max(0.0);
        }
        let pan = (v.base.note_pan + m[D_PAN]).clamp(-1.0, 1.0);
        let a = (pan + 1.0) * 0.25 * PI;
        let pl = a.cos() * std::f64::consts::SQRT_2;
        let pr = a.sin() * std::f64::consts::SQRT_2;
        self.buf_l[..n].fill(0.0);
        self.buf_r[..n].fill(0.0);
        self.sound.render(
            &mut v.sound,
            &v.base,
            &mut self.rng,
            &mut self.buf_l,
            &mut self.buf_r,
            n,
        );
        for i in 0..n {
            let k = (i as f64 + 1.0) / n as f64;
            let g = (g0 + (target - g0) * k) * (kill0 + (v.base.kill_gain - kill0) * k);
            let mut ls = f64::from(self.buf_l[i]);
            let mut rs = f64::from(self.buf_r[i]);
            if v.base.filter_on {
                ls = v.base.filter.run(0, ls);
                rs = v.base.filter.run(1, rs);
            }
            l[start + i] = (f64::from(l[start + i]) + ls * g * pl) as f32;
            r[start + i] = (f64::from(r[start + i]) + rs * g * pr) as f32;
        }
        v.base.gain = target;
        if v.base.amp.is_idle() || (v.base.killing && v.base.kill_gain <= 0.0) {
            v.base.active = false;
            v.base.gain = 0.0;
        }
    }
}
