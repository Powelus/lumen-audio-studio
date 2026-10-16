// SPDX-License-Identifier: GPL-3.0-or-later
//! The channel sampler: one `AudioBufferSourceNode` per note into a gain
//! envelope and a stereo panner, then a shared filter and boost. Port of the
//! web version's `instruments/sampler.ts`, with the Web Audio nodes behaving
//! as they do in Chromium (sub-sample start times, linear interpolation,
//! parameter timelines).

use crate::factory::Audio;
use crate::instrument::stereo_pan;
use crate::param::Param;
use las_dsp::fx::native::WebBiquad;
use serde_json::Value;
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

pub struct Voice {
    src: SourcePlayback,
    gain: Param,
    pan: f64,
    key: i32,
    started: bool,
    done: bool,
}

pub struct Sampler {
    sr: f64,
    state: Value,
    sample: Option<Arc<Audio>>,
    voices: Vec<Voice>,
    filter: [WebBiquad; 2],
    filter_kind: String,
    cutoff: f64,
    res: f64,
    boost: f32,
    tmp: [Vec<f32>; 2],
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
        let mut filter = [WebBiquad::default(); 2];
        for f in &mut filter {
            f.set(&filter_kind, cutoff, res, 0.0, sr);
        }
        let boost = 10f64.powf(num(state.get("boost"), 0.0) / 20.0) as f32;
        Self {
            sr,
            state: state.clone(),
            sample,
            voices: Vec::new(),
            filter,
            filter_kind,
            cutoff,
            res,
            boost,
            tmp: [Vec::new(), Vec::new()],
            gain_buf: Vec::new(),
        }
    }

    /// Swap the voices' remaining envelope for a quick fade at `time`.
    fn cut_voices(&mut self, time: f64) {
        for v in &mut self.voices {
            v.gain.cancel_scheduled_values(time);
            v.gain.set_target_at_time(0.0, time, 0.003);
            v.src.stop = Some(time + 0.03);
        }
    }

    /// Play a note: `duration` gates it, `None` lets the sample ring out.
    pub fn play_note(
        &mut self,
        key: i32,
        vel: f64,
        time: f64,
        duration: Option<f64>,
        fine: f64,
        pan: f64,
    ) {
        let Some(buf) = self.sample.clone() else {
            return;
        };
        let state = self.state.clone();
        let s = &state;
        let n = |k: &str, d: f64| num(s.get(k), d);
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
            n("release", 0.08),
        );
        let (start, end) = (n("start", 0.0), n("end", 1.0));
        let sr = self.sr;
        let dur = buf.frames() as f64 / buf.sample_rate;
        let start_sec = 0f64.max((dur - 0.001).min(start * dur));
        let end_sec = (start_sec + 0.001).max(end * dur);
        let t = time;
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
        self.voices.push(Voice {
            src: SourcePlayback {
                buf: buf.clone(),
                start: t,
                offset: round(start_sec * bsr),
                rate: rate * bsr / sr,
                stop,
                loop_points: looped.then_some((start_sec * bsr, end_sec * bsr)),
            },
            gain: g,
            pan,
            key,
            started: false,
            done: false,
        });
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
        for t in &mut self.tmp {
            t.resize(n, 0.0);
        }
        self.gain_buf.resize(n, 0.0);
        for v in &mut self.voices {
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
            let gains = &mut self.gain_buf[..count];
            v.gain.fill(from, gains, sr);
            let stereo = v.src.buf.channels.len() > 1;
            for k in 0..count {
                let frame = from + k;
                let Some(s) = v.src.read(frame, sr) else {
                    v.done = true;
                    break;
                };
                let g = gains[k];
                let (a, b) = (s[0] * g, s[1] * g);
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
        self.voices.retain(|v| !v.done);
        // the shared filter and boost
        for i in 0..n {
            l[i] = (self.filter[0].run(f64::from(l[i])) as f32) * self.boost;
            r[i] = (self.filter[1].run(f64::from(r[i])) as f32) * self.boost;
        }
        let _ = (&self.filter_kind, self.cutoff, self.res);
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
