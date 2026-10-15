// SPDX-License-Identifier: GPL-3.0-or-later
//! Mist: the granular instrument, on the shared voice engine.
//!
//! Ported from `src/audio/synth/granular.ts` of the web version. Every voice
//! sprays short windowed grains read from a source sample. The note sets the
//! grain playback rate (relative to `rootKey`), position and scan choose where
//! grains start, spray randomises that, size and density set grain length and
//! how many start per second. Grains are mixed with a gain of 1/sqrt(overlap),
//! so density changes do not change the level much.
//!
//! Like the original, the arithmetic is done in `f64` and stored in `f32`
//! where the original stores in a `Float32Array` (source, window, buffers).

use crate::engine::{js_round, BaseVoice, DestDef, Rng, Sound, VoiceEngine, COMMON_DESTS};
use crate::patch::{default_engine_json, merge_patch};
use serde::Deserialize;
use serde_json::{json, Value};
use std::f64::consts::{PI, SQRT_2};
use std::sync::Arc;

/// Grains one voice can have sounding at once.
const MAX_GRAINS: usize = 32;
/// Resolution of the window table.
const WIN: usize = 1024;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrainParams {
    /// Start position in the source, 0 to 1.
    pub position: f64,
    /// Position movement while a note is held, source lengths per second.
    pub scan: f64,
    /// Random start offset, seconds (0 to 0.5).
    pub spray: f64,
    /// Grain length, ms (5 to 500).
    pub size: f64,
    /// Grains per second per voice (1 to 100).
    pub density: f64,
    /// Transpose, semitones.
    pub pitch: f64,
    /// Random pitch per grain, semitones (0 to 12).
    pub pitch_rand: f64,
    /// Random stereo placement per grain, 0 to 1.
    pub spread: f64,
    /// Window: 0 is smooth (Hann), 1 is nearly rectangular.
    pub shape: f64,
    /// Probability that a grain plays backwards, 0 to 1.
    pub reverse: f64,
    /// Key that plays the source at its original pitch.
    pub root_key: f64,
    /// Follow the keyboard (off: every key plays the same pitch).
    pub keytrack: bool,
}

/// Mist's own patch fields.
#[derive(Clone, Debug, Deserialize)]
pub struct MistPatch {
    pub grain: GrainParams,
}

/// Mist's own modulation destinations (after the common ones).
pub const MIST_DESTS: [DestDef; 6] = [
    DestDef {
        id: "position",
        range: 1.0,
    },
    DestDef {
        id: "size",
        range: 1.0,
    },
    DestDef {
        id: "density",
        range: 1.0,
    },
    DestDef {
        id: "spray",
        range: 1.0,
    },
    DestDef {
        id: "spread",
        range: 1.0,
    },
    DestDef {
        id: "pitchRand",
        range: 1.0,
    },
];

const FIRST: usize = COMMON_DESTS.len();
const D_POSITION: usize = FIRST;
const D_SIZE: usize = FIRST + 1;
const D_DENSITY: usize = FIRST + 2;
const D_SPRAY: usize = FIRST + 3;
const D_SPREAD: usize = FIRST + 4;
const D_PITCH_RAND: usize = FIRST + 5;

/// The default Mist patch (common and granular fields) as JSON.
pub fn default_mist_json() -> Value {
    json!({
        "env": [{ "a": 0.08, "h": 0, "d": 0.5, "s": 1, "r": 1.2 }, { "a": 0.01, "h": 0, "d": 0.6, "s": 0, "r": 0.3 }],
        "voice": { "poly": 6 },
        "grain": {
            "position": 0.3, "scan": 0, "spray": 0.05, "size": 90, "density": 24,
            "pitch": 0, "pitchRand": 0, "spread": 0.5, "shape": 0.2, "reverse": 0,
            "rootKey": 60, "keytrack": true
        },
    })
}

/// The built-in source used when a channel has no sample: four seconds of a
/// slowly evolving chord (detuned partials with gentle noise), the same on every
/// machine. Returns the left and right channels.
pub fn default_texture(sr: f64, seconds: f64) -> (Vec<f32>, Vec<f32>) {
    let n = (sr * seconds).floor() as usize;
    let mut l = vec![0f32; n];
    let mut r = vec![0f32; n];
    let notes = [48.0, 55.0, 60.0, 64.0, 67.0, 71.0];
    let mut seed: u64 = 12345;
    let mut rnd = move || {
        seed = (seed * 1664525 + 1013904223) & 0xFFFF_FFFF;
        (seed as f64 / 4294967296.0) * 2.0 - 1.0
    };
    let rot = |f: f64| ((2.0 * PI * f / sr).cos(), (2.0 * PI * f / sr).sin());
    for (ni, key) in notes.iter().enumerate() {
        let ni_f = ni as f64;
        let f = 440.0 * 2f64.powf((key - 69.0) / 12.0);
        for h in 1..=6usize {
            let hf = h as f64;
            let amp = 0.08 / hf / (1.0 + ni_f * 0.15);
            let det = 1.0 + (if h % 2 == 1 { 0.002 } else { -0.0015 }) * (ni_f + 1.0);
            let fl = f * hf * det;
            let fr = f * hf / det;
            let lfo_f = 0.1 + 0.05 * ((ni + h) % 5) as f64;
            // rotating phasors instead of a sine per sample
            let (cl, sl) = rot(fl);
            let (cr, sr2) = rot(fr);
            let (cw, sw0) = rot(lfo_f);
            let (mut xl, mut yl) = (1.0f64, 0.0f64);
            let (mut xr, mut yr) = ((0.3 * hf).cos(), (0.3 * hf).sin());
            let (mut xw, mut yw) = ((ni_f + hf).cos(), (ni_f + hf).sin());
            for i in 0..n {
                let sw = 0.6 + 0.4 * yw;
                l[i] = (f64::from(l[i]) + yl * amp * sw) as f32;
                r[i] = (f64::from(r[i]) + yr * amp * sw) as f32;
                let mut t = xl * cl - yl * sl;
                yl = xl * sl + yl * cl;
                xl = t;
                t = xr * cr - yr * sr2;
                yr = xr * sr2 + yr * cr;
                xr = t;
                t = xw * cw - yw * sw0;
                yw = xw * sw0 + yw * cw;
                xw = t;
            }
        }
    }
    let (mut nl, mut nr) = (0.0f64, 0.0f64);
    for i in 0..n {
        nl = nl * 0.995 + rnd() * 0.005;
        nr = nr * 0.995 + rnd() * 0.005;
        l[i] = (f64::from(l[i]) + nl * 0.6) as f32;
        r[i] = (f64::from(r[i]) + nr * 0.6) as f32;
    }
    // normalise like a typical mastered sample (peak 0.9)
    let mut peak = 0.0f64;
    for i in 0..n {
        peak = peak.max(f64::from(l[i]).abs()).max(f64::from(r[i]).abs());
    }
    let g = if peak > 0.0 { 0.9 / peak } else { 1.0 };
    for i in 0..n {
        l[i] = (f64::from(l[i]) * g) as f32;
        r[i] = (f64::from(r[i]) * g) as f32;
    }
    // loopable: a short crossfade between the end and the start
    let xf = (sr * 0.05).floor() as usize;
    for i in 0..xf {
        let k = i as f64 / xf as f64;
        l[i] = (f64::from(l[i]) * k + f64::from(l[n - xf + i]) * (1.0 - k)) as f32;
        r[i] = (f64::from(r[i]) * k + f64::from(r[n - xf + i]) * (1.0 - k)) as f32;
    }
    (l, r)
}

#[derive(Clone, Copy, Default)]
struct Grain {
    active: bool,
    /// Read position in source samples (fractional).
    pos: f64,
    /// Source samples per output sample (negative: reverse).
    inc: f64,
    age: usize,
    len: usize,
    /// Samples into the current block before this grain starts.
    wait: usize,
    gl: f64,
    gr: f64,
}

pub struct MistVoice {
    grains: [Grain; MAX_GRAINS],
    /// Samples until the next grain starts.
    until_next: f64,
    /// Scan offset accumulated while held, fraction of the source.
    scan_pos: f64,
    // control-rate values
    rate: f64,
    start: f64,
    size_s: f64,
    interval: f64,
    spray_s: f64,
    norm: f64,
    /// Which source the grains were read from (see [`Mist::set_source`]).
    epoch: u32,
}

impl Default for MistVoice {
    fn default() -> Self {
        Self {
            grains: [Grain::default(); MAX_GRAINS],
            until_next: 0.0,
            scan_pos: 0.0,
            rate: 1.0,
            start: 0.0,
            size_s: 1.0,
            interval: 1.0,
            spray_s: 0.0,
            norm: 1.0,
            epoch: 0,
        }
    }
}

/// The granular sound generator.
pub struct Mist {
    src_l: Arc<[f32]>,
    src_r: Arc<[f32]>,
    src_rate: f64,
    win: Vec<f32>,
    win_shape: f64,
    patch: Option<MistPatch>,
    /// Counts source changes; a voice drops its grains when it sees a new one.
    epoch: u32,
    /// Grains started since construction (tests and diagnostics).
    pub grains_started: u64,
}

impl Mist {
    pub fn new() -> Self {
        Self {
            src_l: Arc::from(Vec::new()),
            src_r: Arc::from(Vec::new()),
            src_rate: 48000.0,
            win: vec![0.0; WIN + 1],
            win_shape: -1.0,
            patch: None,
            epoch: 1,
            grains_started: 0,
        }
    }

    /// Replace the source sample. Pass the same buffer for both channels for a
    /// mono source. Sounding grains stop.
    ///
    /// The buffers are shared (`Arc`), so a caller that keeps its own handle
    /// does not make the audio thread free a large buffer when it swaps.
    pub fn set_source(&mut self, left: Arc<[f32]>, right: Arc<[f32]>, rate: f64) {
        self.src_l = left;
        self.src_r = right;
        self.src_rate = rate;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Length of the source in samples.
    pub fn source_length(&self) -> usize {
        self.src_l.len()
    }

    fn spawn(&mut self, v: &mut MistVoice, base: &BaseVoice, rng: &mut Rng, at: usize) {
        let len = self.src_l.len();
        if len < 4 {
            return;
        }
        let Some(slot) = v.grains.iter().position(|g| !g.active) else {
            return;
        };
        let Some(p) = &self.patch else { return };
        let g = &p.grain;
        let m = &base.mods;
        let mut rnd = || rng.next_f64();
        let pr = (g.pitch_rand + m[D_PITCH_RAND] * 12.0).clamp(0.0, 12.0);
        let rate = v.rate
            * if pr > 0.0 {
                2f64.powf(((rnd() * 2.0 - 1.0) * pr) / 12.0)
            } else {
                1.0
            };
        let rev = rnd() < g.reverse;
        let lenf = len as f64;
        let mut start = (v.start - v.start.floor()) * lenf + (rnd() * 2.0 - 1.0) * v.spray_s;
        start = ((start % lenf) + lenf) % lenf;
        let spread = (g.spread + m[D_SPREAD]).clamp(0.0, 1.0);
        let pan = (rnd() * 2.0 - 1.0) * spread;
        let a = (pan + 1.0) * 0.25 * PI;
        v.grains[slot] = Grain {
            active: true,
            age: 0,
            wait: at,
            len: js_round(v.size_s).max(16.0) as usize,
            inc: if rev { -rate } else { rate },
            pos: start,
            gl: a.cos() * SQRT_2,
            gr: a.sin() * SQRT_2,
        };
        self.grains_started += 1;
    }
}

impl Default for Mist {
    fn default() -> Self {
        Self::new()
    }
}

impl Sound for Mist {
    type Patch = MistPatch;
    type Voice = MistVoice;

    fn default_json() -> Value {
        default_mist_json()
    }

    fn extra_dests() -> &'static [DestDef] {
        &MIST_DESTS
    }

    fn new_voice(&self) -> MistVoice {
        MistVoice {
            epoch: self.epoch,
            ..MistVoice::default()
        }
    }

    fn patch_changed(&mut self, patch: MistPatch) -> Option<MistPatch> {
        let shape = patch.grain.shape.clamp(0.0, 1.0);
        let old = self.patch.replace(patch);
        if (shape - self.win_shape).abs() < 1e-4 {
            return old;
        }
        self.win_shape = shape;
        // Tukey window: the taper fraction shrinks as shape goes from 0 (Hann) to 1
        let taper = (1.0 - shape).max(0.02);
        for i in 0..=WIN {
            let x = i as f64 / WIN as f64;
            let t = taper / 2.0;
            let mut w = 1.0;
            if x < t {
                w = 0.5 - 0.5 * (PI * x / t).cos();
            } else if x > 1.0 - t {
                w = 0.5 - 0.5 * (PI * (1.0 - x) / t).cos();
            }
            self.win[i] = w as f32;
        }
        old
    }

    fn start(&mut self, v: &mut MistVoice, _base: &BaseVoice, legato: bool) {
        if legato {
            return;
        }
        for g in &mut v.grains {
            g.active = false;
        }
        v.until_next = 0.0;
        v.scan_pos = 0.0;
    }

    fn control(&mut self, v: &mut MistVoice, base: &BaseVoice, n: usize, pitch: f64, sr: f64) {
        let Some(p) = &self.patch else { return };
        let g = &p.grain;
        let m = &base.mods;
        let semis = (if g.keytrack {
            pitch - g.root_key
        } else {
            pitch - base.key
        }) + g.pitch;
        v.rate = 2f64.powf(semis / 12.0) * (self.src_rate / sr);
        v.scan_pos += (g.scan * n as f64) / sr;
        v.scan_pos -= v.scan_pos.floor();
        v.start = g.position + m[D_POSITION] + v.scan_pos;
        v.size_s =
            ((g.size.clamp(5.0, 500.0) / 1000.0) * sr * 2f64.powf(m[D_SIZE] * 3.0)).max(16.0);
        let density = (g.density * 2f64.powf(m[D_DENSITY] * 3.0)).clamp(0.5, 200.0);
        v.interval = sr / density;
        v.spray_s = (g.spray + m[D_SPRAY] * 0.5).clamp(0.0, 0.5) * self.src_rate;
        // expected overlap: grains alive at once
        v.norm = 1.0 / ((v.size_s * density) / sr).max(1.0).sqrt();
    }

    fn render(
        &mut self,
        v: &mut MistVoice,
        base: &BaseVoice,
        rng: &mut Rng,
        l: &mut [f32],
        r: &mut [f32],
        n: usize,
    ) {
        if v.epoch != self.epoch {
            v.epoch = self.epoch;
            for g in &mut v.grains {
                g.active = false;
            }
        }
        let len = self.src_l.len();
        if len < 4 {
            return;
        }
        for s in 0..n {
            // a released voice stops starting new grains; the amp release fades what remains
            v.until_next -= 1.0;
            if v.until_next <= 0.0 {
                if !base.released {
                    self.spawn(v, base, rng, s);
                }
                v.until_next += v.interval;
                if v.until_next < 1.0 {
                    v.until_next = 1.0;
                }
            }
        }
        // second pass: render the active grains for the whole block
        let (sl, sr) = (&self.src_l, &self.src_r);
        let win = &self.win;
        let lenf = len as f64;
        for g in &mut v.grains {
            if !g.active {
                continue;
            }
            let gl = g.gl * v.norm;
            let gr = g.gr * v.norm;
            let from = g.wait;
            g.wait = 0;
            for s in from..n {
                if g.age >= g.len {
                    g.active = false;
                    break;
                }
                let wpos = (g.age as f64 / g.len as f64) * WIN as f64;
                let wi = wpos as usize;
                let w0 = f64::from(win[wi]);
                let w = w0 + (f64::from(win[wi + 1]) - w0) * (wpos - wi as f64);
                let mut p = g.pos;
                if p >= lenf {
                    p -= lenf;
                } else if p < 0.0 {
                    p += lenf;
                }
                g.pos = p;
                let i0 = p as usize;
                let fr = p - i0 as f64;
                let i1 = if i0 + 1 >= len { 0 } else { i0 + 1 };
                let (a0, a1) = (f64::from(sl[i0]), f64::from(sl[i1]));
                let (b0, b1) = (f64::from(sr[i0]), f64::from(sr[i1]));
                l[s] = (f64::from(l[s]) + (a0 + (a1 - a0) * fr) * w * gl) as f32;
                r[s] = (f64::from(r[s]) + (b0 + (b1 - b0) * fr) * w * gr) as f32;
                g.pos += g.inc;
                g.age += 1;
            }
        }
    }
}

pub type MistEngine = VoiceEngine<Mist>;

/// A Mist engine with the built-in texture as its source.
pub fn mist_engine(sample_rate: f64, seed: u32) -> MistEngine {
    let mut e = VoiceEngine::new(Mist::new(), sample_rate, seed);
    let (l, r) = default_texture(sample_rate, 4.0);
    e.sound.set_source(Arc::from(l), Arc::from(r), sample_rate);
    e
}

/// Defaults of the whole patch (common and granular fields).
pub fn full_default_json() -> Value {
    merge_patch(&default_engine_json(), &default_mist_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Event;

    fn render(e: &mut MistEngine, seconds: f64) -> Vec<f32> {
        let n = (seconds * e.sample_rate()) as usize;
        let mut l = vec![0f32; n];
        let mut r = vec![0f32; n];
        let mut i = 0;
        while i < n {
            let end = (i + 128).min(n);
            e.process(&mut l[i..end], &mut r[i..end], i as f64 / e.sample_rate());
            i = end;
        }
        l.extend(r);
        l
    }

    fn peak(b: &[f32]) -> f32 {
        b.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    fn note(e: &mut MistEngine, key: f64, on: f64, off: f64) {
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

    #[test]
    fn the_default_texture_is_normalised_and_loops() {
        let (l, r) = default_texture(8000.0, 1.0);
        assert_eq!(l.len(), 8000);
        let p = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
        assert!(p <= 0.9 + 1e-6 && p > 0.5, "peak {p}");
        // the loop point: the first sample is a blend, so it sits near the last
        assert!((l[0] - l[7999]).abs() < 0.2);
    }

    #[test]
    fn a_note_makes_grains_and_then_fades() {
        let mut e = mist_engine(48000.0, 1);
        note(&mut e, 60.0, 0.0, 0.5);
        let out = render(&mut e, 4.0);
        assert!(peak(&out) > 0.02, "audible");
        assert!(e.sound.grains_started > 5);
        let tail = &out[out.len() / 2 - 2000..out.len() / 2];
        assert!(peak(tail) < 1e-4, "silent after the release");
    }

    #[test]
    fn the_same_seed_gives_the_same_audio() {
        let go = || {
            let mut e = mist_engine(48000.0, 7);
            e.set_patch(
                &json!({ "grain": { "spray": 0.3, "pitchRand": 5, "reverse": 0.5, "spread": 1 } }),
            )
            .unwrap();
            note(&mut e, 64.0, 0.0, 0.4);
            render(&mut e, 1.0)
        };
        assert_eq!(go(), go());
    }

    #[test]
    fn an_empty_source_is_silent() {
        let mut e = VoiceEngine::new(Mist::new(), 48000.0, 1);
        note(&mut e, 60.0, 0.0, 0.2);
        assert_eq!(peak(&render(&mut e, 0.5)), 0.0);
    }

    #[test]
    fn replacing_the_source_stops_old_grains() {
        let mut e = mist_engine(48000.0, 1);
        e.queue(&[Event::On {
            time: 0.0,
            key: 60.0,
            vel: 1.0,
            fine: 0.0,
            pan: 0.0,
        }]);
        let mut l = vec![0f32; 4800];
        let mut r = vec![0f32; 4800];
        e.process(&mut l, &mut r, 0.0);
        assert!(peak(&l) > 0.0);
        e.sound.set_source(
            Arc::from(vec![0f32; 100]),
            Arc::from(vec![0f32; 100]),
            48000.0,
        );
        assert_eq!(e.sound.source_length(), 100);
        l.fill(0.0);
        r.fill(0.0);
        e.process(&mut l, &mut r, 0.1);
        // a silent source: whatever sounds now reads zeros
        assert_eq!(peak(&l), 0.0);
    }
}
