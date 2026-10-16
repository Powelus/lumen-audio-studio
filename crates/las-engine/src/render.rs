// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline render of a project to a stereo mix. Port of the web version's
//! `audio/render.ts` and the offline half of `audio/scheduler.ts`.

use crate::arp::Transport;
use crate::instrument::{stereo_pan, Instrument};
use crate::mixer::Mixer;
use crate::timeline::*;
use las_dsp::wavetable::TableLibrary;
use las_project::Project;
use std::collections::HashMap;

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
    volume: f32,
    pan: f64,
    track: usize,
    mute: bool,
    pitch: f64,
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
}

impl Scheduler<'_> {
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
        for n in notes {
            let Some(&ci) = self.index.get(n.channel) else {
                continue;
            };
            let ch = &mut self.channels[ci];
            if ch.mute {
                continue;
            }
            let fine = n.note.fine.unwrap_or(0.0) + ch.pitch * 100.0;
            ch.inst.play_note(
                f64::from(n.note.key),
                n.note.vel,
                time_of(n.tick),
                n.note.len * sec_per_tick,
                fine,
                n.note.pan,
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

/// Render the project's mix offline.
pub fn render_mix(project: &Project, opts: &RenderOptions) -> Result<Rendered, String> {
    let sr = opts.sample_rate;
    let mut tables = TableLibrary::new();
    let mut channels = Vec::new();
    let mut index = HashMap::new();
    let last_track = project.mixer.len().saturating_sub(1);
    for c in &project.channels {
        let inst = Instrument::new(&c.plugin_id, &c.state, sr, &mut tables)
            .map_err(|e| format!("{}: {}", c.name, e.0))?;
        index.insert(c.id.clone(), channels.len());
        channels.push(ChannelRt {
            inst,
            volume: if c.mute { 0.0 } else { c.volume as f32 },
            pan: c.pan,
            track: c.mixer_track.min(last_track),
            mute: c.mute,
            pitch: c.pitch,
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
    };
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
        ..
    } = sched;

    let mut mixer = Mixer::new(&project.mixer, project.master_volume, sr, BLOCK);
    let total = length + pad;
    let (mut out_l, mut out_r) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut bl, mut br) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let (mut ml, mut mr) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
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
            if ch.volume != 1.0 {
                for i in 0..n {
                    l[i] *= ch.volume;
                    r[i] *= ch.volume;
                }
            }
            stereo_pan(l, r, ch.pan);
            mixer.add_input(ch.track, l, r);
        }
        mixer.process(&mut ml[..n], &mut mr[..n]);
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
