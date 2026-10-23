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
use las_project::{AutomationClip, AutomationTarget, ClipKind, CtlKind, Project, Sample};
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

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
    /// A stretch of the song, from tick `from` to tick `to`.
    Range { from: f64, to: f64 },
}

/// How far a render on this thread has come: frames done of frames in all, for a
/// progress bar. Set it with [`watch_progress`] before rendering.
#[derive(Default, Debug)]
pub struct Progress {
    pub done: std::sync::atomic::AtomicU64,
    pub total: std::sync::atomic::AtomicU64,
}

impl Progress {
    /// From 0 to 1.
    pub fn fraction(&self) -> f64 {
        use std::sync::atomic::Ordering::Relaxed;
        let t = self.total.load(Relaxed);
        if t == 0 {
            0.0
        } else {
            (self.done.load(Relaxed) as f64 / t as f64).min(1.0)
        }
    }
}

thread_local! {
    static PROGRESS: std::cell::RefCell<Option<std::sync::Arc<Progress>>> =
        const { std::cell::RefCell::new(None) };
}

/// Report the progress of renders made on this thread to `p`.
pub fn watch_progress(p: Option<std::sync::Arc<Progress>>) {
    PROGRESS.with(|c| *c.borrow_mut() = p);
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

/// A stretch of scheduled ticks: where it starts in time and in ticks.
#[derive(Clone, Copy)]
struct Segment {
    time: f64,
    tick: f64,
    sec_per_tick: f64,
}

/// Where the song is at a point in time (tempo changes make anchors).
#[derive(Clone, Copy)]
struct Anchor {
    time: f64,
    beat: f64,
    bpm: f64,
}

/// The scheduler and the block renderer of a project: the offline render and the live
/// player both drive one of these.
pub(crate) struct Engine {
    pub(crate) project: Arc<Project>,
    pub(crate) sr: f64,
    song: bool,
    /// Index of the pattern played in pattern mode.
    pattern: Option<usize>,
    channels: Vec<ChannelRt>,
    index: HashMap<String, usize>,
    cursor_tick: f64,
    cursor_time: f64,
    abs_ticks: f64,
    last_bpm: f64,
    anchors: Vec<Anchor>,
    segments: std::collections::VecDeque<Segment>,
    /// Time, tick and seconds per tick of the start of the stretch at one tempo.
    time_anchor: Option<(f64, f64, f64)>,
    muted_tracks: Vec<bool>,
    mixer: Mixer,
    library: Library,
    /// The first segment of a run: automation jumps rather than ramps.
    fresh: bool,
    automation_first: HashSet<String>,
    /// The peak of each insert's output (falling slowly), for the mixer's meters.
    peaks: Vec<f32>,
    /// The same, left and right apart.
    peaks_lr: Vec<[f32; 2]>,
    /// Scratch for rendering a block.
    bl: Vec<f32>,
    br: Vec<f32>,
    vol: Vec<f32>,
    pan: Vec<f32>,
    mono: Vec<f32>,
}

/// The clip's value at `t`, its stored points ramped, between clip ticks `from` and `to`.
fn automation_breakpoints(clip: &AutomationClip, from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut out = vec![(from, automation_value_at(clip, from))];
    for (t, v) in crate::timeline::automation_curve(clip) {
        if t > from && t < to {
            out.push((t, v));
        }
    }
    if to > from {
        out.push((to, automation_value_at(clip, to)));
    }
    out
}

/// The points of a clip's curve that a channel pitch follows between clip ticks `from`
/// and `to`: the curve's own points (and the next one after, so it ramps on), never
/// points made up at the edges of the stretch, so that the line is the same however
/// the song is cut into stretches (live or rendered). `end` is the clip's end.
fn tune_breakpoints(
    clip: &AutomationClip,
    from: f64,
    to: f64,
    end: f64,
    first: bool,
) -> Vec<(f64, f64)> {
    let at = |t: f64| automation_value_at(clip, t).clamp(0.0, 1.0);
    let mut out = Vec::new();
    if first {
        out.push((from, at(from)));
    }
    let curve = crate::timeline::automation_curve(clip);
    for (t, v) in &curve {
        if *t > from && *t < to {
            out.push((*t, v.clamp(0.0, 1.0)));
        }
    }
    match curve.iter().find(|(t, _)| *t >= to) {
        Some(&(t, v)) if t <= end => out.push((t, v.clamp(0.0, 1.0))),
        _ if to < end => out.push((end, at(end))),
        _ => {}
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

impl Engine {
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
        // normalize brings the source's peak to 0 dB, then the clip gain applies
        let mut gain = plan.gain;
        if edit.and_then(|e| e.normalize) == Some(true) {
            let peak = f64::from(self.library.peak(&sample_id));
            if peak > 1e-6 {
                gain /= peak;
            }
        }
        let playback = ClipPlayback {
            buffer,
            rate: plan.rate,
            gain,
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
        // with tempo automation the tempo is that at the start of the cell the cursor is in
        let tick = if has_tempo_automation(&self.project, self.song) {
            (self.cursor_tick / TEMPO_CELL).floor() * TEMPO_CELL
        } else {
            self.cursor_tick
        };
        tempo_at(&self.project, tick, self.song)
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

    /// Schedule the notes and automation in ticks `[a, b)`. A tick `t` plays at engine
    /// time `anchor.0 + (t - anchor.1) * sec_per_tick`.
    fn schedule_range(&mut self, a: f64, b: f64, anchor: (f64, f64), sec_per_tick: f64) {
        let time_a = anchor.0 + (a - anchor.1) * sec_per_tick;
        let project = Arc::clone(&self.project);
        let p = &*project;
        let time_of = |tick: f64| anchor.0 + (tick - anchor.1) * sec_per_tick;
        let mut notes = Vec::new();
        let mut ctls = Vec::new();
        if self.song {
            notes = song_notes_in_range(p, a, b, &self.muted_tracks);
            ctls = song_controllers_in_range(p, a, b, &self.muted_tracks, self.fresh);
        } else if let Some(pat) = self.pattern.and_then(|i| p.patterns.get(i)) {
            pattern_notes_in_range(pat, 0.0, 0.0, f64::INFINITY, a, b, p, &mut notes);
            let chase = self.fresh;
            pattern_controllers_in_range(pat, 0.0, 0.0, f64::INFINITY, a, b, chase, &mut ctls);
        }
        // controllers go first, so a note on the same tick starts with them
        ctls.sort_by(|x, y| x.tick.total_cmp(&y.tick));
        for c in ctls {
            let Some(&ci) = self.index.get(c.channel) else {
                continue;
            };
            let cc = match c.kind {
                CtlKind::ModWheel => las_dsp::engine::Cc::Mw,
                CtlKind::PitchBend => las_dsp::engine::Cc::Pb,
                CtlKind::Aftertouch => las_dsp::engine::Cc::At,
            };
            self.channels[ci]
                .inst
                .controller(cc, c.value, time_of(c.tick));
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
                if let AutomationTarget::Channel { channel_id, param } = &target {
                    if param == "pitch" {
                        let (lo, hi) = target_range(&target).unwrap_or((-12.0, 12.0));
                        let pts: Vec<(f64, f64)> = tune_breakpoints(
                            clip,
                            s - c.start + c.offset,
                            e - c.start + c.offset,
                            c.length + c.offset,
                            first,
                        )
                        .into_iter()
                        .map(|(t, v)| (time_of(t + c.start - c.offset), lo + (hi - lo) * v))
                        .collect();
                        if let Some(&ci) = self.index.get(channel_id) {
                            let ch = &mut self.channels[ci];
                            if ch.inst.bends_live() {
                                ch.inst.automate_tune(&pts, first);
                            }
                        }
                    }
                }
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
                n.note.release.unwrap_or(0.5),
            );
        }
    }

    pub(crate) fn output_latency(&self) -> f64 {
        self.mixer.output_latency(0).min(MAX_PAD)
    }

    pub(crate) fn cursor_time(&self) -> f64 {
        self.cursor_time
    }

    pub(crate) fn cursor_tick(&self) -> f64 {
        self.cursor_tick
    }

    pub(crate) fn current_bpm(&self) -> f64 {
        self.bpm()
    }

    /// Stop every instrument at engine time `time`.
    pub(crate) fn stop_instruments(&mut self, time: f64) {
        for ch in &mut self.channels {
            ch.inst.stop(time);
        }
    }

    /// A key played live on channel `channel` (by index).
    pub(crate) fn note_on(&mut self, channel: usize, key: f64, vel: f64, time: f64) {
        if let Some(ch) = self.channels.get_mut(channel) {
            ch.inst.note_on(key, vel, time);
        }
    }

    pub(crate) fn weave_patch(
        &mut self,
        channel: usize,
        patch: Box<las_dsp::modular::Patch>,
    ) -> Box<las_dsp::modular::Patch> {
        match self.channels.get_mut(channel) {
            Some(ch) => ch.inst.weave_patch(patch),
            None => patch,
        }
    }

    pub(crate) fn channel_patch(
        &mut self,
        channel: usize,
        patch: Box<crate::instrument::ChannelPatch>,
    ) -> Box<crate::instrument::ChannelPatch> {
        match self.channels.get_mut(channel) {
            Some(ch) => ch.inst.channel_patch(patch),
            None => patch,
        }
    }

    /// Set one value of a channel's instrument now (see
    /// [`crate::instrument::live_param`]), `normalised` as an automation clip has it.
    pub(crate) fn plugin_param(&mut self, channel: usize, path: &str, normalised: f64, time: f64) {
        let length = self.mixer.length();
        if let Some(ch) = self.channels.get_mut(channel) {
            ch.inst.set_live(path, normalised, time, length);
        }
    }

    pub(crate) fn note_off(&mut self, channel: usize, key: f64, time: f64) {
        if let Some(ch) = self.channels.get_mut(channel) {
            ch.inst.note_off(key, time);
        }
    }

    pub(crate) fn controller(
        &mut self,
        channel: usize,
        cc: las_dsp::engine::Cc,
        value: f64,
        time: f64,
    ) {
        if let Some(ch) = self.channels.get_mut(channel) {
            ch.inst.controller(cc, value, time);
        }
    }

    /// Start a run: the ticks from `from` will play from engine time `at`.
    pub(crate) fn begin_run(&mut self, from: f64, at: f64) {
        self.cursor_tick = from;
        self.cursor_time = at;
        self.abs_ticks = from;
        self.last_bpm = 0.0;
        self.fresh = true;
        self.time_anchor = None;
        self.automation_first.clear();
        for ch in &mut self.channels {
            ch.inst.set_tune_origin(at);
        }
        let bpm0 = self.bpm();
        self.anchors.push(Anchor {
            time: at,
            beat: from / PPQ_F,
            bpm: bpm0,
        });
    }

    /// Schedule the next stretch of the run: up to tick `to`, at most `max_ticks` long
    /// and not across a tempo cell. Returns the engine time of its end.
    pub(crate) fn schedule_step(&mut self, to: f64, max_ticks: f64) -> f64 {
        let bpm = self.bpm();
        let sec_per_tick = 60.0 / (bpm * PPQ_F);
        let seg_end = if has_tempo_automation(&self.project, self.song) {
            // one tempo cell at a time, as the offline render goes, so both play alike
            to.min(((self.cursor_tick / TEMPO_CELL).floor() + 1.0) * TEMPO_CELL)
        } else {
            to.min(self.cursor_tick + max_ticks)
        };
        self.note_tempo(bpm);
        let t0 = self.cursor_time;
        self.segments.push_back(Segment {
            time: t0,
            tick: self.cursor_tick,
            sec_per_tick,
        });
        // pieces of one stretch at one tempo share their reckoning of time, so that how
        // the song is cut up makes no difference to when a note plays
        let anchor = match self.time_anchor {
            Some((_, _, spt)) if spt == sec_per_tick => self.time_anchor.unwrap(),
            _ => (t0, self.cursor_tick, sec_per_tick),
        };
        self.time_anchor = Some(anchor);
        self.schedule_range(
            self.cursor_tick,
            seg_end,
            (anchor.0, anchor.1),
            sec_per_tick,
        );
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
        self.cursor_time
    }

    /// Schedule ticks `[from, to)` starting at time `at`, without looping.
    /// Returns the end time.
    fn schedule_offline(&mut self, from: f64, to: f64, at: f64) -> f64 {
        self.begin_run(from, at);
        while self.cursor_tick < to - 1e-9 {
            self.schedule_step(to, f64::INFINITY);
        }
        self.cursor_time
    }

    /// The tick playing at engine time `time`, from the stretches scheduled so far.
    pub(crate) fn tick_at(&self, time: f64) -> f64 {
        let seg = self
            .segments
            .iter()
            .rev()
            .find(|s| s.time <= time)
            .or(self.segments.front());
        seg.map_or(0.0, |s| s.tick + (time - s.time).max(0.0) / s.sec_per_tick)
    }

    /// Forget what is older than engine time `time`.
    pub(crate) fn forget_before(&mut self, time: f64) {
        while self.segments.len() > 2 && self.segments[1].time <= time {
            self.segments.pop_front();
        }
        while self.anchors.len() > 2 && self.anchors[1].time <= time {
            self.anchors.remove(0);
        }
        for ch in &mut self.channels {
            ch.pitch_line.prune(time);
            ch.inst.prune_tune(time);
            ch.volume.compact();
            ch.pan.compact();
        }
        self.mixer.compact();
    }
}

/// Single-cycle frames of a sample for a wavetable: 2048 samples each, up to 256.
fn slice_frames(data: &[f32]) -> Vec<Vec<f32>> {
    const SIZE: usize = 2048;
    if data.len() < SIZE {
        return vec![data.to_vec()];
    }
    let count = (data.len() / SIZE).clamp(1, 256);
    (0..count)
        .map(|i| data[i * SIZE..(i + 1) * SIZE].to_vec())
        .collect()
}

/// Build the wavetable for every `user:` id in `value`.
fn register_user_tables(
    value: &serde_json::Value,
    library: &mut Library,
    tables: &mut TableLibrary,
) {
    match value {
        serde_json::Value::String(id) => {
            if let Some(sample) = id.strip_prefix("user:") {
                if let Some(audio) = library.get(sample) {
                    if let Some(first) = audio.channels.first() {
                        let table = las_dsp::wavetable::table_from_frames(&slice_frames(first));
                        tables.register(id, table);
                    }
                }
            }
        }
        serde_json::Value::Array(a) => a
            .iter()
            .for_each(|v| register_user_tables(v, library, tables)),
        serde_json::Value::Object(o) => o
            .values()
            .for_each(|v| register_user_tables(v, library, tables)),
        _ => {}
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
    let mut all = render_taps(project, samples, opts, &[None])?;
    Ok(all.remove(0))
}

/// Render one output of the mixer offline: the master (`None`) or an insert after its
/// fader. Used to bounce a channel or an insert to audio.
pub fn render_tap(
    project: &Project,
    samples: &BTreeMap<String, Sample>,
    opts: &RenderOptions,
    insert: Option<usize>,
) -> Result<Rendered, String> {
    let mut all = render_taps(project, samples, opts, &[insert])?;
    Ok(all.remove(0))
}

/// One output of the mixer: the master, or an insert after its fader.
type Tap = Option<usize>;

/// Mixer inserts that receive signal: those a channel plays into and what they
/// feed (the master excluded).
pub fn used_inserts(project: &Project) -> Vec<usize> {
    let n = project.mixer.len();
    let mut used = vec![false; n];
    for c in &project.channels {
        used[c.mixer_track.min(n.saturating_sub(1))] = true;
    }
    let mut grew = true;
    while grew {
        grew = false;
        for (i, t) in project.mixer.iter().enumerate() {
            if !used[i] {
                continue;
            }
            let outs = t.output.into_iter().chain(
                t.sends
                    .iter()
                    .filter(|s| !s.sidechain_only)
                    .map(|s| s.target),
            );
            for o in outs.filter(|&o| o < n) {
                if !used[o] {
                    used[o] = true;
                    grew = true;
                }
            }
        }
    }
    (1..n).filter(|&i| used[i]).collect()
}

/// A stem: an insert (or the master) rendered on its own.
pub struct Stem {
    pub name: String,
    pub audio: Rendered,
}

/// Render the master and each used insert (after its fader) as stems, each trimmed by
/// its own plugin delay, as the web version does.
pub fn render_stems(
    project: &Project,
    samples: &BTreeMap<String, Sample>,
    opts: &RenderOptions,
) -> Result<Vec<Stem>, String> {
    let inserts = used_inserts(project);
    let mut taps: Vec<Tap> = vec![None];
    taps.extend(inserts.iter().map(|&i| Some(i)));
    let audio = render_taps(project, samples, opts, &taps)?;
    let mut names = vec!["Master".to_string()];
    names.extend(
        inserts
            .iter()
            .map(|&i| format!("{i:02} {}", project.mixer[i].name)),
    );
    Ok(names
        .into_iter()
        .zip(audio)
        .map(|(name, audio)| Stem { name, audio })
        .collect())
}

impl Engine {
    /// Build the engine for a project: instruments, mixer, samples.
    pub(crate) fn new(
        project: Arc<Project>,
        samples: &BTreeMap<String, Sample>,
        sr: f64,
        song: bool,
        pattern: Option<usize>,
    ) -> Result<Self, String> {
        let mut library = Library::new(sr, samples);
        let mut tables = TableLibrary::new();
        let mut channels = Vec::new();
        let mut index = HashMap::new();
        // wavetables made from samples (`user:<sample id>` in a patch)
        for c in &project.channels {
            register_user_tables(&c.state, &mut library, &mut tables);
        }
        let last_track = project.mixer.len().saturating_sub(1);
        for c in &project.channels {
            let sample = c.sample_id.as_deref().and_then(|id| library.get(id));
            // each pad of a pad instrument has its own sample
            let pad_samples: Vec<Option<Arc<crate::factory::Audio>>> = if c.plugin_id == "lumapad" {
                c.state
                    .get("pads")
                    .and_then(|p| p.as_array())
                    .map(|pads| {
                        pads.iter()
                            .map(|p| {
                                p.get("sampleId")
                                    .and_then(|v| v.as_str())
                                    .and_then(|id| library.get(id))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            // a channel whose settings cannot be read is silent; the rest still plays
            let mut inst =
                match Instrument::new(&c.plugin_id, &c.state, sr, &mut tables, sample, pad_samples)
                {
                    Ok(i) => i,
                    Err(e) => {
                        eprintln!("{}: {}", c.name, e.0);
                        Instrument::new(
                            "",
                            &serde_json::Value::Null,
                            sr,
                            &mut tables,
                            None,
                            Vec::new(),
                        )
                        .map_err(|e| format!("{}: {}", c.name, e.0))?
                    }
                };
            inst.set_smart_disable(smart_disable(&c.extra));
            inst.set_tune_base(c.pitch);
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
        let muted_tracks: Vec<bool> = project.playlist.silent_tracks();
        let mixer = Mixer::new(&project.mixer, project.master_volume, sr, BLOCK);
        let mut e = Engine {
            sr,
            song,
            pattern,
            channels,
            index,
            cursor_tick: 0.0,
            cursor_time: 0.0,
            abs_ticks: 0.0,
            last_bpm: 0.0,
            anchors: Vec::new(),
            segments: Default::default(),
            time_anchor: None,
            muted_tracks,
            mixer,
            library,
            fresh: true,
            automation_first: HashSet::new(),
            project,
            peaks: Vec::new(),
            peaks_lr: Vec::new(),
            bl: vec![0.0; BLOCK],
            br: vec![0.0; BLOCK],
            vol: vec![0.0; BLOCK],
            pan: vec![0.0; BLOCK],
            mono: Vec::with_capacity(BLOCK),
        };
        let bpm0 = tempo_at(&e.project, 0.0, song);
        for ch in &mut e.channels {
            ch.inst.set_bpm(bpm0);
        }
        Ok(e)
    }

    /// Render the block of frames starting at `frame` into `out_l` and `out_r`: the
    /// master's output. Insert outputs can be read afterwards from the mixer.
    pub(crate) fn render_block(&mut self, frame: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        let sr = self.sr;
        let t0 = frame as f64 / sr;
        let a = self
            .anchors
            .iter()
            .rev()
            .find(|a| a.time <= t0)
            .or(self.anchors.first())
            .copied()
            .unwrap_or(Anchor {
                time: 0.0,
                beat: 0.0,
                bpm: self.project.bpm,
            });
        let beat = a.beat + (t0 - a.time) * a.bpm / 60.0;
        self.mixer.set_position(beat, a.bpm);
        self.mixer.begin(n);
        for v in [&mut self.bl, &mut self.br, &mut self.vol, &mut self.pan] {
            v.resize(n, 0.0);
        }
        for ch in &mut self.channels {
            if ch.inst.is_silent() {
                continue;
            }
            ch.inst.set_position(beat, a.bpm);
            let (l, r) = (&mut self.bl[..n], &mut self.br[..n]);
            ch.inst.render(t0, l, r);
            if ch.inst.is_asleep() {
                continue;
            }
            let mono = ch.inst.is_mono();
            let (vol, pan) = (&mut self.vol, &mut self.pan);
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
                let src = &mut self.mono;
                src.clear();
                src.extend_from_slice(l);
                mono_pan_a(src, pan, l, r);
            } else if pan.iter().any(|&p| p != 0.0) {
                stereo_pan_a(l, r, pan);
            }
            self.mixer.add_input(ch.track, l, r);
        }
        self.mixer.process(frame, out_l, out_r);
        let inserts = self.mixer.len();
        self.peaks.resize(inserts, 0.0);
        self.peaks_lr.resize(inserts, [0.0; 2]);
        for i in 0..inserts {
            let (l, r) = self.mixer.insert_output(i);
            let peak = |b: &[f32]| b[..n].iter().fold(0f32, |m, s| m.max(s.abs()));
            let (pl, pr) = (peak(l), peak(r));
            let held = &mut self.peaks_lr[i];
            held[0] = pl.max(held[0] * 0.93);
            held[1] = pr.max(held[1] * 0.93);
            self.peaks[i] = pl.max(pr).max(self.peaks[i] * 0.93);
        }
    }

    /// The channels and the inserts that are running (not asleep, see smart disable).
    pub fn awake(&self) -> (Vec<usize>, Vec<usize>) {
        let ch = (0..self.channels.len())
            .filter(|&i| !self.channels[i].inst.is_silent() && !self.channels[i].inst.is_asleep())
            .collect();
        let ins = (0..self.mixer.len())
            .filter(|&i| !self.mixer.is_asleep(i))
            .collect();
        (ch, ins)
    }

    /// Add the mono signal of insert `i` for the block just rendered to `out`, as far
    /// as `out` has room (it never grows).
    pub(crate) fn tap_insert(&self, i: usize, out: &mut Vec<f32>) {
        if i >= self.mixer.len() {
            return;
        }
        let (l, r) = self.mixer.insert_output(i);
        for (a, b) in l.iter().zip(r) {
            if out.len() == out.capacity() {
                break;
            }
            out.push((a + b) * 0.5);
        }
    }

    /// The peak level of each insert, master first.
    pub fn insert_peaks(&self) -> &[f32] {
        &self.peaks
    }

    /// The peak level of each insert, left and right, master first.
    pub fn insert_peaks_lr(&self) -> &[[f32; 2]] {
        &self.peaks_lr
    }

    /// See [`Mixer::set_live`].
    pub fn set_live(
        &mut self,
        insert: usize,
        control: &crate::mixer::LiveControl,
        value: f64,
        time: f64,
    ) {
        self.mixer.set_live(insert, control, value, time);
    }
}

/// Whether a channel or an effect slot may sleep while it is quiet (FL's "smart
/// disable"): on unless the project says `"smartDisable": false`.
pub fn smart_disable(extra: &las_project::Extra) -> bool {
    extra.get("smartDisable") != Some(&serde_json::Value::Bool(false))
}

/// Render the project and return each tap's output, trimmed by its plugin delay.
fn render_taps(
    project: &Project,
    samples: &BTreeMap<String, Sample>,
    opts: &RenderOptions,
    taps: &[Tap],
) -> Result<Vec<Rendered>, String> {
    let sr = opts.sample_rate;
    let song = matches!(opts.mode, Mode::Song | Mode::Range { .. });
    let (from, to, loops, pattern) = match &opts.mode {
        Mode::Song => (0.0, song_length(project), 1, None),
        Mode::Range { from, to } => (from.max(0.0), to.max(from.max(0.0)), 1, None),
        Mode::Pattern { id, loops } => {
            let index = id
                .as_ref()
                .and_then(|i| project.patterns.iter().position(|p| &p.id == i))
                .or(if project.patterns.is_empty() {
                    None
                } else {
                    Some(0)
                });
            let pat = index.map(|i| &project.patterns[i]);
            let loops = (*loops).max(1);
            (
                0.0,
                pattern_length(pat, project.time_sig) * loops as f64,
                loops,
                index,
            )
        }
    };
    let seconds = seconds_between(project, from, to, song) + opts.tail;
    let length = (seconds * sr).ceil() as usize;
    let pad = (MAX_PAD * sr).ceil() as usize;

    let mut sched = Engine::new(Arc::new(project.clone()), samples, sr, song, pattern)?;
    sched.mixer.set_length(length + pad);
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

    let total = length + pad;
    let mut outs: Vec<(Vec<f32>, Vec<f32>)> = taps
        .iter()
        .map(|_| (Vec::with_capacity(total), Vec::with_capacity(total)))
        .collect();
    let (mut ml, mut mr) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let mut frame = 0usize;
    let progress = PROGRESS.with(|c| c.borrow().clone());
    if let Some(p) = &progress {
        p.total
            .store(total as u64, std::sync::atomic::Ordering::Relaxed);
    }
    while frame < total {
        if let Some(p) = &progress {
            p.done
                .store(frame as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let n = BLOCK.min(total - frame);
        sched.render_block(frame, &mut ml[..n], &mut mr[..n]);
        for (tap, (out_l, out_r)) in taps.iter().zip(outs.iter_mut()) {
            match tap {
                None => {
                    out_l.extend_from_slice(&ml[..n]);
                    out_r.extend_from_slice(&mr[..n]);
                }
                Some(i) => {
                    let (l, r) = sched.mixer.insert_output(*i);
                    out_l.extend_from_slice(&l[..n]);
                    out_r.extend_from_slice(&r[..n]);
                }
            }
        }
        frame += n;
    }
    if let Some(p) = &progress {
        p.done
            .store(total as u64, std::sync::atomic::Ordering::Relaxed);
    }
    // plugin delay moves the output later: trim it from the start
    let mixer = &sched.mixer;
    Ok(taps
        .iter()
        .zip(outs)
        .map(|(tap, (out_l, out_r))| {
            let latency = mixer.output_latency(tap.unwrap_or(0)).min(MAX_PAD);
            let off = js_round(latency * sr) as usize;
            let end = (off + length).min(out_l.len());
            Rendered {
                left: out_l[off.min(end)..end].to_vec(),
                right: out_r[off.min(end)..end].to_vec(),
                sample_rate: sr,
            }
        })
        .collect())
}
