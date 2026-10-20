// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio clips: the maths shared by playback and the playlist (tempo follow,
//! pitch, reverse, gain, fades, crossfades) and the instrument that plays them.
//! Port of `core/audioClip.ts` and the clip half of `instruments/sampler.ts`.

use crate::factory::Audio;
use crate::param::Param;
use crate::sampler::SourcePlayback;
use las_project::{AudioClipEdit, Clip, ClipKind, StretchMode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

fn near1(v: f64) -> bool {
    (v - 1.0).abs() < 1e-6
}

/// How a clip plays at the project tempo.
#[derive(Clone, Copy, Debug)]
pub struct ClipPlan {
    /// Time stretch of the source, output length over input length (1 is none).
    pub factor: f64,
    /// Playback rate of the processed buffer.
    pub rate: f64,
    pub reverse: bool,
    /// Linear clip gain.
    pub gain: f64,
}

/// With tempo follow the clip lasts `src_bpm / bpm` times its source. "Stretch"
/// does that with a time stretch (pitch kept), "resample" with the playback rate
/// (pitch follows). A pitch shift is a stretch by the pitch ratio played back
/// faster by the same ratio, so the timing never changes.
pub fn clip_plan(edit: Option<&AudioClipEdit>, bpm: f64) -> ClipPlan {
    let pf = 2f64.powf(edit.and_then(|e| e.pitch).unwrap_or(0.0) / 12.0);
    let st = edit.and_then(|e| e.stretch.as_ref());
    let tr = st.map_or(1.0, |s| s.bpm / bpm);
    let mut factor = pf;
    let mut rate = pf;
    match st.map(|s| s.mode) {
        Some(StretchMode::Stretch) => factor = tr * pf,
        Some(StretchMode::Resample) => rate = pf / tr,
        None => {}
    }
    ClipPlan {
        factor: if near1(factor) { 1.0 } else { factor },
        rate: if near1(rate) { 1.0 } else { rate },
        reverse: edit.and_then(|e| e.reverse) == Some(true),
        gain: 10f64.powf(edit.and_then(|e| e.gain).unwrap_or(0.0) / 20.0),
    }
}

/// A rising fade shape: `curve` 0 is linear, above 0 starts slowly, below 0 quickly.
pub fn fade_shape(x: f64, curve: f64) -> f64 {
    x.clamp(0.0, 1.0).powf(2f64.powf(curve * 2.0))
}

/// Equal power crossfade shape (the rising half).
pub fn equal_power(x: f64) -> f64 {
    (x.clamp(0.0, 1.0) * std::f64::consts::PI / 2.0).sin()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClipFades {
    /// Ticks.
    pub fade_in: f64,
    pub fade_out: f64,
    /// A fade made by an overlap with a neighbour: equal power.
    pub x_in: bool,
    pub x_out: bool,
}

/// The fades of every unmuted audio clip, with automatic crossfades where two
/// clips overlap on one track (the earlier fades out across the overlap, the
/// later fades in). A longer fade of the user's wins.
pub fn clip_fades(clips: &[Clip], muted_tracks: &[bool]) -> HashMap<String, ClipFades> {
    let mut out: HashMap<String, ClipFades> = HashMap::new();
    let mut by_track: Vec<(usize, Vec<&Clip>)> = Vec::new();
    for c in clips {
        if c.kind != ClipKind::Audio
            || c.muted == Some(true)
            || muted_tracks.get(c.track).copied().unwrap_or(false)
        {
            continue;
        }
        match by_track.iter_mut().find(|(t, _)| *t == c.track) {
            Some((_, list)) => list.push(c),
            None => by_track.push((c.track, vec![c])),
        }
        out.insert(
            c.id.clone(),
            ClipFades {
                fade_in: c.audio.as_ref().and_then(|a| a.fade_in).unwrap_or(0.0),
                fade_out: c.audio.as_ref().and_then(|a| a.fade_out).unwrap_or(0.0),
                x_in: false,
                x_out: false,
            },
        );
    }
    for (_, list) in &mut by_track {
        list.sort_by(|a, b| a.start.total_cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
        for i in 0..list.len() {
            let a = list[i];
            let a_end = a.start + a.length;
            let mut j = i + 1;
            while j < list.len() && list[j].start < a_end {
                let b = list[j];
                j += 1;
                let b_end = b.start + b.length;
                // b sits entirely inside a: no crossfade (it plays on top)
                if b_end <= a_end {
                    continue;
                }
                let ov = a_end - b.start;
                if let Some(fa) = out.get_mut(&a.id) {
                    if ov > fa.fade_out {
                        fa.fade_out = ov;
                        fa.x_out = true;
                    }
                }
                if let Some(fb) = out.get_mut(&b.id) {
                    if ov > fb.fade_in {
                        fb.fade_in = ov;
                        fb.x_in = true;
                    }
                }
            }
        }
    }
    // fades never add up to more than the clip
    for c in clips {
        if let Some(f) = out.get_mut(&c.id) {
            let total = f.fade_in + f.fade_out;
            if total > c.length {
                let k = c.length / total;
                f.fade_in *= k;
                f.fade_out *= k;
            }
        }
    }
    out
}

/// How one clip plays, all times in seconds.
#[derive(Clone)]
pub struct ClipPlayback {
    /// The processed source (reversed and stretched) and its playback rate.
    pub buffer: Arc<Audio>,
    pub rate: f64,
    pub gain: f64,
    /// Where in the clip this playback starts, and the clip's length.
    pub clip_pos: f64,
    pub clip_len: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub fade_in_curve: f64,
    pub fade_out_curve: f64,
    pub x_in: bool,
    pub x_out: bool,
}

/// The gain of a clip `u` seconds into it, fades included.
#[allow(clippy::too_many_arguments)]
pub fn clip_gain_at(
    u: f64,
    len: f64,
    gain: f64,
    fade_in: f64,
    fade_out: f64,
    curve_in: f64,
    curve_out: f64,
    x_in: bool,
    x_out: bool,
) -> f64 {
    let mut g = gain;
    if fade_in > 0.0 && u < fade_in {
        g *= if x_in {
            equal_power(u / fade_in)
        } else {
            fade_shape(u / fade_in, curve_in)
        };
    }
    if fade_out > 0.0 && u > len - fade_out {
        g *= if x_out {
            equal_power((len - u) / fade_out)
        } else {
            fade_shape((len - u) / fade_out, curve_out)
        };
    }
    g
}

/// The gain envelope of one clip playback: clip gain, fades of any shape and
/// equal power crossfades, as short linear segments. `u0` is the clip time at
/// `t`; the playback lasts `dur` seconds. An edge without a fade gets a 3 ms
/// ramp so cuts never click.
pub fn clip_envelope(param: &mut Param, t: f64, dur: f64, u0: f64, c: &ClipPlayback) {
    const MIN: f64 = 0.003;
    let fi = c.fade_in.max(MIN);
    let fo = c.fade_out.max(MIN);
    let u1 = u0 + dur;
    // starting mid-clip (a seek, a loop) or ending early gets a short ramp too
    let cut_in = u0 > 1e-6 && u0 >= fi;
    let cut_out = u1 < c.clip_len - 1e-6;
    let env = |u: f64| -> f64 {
        let mut g = clip_gain_at(
            u,
            c.clip_len,
            c.gain,
            fi,
            fo,
            c.fade_in_curve,
            c.fade_out_curve,
            c.x_in,
            c.x_out,
        );
        if cut_in {
            g *= ((u - u0) / MIN).min(1.0);
        }
        if cut_out {
            g *= ((u1 - u) / MIN).min(1.0);
        }
        g
    };
    // breakpoints: dense inside fades, none in between
    let mut pts: Vec<f64> = vec![u1];
    if cut_in {
        pts.push(u1.min(u0 + MIN));
    }
    if cut_out {
        pts.push(u0.max(u1 - MIN));
    }
    let fill = |a: f64, b: f64, pts: &mut Vec<f64>| {
        let lo = a.max(u0);
        let hi = b.min(u1);
        if hi <= lo {
            return;
        }
        let n = (((hi - lo) / (b - a)) * 32.0).ceil().max(1.0) as usize;
        for i in 0..=n {
            pts.push(lo + ((hi - lo) * i as f64) / n as f64);
        }
    };
    fill(0.0, fi, &mut pts);
    fill(c.clip_len - fo, c.clip_len, &mut pts);
    param.set_value_at_time(env(u0), t);
    pts.sort_by(f64::total_cmp);
    pts.dedup();
    for u in pts {
        if u > u0 {
            param.linear_ramp_to_value_at_time(env(u), t + (u - u0));
        }
    }
}

struct ClipVoice {
    src: SourcePlayback,
    gain: Param,
    done: bool,
}

/// The audio clip instrument of a channel.
pub struct AudioClipper {
    sr: f64,
    sample: Option<Arc<Audio>>,
    voices: Vec<ClipVoice>,
    gain: Param,
    gain_buf: Vec<f32>,
    out_buf: Vec<f32>,
}

fn num(v: Option<&Value>, d: f64) -> f64 {
    v.and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(d)
}

impl AudioClipper {
    pub fn new(state: &Value, sample: Option<Arc<Audio>>, sr: f64) -> Self {
        Self {
            sr,
            sample,
            voices: Vec::new(),
            gain: Param::new(10f64.powf(num(state.get("gain"), 0.0) / 20.0)),
            gain_buf: Vec::new(),
            out_buf: Vec::new(),
        }
    }

    /// Automate the output gain through `points` (time, dB).
    pub fn automate_gain(&mut self, points: &[(f64, f64)], first: bool) {
        let pts: Vec<(f64, f64)> = points
            .iter()
            .map(|&(t, db)| (t, 10f64.powf(db / 20.0)))
            .collect();
        self.gain.ramp(&pts, first);
    }

    /// Whether the clips are mono (the channel then pans them with the mono law).
    pub fn is_mono(&self) -> bool {
        self.sample.as_ref().is_none_or(|s| s.channels.len() == 1)
    }

    /// Play a clip: `offset` is clip time into the source, `duration` how long.
    pub fn play_clip(
        &mut self,
        time: f64,
        offset: f64,
        duration: f64,
        clip: Option<&ClipPlayback>,
    ) {
        let (buf, rate) = match clip {
            Some(c) => (c.buffer.clone(), c.rate),
            None => match &self.sample {
                Some(s) => (s.clone(), 1.0),
                None => return,
            },
        };
        let rate = f64::from(rate as f32);
        let t = time;
        let off = offset * rate;
        let bsr = buf.sample_rate;
        let buf_dur = buf.frames() as f64 / bsr;
        if off >= buf_dur {
            return;
        }
        let dur = 0.005f64.max(duration.min((buf_dur - off) / rate));
        let mut g = Param::new(0.0);
        match clip {
            Some(c) => clip_envelope(&mut g, t, dur, c.clip_pos, c),
            None => {
                g.set_value_at_time(0.0, t);
                g.linear_ramp_to_value_at_time(1.0, t + 0.003);
                g.set_value_at_time(1.0, t + dur - 0.004);
                g.linear_ramp_to_value_at_time(0.0, t + dur);
            }
        }
        let sr = self.sr;
        self.voices.push(ClipVoice {
            src: SourcePlayback {
                buf,
                start: t,
                offset: (off * bsr + 0.5).floor(),
                rate: rate * bsr / sr,
                // the duration is in buffer seconds: `dur * rate`, at `rate` frames per frame
                stop: Some(t + dur),
                loop_points: None,
            },
            gain: g,
            done: false,
        });
    }

    /// A note previews the sample from its start.
    pub fn play_note(&mut self, time: f64) {
        if let Some(s) = self.sample.clone() {
            let d = s.frames() as f64 / s.sample_rate;
            self.play_clip(time, 0.0, d, None);
        }
    }

    pub fn render(&mut self, f0: usize, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let sr = self.sr;
        l.fill(0.0);
        r.fill(0.0);
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
            let gains = &mut self.gain_buf[..count];
            v.gain.fill(from, gains, sr);
            for (k, g) in gains.iter().enumerate() {
                let Some(s) = v.src.read(from + k, sr) else {
                    v.done = true;
                    break;
                };
                l[from - f0 + k] += s[0] * g;
                r[from - f0 + k] += s[1] * g;
            }
        }
        self.voices.retain(|v| !v.done);
        self.out_buf.resize(n, 0.0);
        self.gain.fill(f0, &mut self.out_buf, sr);
        if self.out_buf.iter().any(|&g| g != 1.0) {
            for i in 0..n {
                l[i] *= self.out_buf[i];
                r[i] *= self.out_buf[i];
            }
        }
    }
}
