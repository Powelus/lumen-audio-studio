// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline render of a project to a stereo mix. Port of the web version's
//! `audio/render.ts` and the offline half of `audio/scheduler.ts`.

use crate::arp::Transport;
use crate::audioclip::{clip_fades, clip_plan, ClipPlayback};
use crate::instrument::{mono_pan_a, stereo_pan_a, Instrument};
use crate::library::Library;
use crate::mixer::Mixer;
use crate::param::{Param, Timeline};
use crate::timeline::*;
use las_dsp::wavetable::TableLibrary;
use las_project::{AutomationClip, AutomationTarget, ClipKind, Project, Sample};
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet};

/// Frames per block (the web version's render quantum).
pub const BLOCK: usize = 128;
/// Longest plugin latency a render pads for, seconds.
const MAX_PAD: f64 = 1.0;

#[derive(Clone, Debug)]
pub enum Mode {
    /// The whole song.
    Song,
    /// One pattern (the first when `id` is `None`), played `loops` times.
    Pattern { id: Option<String>, loops: usize },
}

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub sample_rate: f64,
    /// Extra seconds after the end, for tails.
    pub tail: f64,
    pub mode: Mode,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            sample_rate: 44100.0,
            tail: 2.0,
            mode: Mode::Song,
        }
    }
}

pub struct Rendered {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub sample_rate: f64,
}

impl Rendered {
    pub fn seconds(&self) -> f64 {
        self.left.len() as f64 / self.sample_rate
    }
}

struct ChannelRt {
    inst: Instrument,
    volume: Param,
    pan: Param,
    track: usize,
    mute: bool,
    /// The channel's pitch offset, semitones, and its automation.
    pitch: f64,
    pitch_line: Timeline,
}

/// Where the song is at a point in time (tempo changes make anchors).
#[derive(Clone, Copy)]
struct Anchor {
    time: f64,
    beat: f64,
    bpm: f64,
}

struct Scheduler<'a> {
    project: &'a Project,
    song: bool,
    pattern: Option<&'a las_project::Pattern>,
    channels: Vec<ChannelRt>,
    index: HashMap<String, usize>,
    cursor_tick: f64,
    cursor_time: f64,
    abs_ticks: f64,
    last_bpm: f64,
    anchors: Vec<Anchor>,
    muted_tracks: Vec<bool>,
    mixer: Mixer,
    library: Library,
    /// The first segment of a run: automation jumps rather than ramps.
    fresh: bool,
    automation_first: HashSet<String>,
}

/// The clip's value at `t`, its stored points ramped, between clip ticks `from` and `to`.
fn automation_breakpoints(clip: &AutomationClip, from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut out = vec![(from, automation_value_at(clip, from))];
    for p in &clip.points {
        if p.t > from && p.t < to {
            out.push((p.t, p.v));
        }
    }
    if to > from {
        out.push((to, automation_value_at(clip, to)));
    }
    out
}

/// The range a normalised automation value maps onto, for the targets handled here.
fn target_range(target: &AutomationTarget) -> Option<(f64, f64)> {
    Some(match target {
        AutomationTarget::Mixer { param, .. } | AutomationTarget::Channel { param, .. } => {
            match param.as_str() {
                "volume" => (0.0, 1.25),
                "pan" => (-1.0, 1.0),
                "pitch" => (-12.0, 12.0),
                _ => return None,
            }
        }
        AutomationTarget::Master { param } if param == "volume" => (0.0, 1.25),
        _ => return None,
    })
}

/// A clip's timing in seconds.
struct ClipInfoSecs {
    clip_pos: f64,
    clip_len: f64,
    fade_in: f64,
    fade_out: f64,
    x_in: bool,
    x_out: bool,
}

impl Scheduler<'_> {
    /// Play an audio clip on its channel, as the web engine's `playClip`.
    fn play_clip(
        &mut self,
        channel_id: &str,
        time: f64,
        offset: f64,
        duration: f64,
        clip: &las_project::Clip,
        info: ClipInfoSecs,
    ) {
        let Some(&ci) = self.index.get(channel_id) else {
            return;
        };
        if self.channels[ci].mute {
            return;
        }
        let Some(sample_id) = self.project.channels[ci].sample_id.clone() else {
            return;
        };
        let plan = clip_plan(clip.audio.as_ref(), self.project.bpm);
        let Some(buffer) = self
            .library
            .processed(&sample_id, plan.reverse, plan.factor)
        else {
            return;
        };
        let edit = clip.audio.as_ref();
        let playback = ClipPlayback {
            buffer,
            rate: plan.rate,
            gain: plan.gain,
            clip_pos: info.clip_pos,
            clip_len: info.clip_len,
            fade_in: info.fade_in,
            fade_out: info.fade_out,
            fade_in_curve: edit.and_then(|e| e.fade_in_curve).unwrap_or(0.0),
            fade_out_curve: edit.and_then(|e| e.fade_out_curve).unwrap_or(0.0),
            x_in: info.x_in,
            x_out: info.x_out,
        };
        self.channels[ci]
            .inst
            .play_clip(time, offset, duration, Some(&playback));
    }

    /// Apply automation points (time, 0 to 1) to their target, as the web
    /// version's engine does.
    fn automate(&mut self, target: &AutomationTarget, points: &[(f64, f64)], first: bool) {
        if let AutomationTarget::Effect {
            track,
            slot_id,
            param,
        } = target
        {
            if !self.mixer.has_slot(*track, slot_id) {
                return;
            }
            let shift = self.mixer.path_latency(*track, slot_id);
            let pts: Vec<(f64, f64)> = points
                .iter()
                .map(|&(t, v)| (if shift > 0.0 { t + shift } else { t }, v))
                .collect();
            self.mixer
                .automate_effect(*track, slot_id, param, &pts, first);
            return;
        }
        if let AutomationTarget::Plugin { channel_id, param } = target {
            let length = self.mixer.length();
            if let Some(&ci) = self.index.get(channel_id) {
                self.channels[ci]
                    .inst
                    .automate(param, points, first, length);
            }
            return;
        }
        let Some((lo, hi)) = target_range(target) else {
            return;
        };
        // after latent effects the points move later, to stay with the audio
        let shift = match target {
            AutomationTarget::Mixer { track, .. } => self.mixer.output_latency(*track),
            AutomationTarget::Master { .. } => self.mixer.output_latency(0),
            _ => 0.0,
        };
        let pts: Vec<(f64, f64)> = points
            .iter()
            .map(|&(t, v)| {
                (
                    if shift > 0.0 { t + shift } else { t },
                    lo + (hi - lo) * v.clamp(0.0, 1.0),
                )
            })
            .collect();
        match target {
            AutomationTarget::Mixer { track, param } => {
                self.mixer.automate_track(*track, param, &pts, first);
            }
            AutomationTarget::Master { .. } => self.mixer.automate_master(&pts, first),
            AutomationTarget::Channel { channel_id, param } => {
                let Some(&ci) = self.index.get(channel_id) else {
                    return;
                };
                let ch = &mut self.channels[ci];
                match param.as_str() {
                    "volume" if !ch.mute => ch.volume.ramp(&pts, first),
                    "pan" => ch.pan.ramp(&pts, first),
                    "pitch" => ch.pitch_line.set(&pts, first),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn bpm(&self) -> f64 {
        tempo_at(self.project, self.cursor_tick, self.song)
    }

    fn transport(&self) -> Transport {
        Transport {
            bpm: self.bpm(),
            anchor_time: self.cursor_time,
            anchor_beat: self.abs_ticks / PPQ_F,
        }
    }

    fn note_tempo(&mut self, bpm: f64) {
        if (bpm - self.last_bpm).abs() < 1e-6 {
            return;
        }
        let first = self.last_bpm == 0.0;
        self.last_bpm = bpm;
        if first {
            return;
        }
        self.anchors.push(Anchor {
            time: self.cursor_time,
            beat: self.abs_ticks / PPQ_F,
            bpm,
        });
    }

    fn schedule_range(&mut self, a: f64, b: f64, time_a: f64, sec_per_tick: f64) {
        let p = self.project;
        let time_of = |tick: f64| time_a + (tick - a) * sec_per_tick;
        let mut notes = Vec::new();
        if self.song {
            notes = song_notes_in_range(p, a, b, &self.muted_tracks);
        } else if let Some(pat) = self.pattern {
            pattern_notes_in_range(pat, 0.0, 0.0, f64::INFINITY, a, b, p.swing, &mut notes);
        }
        if self.song {
            let fresh = self.fresh;
            let mut fades = None;
            for c in &p.playlist.clips {
                if c.muted == Some(true) || self.muted_tracks.get(c.track).copied().unwrap_or(false)
                {
                    continue;
                }
                let c_end = c.start + c.length;
                if c.kind == ClipKind::Audio {
                    let (into, at, off, len) = if c.start >= a && c.start < b {
                        (0.0, time_of(c.start), c.offset, c.length)
                    } else if fresh && c.start < a && c_end > a {
                        let into = a - c.start;
                        (into, time_a, c.offset + into, c.length - into)
                    } else {
                        continue;
                    };
                    let fades = fades
                        .get_or_insert_with(|| clip_fades(&p.playlist.clips, &self.muted_tracks));
                    let f = fades.get(&c.id).copied().unwrap_or_default();
                    self.play_clip(
                        &c.ref_id,
                        at,
                        off * sec_per_tick,
                        len * sec_per_tick,
                        c,
                        ClipInfoSecs {
                            clip_pos: into * sec_per_tick,
                            clip_len: c.length * sec_per_tick,
                            fade_in: f.fade_in * sec_per_tick,
                            fade_out: f.fade_out * sec_per_tick,
                            x_in: f.x_in,
                            x_out: f.x_out,
                        },
                    );
                    continue;
                }
                if c.kind != ClipKind::Automation {
                    continue;
                }
                if c.start >= b || c_end <= a {
                    continue;
                }
                let Some(clip) = p.automation.iter().find(|x| x.id == c.ref_id) else {
                    continue;
                };
                let Some(target) = clip.target() else {
                    continue;
                };
                if matches!(&target, AutomationTarget::Master { param } if param == "tempo") {
                    continue;
                }
                let s = a.max(c.start);
                let e = b.min(c_end);
                let pts: Vec<(f64, f64)> =
                    automation_breakpoints(clip, s - c.start + c.offset, e - c.start + c.offset)
                        .into_iter()
                        .map(|(t, v)| (time_of(t + c.start - c.offset), v))
                        .collect();
                let first = fresh || !self.automation_first.contains(&c.id) || s == c.start;
                self.automation_first.insert(c.id.clone());
                self.automate(&target, &pts, first);
            }
        }
        for n in notes {
            let Some(&ci) = self.index.get(n.channel) else {
                continue;
            };
            let ch = &mut self.channels[ci];
            if ch.mute {
                continue;
            }
            let time = time_of(n.tick);
            let pitch = ch.pitch_line.get(time, ch.pitch);
            let fine = n.note.fine.unwrap_or(0.0) + pitch * 100.0;
            ch.inst.play_note(
                f64::from(n.note.key),
                n.note.vel,
                time,
                n.note.len * sec_per_tick,
                fine,
                n.note.pan,
                n.note.step == Some(true),
            );
        }
    }

    /// Schedule ticks `[from, to)` starting at time `at`, without looping.
    /// Returns the end time.
    fn schedule_offline(&mut self, from: f64, to: f64, at: f64) -> f64 {
        self.cursor_tick = from;
        self.cursor_time = at;
        self.abs_ticks = from;
        self.last_bpm = 0.0;
        self.fresh = true;
        self.automation_first.clear();
        let bpm0 = self.bpm();
        self.anchors.push(Anchor {
            time: at,
            beat: from / PPQ_F,
            bpm: bpm0,
        });
        while self.cursor_tick < to - 1e-9 {
            let bpm = self.bpm();
            let sec_per_tick = 60.0 / (bpm * PPQ_F);
            let seg_end = if has_tempo_automation(self.project, self.song) {
                to.min(((self.cursor_tick / TEMPO_CELL).floor() + 1.0) * TEMPO_CELL)
            } else {
                to
            };
            self.note_tempo(bpm);
            let t0 = self.cursor_time;
            self.schedule_range(self.cursor_tick, seg_end, t0, sec_per_tick);
            self.abs_ticks += seg_end - self.cursor_tick;
            self.cursor_time += (seg_end - self.cursor_tick) * sec_per_tick;
            self.cursor_tick = seg_end;
            self.fresh = false;
            // time-based generators (the arpeggiator) follow the same segments
            let transport = self.transport();
            let end = self.cursor_time;
            for ch in &mut self.channels {
                ch.inst.tick(t0, end, transport);
            }
        }
        self.cursor_time
    }
}

/// Render the project's mix offline (the factory samples only).
pub fn render_mix(project: &Project, opts: &RenderOptions) -> Result<Rendered, String> {
    render_mix_with(project, &BTreeMap::new(), opts)
}

/// Render the project's mix offline, with the samples stored in its file.
pub fn render_mix_with(
    project: &Project,
    samples: &BTreeMap<String, Sample>,
    opts: &RenderOptions,
) -> Result<Rendered, String> {
    let sr = opts.sample_rate;
    let mut library = Library::new(sr, samples);
    let mut tables = TableLibrary::new();
    let mut channels = Vec::new();
    let mut index = HashMap::new();
    let last_track = project.mixer.len().saturating_sub(1);
    for c in &project.channels {
        let sample = c.sample_id.as_deref().and_then(|id| library.get(id));
        let inst = Instrument::new(&c.plugin_id, &c.state, sr, &mut tables, sample)
            .map_err(|e| format!("{}: {}", c.name, e.0))?;
        index.insert(c.id.clone(), channels.len());
        channels.push(ChannelRt {
            inst,
            volume: Param::new(if c.mute { 0.0 } else { c.volume }),
            pan: Param::new(c.pan),
            track: c.mixer_track.min(last_track),
            mute: c.mute,
            pitch: c.pitch,
            pitch_line: Timeline::default(),
        });
    }
    let song = matches!(opts.mode, Mode::Song);
    let (from, to, loops, pattern) = match &opts.mode {
        Mode::Song => (0.0, song_length(project), 1, None),
        Mode::Pattern { id, loops } => {
            let pat = id
                .as_ref()
                .and_then(|i| project.pattern(i))
                .or_else(|| project.patterns.first());
            let loops = (*loops).max(1);
            (
                0.0,
                pattern_length(pat, project.time_sig) * loops as f64,
                loops,
                pat,
            )
        }
    };
    let seconds = seconds_between(project, from, to, song) + opts.tail;
    let length = (seconds * sr).ceil() as usize;
    let pad = (MAX_PAD * sr).ceil() as usize;

    let muted_tracks: Vec<bool> = project.playlist.tracks.iter().map(|t| t.mute).collect();
    let mut sched = Scheduler {
        project,
        song,
        pattern,
        channels,
        index,
        cursor_tick: 0.0,
        cursor_time: 0.0,
        abs_ticks: 0.0,
        last_bpm: 0.0,
        anchors: Vec::new(),
        muted_tracks,
        mixer: Mixer::new(&project.mixer, project.master_volume, sr, BLOCK),
        library,
        fresh: true,
        automation_first: HashSet::new(),
    };
    sched.mixer.set_length(length + pad);
    let bpm0 = tempo_at(project, 0.0, song);
    for ch in &mut sched.channels {
        ch.inst.set_bpm(bpm0);
    }
    if song {
        sched.schedule_offline(from, to, 0.0);
    } else {
        // pattern mode renders the loops back to back
        let len = (to - from) / loops as f64;
        let mut t = 0.0;
        for _ in 0..loops {
            t = sched.schedule_offline(0.0, len, t);
        }
    }
    let Scheduler {
        mut channels,
        anchors,
        mut mixer,
        ..
    } = sched;

    let total = length + pad;
    let (mut out_l, mut out_r) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut bl, mut br) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let (mut ml, mut mr) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let mut gains = [vec![0f32; BLOCK], vec![0f32; BLOCK]];
    let mut frame = 0usize;
    while frame < total {
        let n = BLOCK.min(total - frame);
        let t0 = frame as f64 / sr;
        let a = anchors
            .iter()
            .rev()
            .find(|a| a.time <= t0)
            .or(anchors.first())
            .copied()
            .unwrap_or(Anchor {
                time: 0.0,
                beat: 0.0,
                bpm: project.bpm,
            });
        let beat = a.beat + (t0 - a.time) * a.bpm / 60.0;
        mixer.set_position(beat, a.bpm);
        mixer.begin(n);
        for ch in &mut channels {
            if ch.inst.is_silent() {
                continue;
            }
            ch.inst.set_position(beat, a.bpm);
            let (l, r) = (&mut bl[..n], &mut br[..n]);
            ch.inst.render(t0, l, r);
            let mono = ch.inst.is_mono();
            let [vol, pan] = &mut gains;
            vol.resize(n, 0.0);
            pan.resize(n, 0.0);
            ch.volume.fill(frame, vol, sr);
            ch.pan.fill(frame, pan, sr);
            if vol.iter().any(|&g| g != 1.0) {
                for i in 0..n {
                    l[i] *= vol[i];
                    r[i] *= vol[i];
                }
            }
            if mono {
                // a mono signal pans with the mono law, even in the middle
                let src = l.to_vec();
                mono_pan_a(&src, pan, l, r);
            } else if pan.iter().any(|&p| p != 0.0) {
                stereo_pan_a(l, r, pan);
            }
            mixer.add_input(ch.track, l, r);
        }
        mixer.process(frame, &mut ml[..n], &mut mr[..n]);
        out_l.extend_from_slice(&ml[..n]);
        out_r.extend_from_slice(&mr[..n]);
        frame += n;
    }
    // plugin delay moves the output later: trim it from the start
    let latency = mixer.output_latency(0).min(MAX_PAD);
    let off = js_round(latency * sr) as usize;
    let end = (off + length).min(out_l.len());
    Ok(Rendered {
        left: out_l[off.min(end)..end].to_vec(),
        right: out_r[off.min(end)..end].to_vec(),
        sample_rate: sr,
    })
}
