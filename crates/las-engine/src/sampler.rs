// SPDX-License-Identifier: GPL-3.0-or-later
//! The channel sampler: one `AudioBufferSourceNode` per note into a gain
//! envelope and a stereo panner, then a shared filter and boost. Port of the
//! web version's `instruments/sampler.ts`, with the Web Audio nodes behaving
//! as they do in Chromium (sub-sample start times, linear interpolation,
//! parameter timelines).

use crate::factory::Audio;
use crate::instrument::stereo_pan;
use crate::param::{Param, Timeline};
use las_dsp::fx::native::WebBiquad;
use las_dsp::param::Glides;
use serde_json::Value;
use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

fn num(v: Option<&Value>, d: f64) -> f64 {
    v.and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(d)
}

/// `Math.round`.
fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// What a `StereoPannerNode` does to a mono signal: equal power.
fn mono_pan(x: f32, pan: f64) -> (f32, f32) {
    let t = ((pan.clamp(-1.0, 1.0) + 1.0) / 2.0) as f32;
    (x * (t * FRAC_PI_2).cos(), x * (t * FRAC_PI_2).sin())
}

/// A buffer source: where it starts, how it reads and when it stops.
pub struct SourcePlayback {
    pub buf: Arc<Audio>,
    /// Seconds (may fall between frames).
    pub start: f64,
    /// Frames into the buffer where the first output frame reads.
    pub offset: f64,
    /// Buffer frames per output frame.
    pub rate: f64,
    pub stop: Option<f64>,
    /// Loop points, in buffer frames.
    pub loop_points: Option<(f64, f64)>,
}

impl SourcePlayback {
    /// Frame at which the source begins to sound.
    pub fn first_frame(&self, sr: f64) -> usize {
        // Chromium works the frame out in `f32`
        f64::from((self.start * sr) as f32).ceil().max(0.0) as usize
    }

    /// The output at `frame`: the buffer's channels, interpolated, or `None`
    /// once the source is over.
    pub fn read(&self, frame: usize, sr: f64) -> Option<[f32; 2]> {
        if let Some(stop) = self.stop {
            if frame as f64 >= f64::from((stop * sr) as f32).ceil() {
                return None;
            }
        }
        let len = self.buf.frames();
        let mut pos = self.offset + (frame as f64 - self.start * sr) * self.rate;
        let mut looping = false;
        if let Some((ls, le)) = self.loop_points {
            if le > ls && len > 0 {
                if pos >= le {
                    pos = ls + (pos - ls) % (le - ls);
                }
                looping = true;
            }
        }
        if pos < 0.0 || (!looping && pos >= len as f64) {
            return None;
        }
        let i1 = pos.floor() as usize;
        let frac = (pos - pos.floor()) as f32;
        let mut out = [0f32; 2];
        let n = self.buf.channels.len();
        for (c, o) in out.iter_mut().enumerate().take(n.min(2)) {
            let ch = &self.buf.channels[c];
            let s1 = ch.get(i1).copied().unwrap_or(0.0);
            // the next sample is the next one in the buffer, even across the loop's
            // end; past the buffer's end Chromium carries the last slope on
            let s2 = match ch.get(i1 + 1) {
                Some(&s) => s,
                None if i1 > 0 => 2.0 * s1 - ch[i1 - 1],
                None => s1,
            };
            *o = (1.0 - frac) * s1 + frac * s2;
        }
        if n == 1 {
            out[1] = out[0];
        }
        Some(out)
    }
}

/// A voice's own filter, moved by its own envelope (the sampler's filter envelope).
#[derive(Clone, Debug)]
pub struct FilterEnv {
    pub kind: &'static str,
    /// Cutoff in Hz with the envelope at 0.
    pub cutoff: f64,
    pub res: f64,
    /// How far the envelope at full moves the cutoff, in octaves (may be negative).
    pub amount: f64,
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
    /// When the note starts, and when its release begins (seconds).
    pub start: f64,
    pub released_at: Option<f64>,
    filt: [WebBiquad; 2],
    made: f64,
}

impl FilterEnv {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: &'static str,
        cutoff: f64,
        res: f64,
        amount: f64,
        attack: f64,
        decay: f64,
        sustain: f64,
        release: f64,
    ) -> Self {
        Self {
            kind,
            cutoff,
            res,
            amount,
            attack: attack.max(0.001),
            decay: decay.max(0.001),
            sustain: sustain.clamp(0.0, 1.0),
            release: release.max(0.001),
            start: 0.0,
            released_at: None,
            filt: [WebBiquad::default(); 2],
            made: f64::NAN,
        }
    }

    /// The envelope (0 to 1) `t` seconds on the engine clock.
    pub fn level(&self, t: f64) -> f64 {
        let held = |t: f64| {
            let x = t - self.start;
            if x <= 0.0 {
                0.0
            } else if x < self.attack {
                x / self.attack
            } else {
                let k = (-(x - self.attack) * 4.6 / self.decay).exp();
                self.sustain + (1.0 - self.sustain) * k
            }
        };
        match self.released_at {
            Some(r) if t > r => held(r) * (-(t - r) * 4.6 / self.release).exp(),
            _ => held(t),
        }
    }

    fn run(&mut self, t: f64, sr: f64, l: f32, r: f32) -> (f32, f32) {
        let c = (self.cutoff * 2f64.powf(self.amount * self.level(t))).clamp(20.0, sr * 0.45);
        // the coefficients move in steps of a hundredth of an octave
        if !(self.made.is_finite() && (c / self.made).log2().abs() < 0.01) {
            self.made = c;
            for f in &mut self.filt {
                f.set(self.kind, c, self.res, 0.0, sr);
            }
        }
        (
            self.filt[0].run(f64::from(l)) as f32,
            self.filt[1].run(f64::from(r)) as f32,
        )
    }
}

fn filter_kind(k: &str) -> &'static str {
    match k {
        "highpass" => "highpass",
        "bandpass" => "bandpass",
        _ => "lowpass",
    }
}

pub struct Voice {
    src: SourcePlayback,
    /// Its own filter, when the filter envelope moves the cutoff.
    fenv: Option<Box<FilterEnv>>,
    gain: Param,
    pan: f64,
    key: i32,
    /// The choke group of a pad voice.
    group: i32,
    /// Its release has begun (a note that was gated, or let go).
    released: bool,
    started: bool,
    done: bool,
}

/// How a one-shot voice starts (the web version's `startVoice`).
pub struct VoiceSpec {
    pub key: i32,
    pub time: f64,
    pub rate: f64,
    pub gain: f64,
    pub pan: f64,
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
    pub duration: Option<f64>,
    pub start: f64,
    pub end: f64,
    pub looped: bool,
    pub filter: Option<FilterEnv>,
}

fn make_voice(buf: &Arc<Audio>, sr: f64, v: VoiceSpec) -> Voice {
    let VoiceSpec {
        key,
        time: t,
        rate,
        gain,
        pan,
        attack,
        decay,
        sustain,
        release,
        duration,
        start,
        end,
        looped,
        filter,
    } = v;
    let dur = buf.frames() as f64 / buf.sample_rate;
    let start_sec = 0f64.max((dur - 0.001).min(start * dur));
    let end_sec = (start_sec + 0.001).max(end * dur);
    let mut g = Param::new(0.0);
    g.set_value_at_time(0.0, t);
    g.linear_ramp_to_value_at_time(gain, t + attack.max(0.001));
    g.set_target_at_time(
        gain * sustain,
        t + attack.max(0.001),
        decay.max(0.001) / 3.0,
    );
    let natural = (end_sec - start_sec) / rate;
    let mut stop = None;
    if let Some(d) = duration {
        let rel = t + d.max(0.005);
        g.cancel_scheduled_values(rel);
        g.set_target_at_time(0.0, rel, release.max(0.002) / 4.0);
        stop = Some(rel + release.max(0.01) * 1.5 + 0.05);
    } else if !looped {
        stop = Some(t + natural + 0.01);
    }
    if !looped && end < 1.0 {
        stop = Some(t + natural);
    }
    let bsr = buf.sample_rate;
    Voice {
        src: SourcePlayback {
            buf: buf.clone(),
            start: t,
            offset: round(start_sec * bsr),
            rate: rate * bsr / sr,
            stop,
            loop_points: looped.then_some((start_sec * bsr, end_sec * bsr)),
        },
        gain: g,
        fenv: filter.map(|mut f| {
            f.start = t;
            f.released_at = duration.map(|d| t + d.max(0.005));
            Box::new(f)
        }),
        pan,
        key,
        group: 0,
        released: duration.is_some(),
        started: false,
        done: false,
    }
}

/// Add the sounding voices to `l` and `r` for frames `f0..f0 + l.len()`.
fn mix_voices(
    voices: &mut Vec<Voice>,
    f0: usize,
    l: &mut [f32],
    r: &mut [f32],
    sr: f64,
    gain_buf: &mut Vec<f32>,
) {
    let n = l.len();
    gain_buf.resize(n, 0.0);
    for v in voices.iter_mut() {
        if v.done {
            continue;
        }
        let first = v.src.first_frame(sr);
        if f0 + n <= first {
            continue;
        }
        let from = f0.max(first);
        let count = f0 + n - from;
        v.started = true;
        let gains = &mut gain_buf[..count];
        v.gain.fill(from, gains, sr);
        let stereo = v.src.buf.channels.len() > 1;
        for k in 0..count {
            let frame = from + k;
            let Some(s) = v.src.read(frame, sr) else {
                v.done = true;
                break;
            };
            let g = gains[k];
            let (mut a, mut b) = (s[0] * g, s[1] * g);
            if let Some(f) = &mut v.fenv {
                (a, b) = f.run(frame as f64 / sr, sr, a, b);
            }
            let (pl, pr) = if stereo {
                let (mut x, mut y) = ([a], [b]);
                stereo_pan(&mut x, &mut y, v.pan);
                (x[0], y[0])
            } else {
                mono_pan(a, v.pan)
            };
            l[from - f0 + k] += pl;
            r[from - f0 + k] += pr;
        }
    }
    voices.retain(|v| !v.done);
}

pub struct Sampler {
    sr: f64,
    state: Value,
    sample: Option<Arc<Audio>>,
    voices: Vec<Voice>,
    filter: [WebBiquad; 2],
    filter_kind: String,
    /// Cutoff and Q (each read once per channel), and the boost gain.
    g: Glides,
    made: [(f32, f32); 2],
    /// Automation of what is read when a note starts.
    timeline: BTreeMap<String, Timeline>,
    gain_buf: Vec<f32>,
}

fn reversed(a: &Audio) -> Audio {
    Audio {
        channels: a
            .channels
            .iter()
            .map(|c| c.iter().rev().copied().collect())
            .collect(),
        sample_rate: a.sample_rate,
    }
}

impl Sampler {
    pub fn new(state: &Value, sample: Option<Arc<Audio>>, sr: f64) -> Self {
        let sample = if state.get("reverse").and_then(Value::as_bool) == Some(true) {
            sample.map(|a| Arc::new(reversed(&a)))
        } else {
            sample
        };
        let filter_kind = state
            .get("filterType")
            .and_then(Value::as_str)
            .unwrap_or("lowpass")
            .to_string();
        let cutoff = num(state.get("cutoff"), 20000.0);
        let res = num(state.get("res"), 0.7);
        let boost = 10f64.powf(num(state.get("boost"), 0.0) / 20.0);
        let g = Glides::with_channels(&[cutoff, res, boost], &[2, 2, 1]);
        Self {
            sr,
            state: state.clone(),
            sample,
            voices: Vec::new(),
            filter: [WebBiquad::default(); 2],
            filter_kind,
            g,
            made: [(f32::NAN, 0.0); 2],
            timeline: BTreeMap::new(),
            gain_buf: Vec::new(),
        }
    }

    /// Automate a parameter through `points` (time, value). Returns whether the
    /// sampler has it.
    pub fn automate(&mut self, path: &str, points: &[(f64, f64)], first: bool) -> bool {
        match path {
            "cutoff" => self.g.param_mut(0).ramp(points, first),
            "res" => self.g.param_mut(1).ramp(points, first),
            "boost" => {
                let pts: Vec<(f64, f64)> = points
                    .iter()
                    .map(|&(t, db)| (t, 10f64.powf(db / 20.0)))
                    .collect();
                self.g.param_mut(2).ramp(&pts, first);
            }
            "tune" | "fine" | "velToVol" | "attack" | "decay" | "sustain" | "release" | "start"
            | "end" => {
                self.timeline
                    .entry(path.to_string())
                    .or_default()
                    .set(points, first);
            }
            _ => return false,
        }
        true
    }

    /// Swap the voices' remaining envelope for a quick fade at `time`.
    fn cut_voices(&mut self, time: f64) {
        for v in &mut self.voices {
            v.gain.cancel_scheduled_values(time);
            v.gain.set_target_at_time(0.0, time, 0.003);
            v.src.stop = Some(time + 0.03);
        }
    }

    /// Play a note: `duration` gates it, `None` lets the sample ring out; `rel_scale`
    /// stretches the release (the note's release).
    #[allow(clippy::too_many_arguments)]
    pub fn play_note(
        &mut self,
        key: i32,
        vel: f64,
        time: f64,
        duration: Option<f64>,
        fine: f64,
        pan: f64,
        rel_scale: f64,
    ) {
        let Some(buf) = self.sample.clone() else {
            return;
        };
        let state = self.state.clone();
        let s = &state;
        // automation of a parameter is read as the note starts
        let timeline = self.timeline.clone();
        let n = move |k: &str, d: f64| {
            let base = num(s.get(k), d);
            timeline.get(k).map_or(base, |t| t.get(time, base))
        };
        let looped = s.get("loop").is_some_and(|v| v.as_bool() == Some(true));
        if s.get("cut").is_some_and(truthy) {
            self.cut_voices(time);
        }
        let semis =
            f64::from(key) - n("root", 60.0) + n("tune", 0.0) + (n("fine", 0.0) + fine) / 100.0;
        let vel_to_vol = n("velToVol", 0.7);
        let gain = 1.0 - vel_to_vol + vel_to_vol * vel;
        // the node's playbackRate is a float
        let rate = f64::from(2f64.powf(semis / 12.0) as f32);
        let (attack, decay, sustain, release) = (
            n("attack", 0.001),
            n("decay", 0.3),
            n("sustain", 1.0),
            n("release", 0.08) * rel_scale,
        );
        let (start, end) = (n("start", 0.0), n("end", 1.0));
        let amount = n("fltEnvAmt", 0.0);
        let filter = (amount != 0.0).then(|| {
            FilterEnv::new(
                filter_kind(&self.filter_kind),
                n("cutoff", 20000.0),
                n("res", 0.7),
                amount,
                n("fltAttack", 0.01),
                n("fltDecay", 0.4),
                n("fltSustain", 0.0),
                n("fltRelease", 0.3) * rel_scale,
            )
        });
        self.voices.push(make_voice(
            &buf,
            self.sr,
            VoiceSpec {
                key,
                time,
                rate,
                gain,
                pan,
                attack,
                decay,
                sustain,
                release,
                duration,
                start,
                end,
                looped,
                filter,
            },
        ));
    }

    /// Let go of the key: the voices playing it release.
    pub fn note_off(&mut self, key: i32, time: f64) {
        let rel = num(self.state.get("release"), 0.08);
        for v in &mut self.voices {
            if v.key == key && !v.released {
                v.released = true;
                if let Some(f) = &mut v.fenv {
                    f.released_at = Some(time);
                }
                v.gain.cancel_scheduled_values(time);
                v.gain.set_target_at_time(0.0, time, rel.max(0.002) / 4.0);
                v.src.stop = Some(time + rel * 1.5 + 0.05);
            }
        }
    }

    /// Fade out everything that sounds.
    pub fn stop(&mut self, time: f64) {
        self.cut_voices(time);
    }

    /// Whether the sampler loops its sample.
    pub fn state_loop(&self) -> Option<bool> {
        self.state.get("loop").and_then(Value::as_bool)
    }

    pub fn voice_keys(&self) -> impl Iterator<Item = i32> + '_ {
        self.voices.iter().map(|v| v.key)
    }

    /// Render frames `f0..f0 + l.len()` (overwriting `l` and `r`).
    pub fn render(&mut self, f0: usize, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let sr = self.sr;
        l.fill(0.0);
        r.fill(0.0);
        mix_voices(&mut self.voices, f0, l, r, sr, &mut self.gain_buf);
        // the shared filter and boost (with the filter envelope on, each voice has its
        // own filter instead)
        self.g.fill(f0, n, sr);
        if num(self.state.get("fltEnvAmt"), 0.0) != 0.0 {
            for i in 0..n {
                let boost = self.g.buf(2)[i];
                l[i] *= boost;
                r[i] *= boost;
            }
            return;
        }
        for i in 0..n {
            let boost = self.g.buf(2)[i];
            for ch in 0..2 {
                let now = (self.g.chan(0, ch)[i], self.g.chan(1, ch)[i]);
                if now != self.made[ch] {
                    self.made[ch] = now;
                    self.filter[ch].set(
                        &self.filter_kind,
                        f64::from(now.0),
                        f64::from(now.1),
                        0.0,
                        sr,
                    );
                }
                let x = if ch == 0 { &mut l[i] } else { &mut r[i] };
                *x = (self.filter[ch].run(f64::from(*x)) as f32) * boost;
            }
        }
    }
}

/// One pad of the pad instrument.
struct Pad {
    key: i32,
    level: f64,
    pan: f64,
    tune: f64,
    cut: i32,
    sample: Option<Arc<Audio>>,
}

/// The 16-pad drum sampler with choke groups (LumaPad).
pub struct PadBank {
    sr: f64,
    pads: Vec<Pad>,
    voices: Vec<Voice>,
    timeline: BTreeMap<String, Timeline>,
    gain_buf: Vec<f32>,
}

impl PadBank {
    /// `samples` holds each pad's sample, in the order of the state's pads.
    pub fn new(state: &Value, samples: Vec<Option<Arc<Audio>>>, sr: f64) -> Self {
        let pads = state
            .get("pads")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, p)| Pad {
                        key: num(p.get("key"), 0.0) as i32,
                        level: num(p.get("level"), 0.85),
                        pan: num(p.get("pan"), 0.0),
                        tune: num(p.get("tune"), 0.0),
                        cut: num(p.get("cut"), 0.0) as i32,
                        sample: samples.get(i).cloned().flatten(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            sr,
            pads,
            voices: Vec::new(),
            timeline: BTreeMap::new(),
            gain_buf: Vec::new(),
        }
    }

    /// Automate `pads.N.level`, `.pan` or `.tune`.
    pub fn automate(&mut self, path: &str, points: &[(f64, f64)], first: bool) -> bool {
        let ok = path.strip_prefix("pads.").is_some_and(|rest| {
            rest.split_once('.').is_some_and(|(i, what)| {
                i.parse::<usize>().is_ok() && matches!(what, "level" | "pan" | "tune")
            })
        });
        if ok {
            self.timeline
                .entry(path.to_string())
                .or_default()
                .set(points, first);
        }
        ok
    }

    /// The pad with this key plays.
    pub fn play_note(&mut self, key: i32, vel: f64, time: f64, fine: f64) {
        let Some(i) = self.pads.iter().position(|p| p.key == key) else {
            return;
        };
        let pad = &self.pads[i];
        let Some(buf) = pad.sample.clone() else {
            return;
        };
        let at = |k: &str, d: f64| {
            self.timeline
                .get(&format!("pads.{i}.{k}"))
                .map_or(d, |t| t.get(time, d))
        };
        let (tune, level, pan) = (
            at("tune", pad.tune),
            at("level", pad.level),
            at("pan", pad.pan),
        );
        let cut = pad.cut;
        if cut != 0 {
            for v in &mut self.voices {
                if v.group == cut {
                    v.gain.cancel_scheduled_values(time);
                    v.gain.set_target_at_time(0.0, time, 0.004);
                    v.src.stop = Some(time + 0.04);
                }
            }
        }
        let semis = tune + fine / 100.0;
        let mut v = make_voice(
            &buf,
            self.sr,
            VoiceSpec {
                key,
                time,
                rate: f64::from(2f64.powf(semis / 12.0) as f32),
                gain: level * (0.3 + 0.7 * vel),
                pan,
                attack: 0.001,
                decay: 1.0,
                sustain: 1.0,
                release: 0.05,
                duration: None,
                start: 0.0,
                end: 1.0,
                looped: false,
                filter: None,
            },
        );
        v.group = cut;
        self.voices.push(v);
    }

    /// Fade out everything that sounds.
    pub fn stop(&mut self, time: f64) {
        for v in &mut self.voices {
            v.gain.cancel_scheduled_values(time);
            v.gain.set_target_at_time(0.0, time, 0.02);
            v.src.stop = Some(time + 0.15);
        }
    }

    /// Render frames `f0..f0 + l.len()` (overwriting `l` and `r`).
    pub fn render(&mut self, f0: usize, l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
        mix_voices(&mut self.voices, f0, l, r, self.sr, &mut self.gain_buf);
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::Null => false,
        _ => true,
    }
}
