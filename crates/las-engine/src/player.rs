// SPDX-License-Identifier: GPL-3.0-or-later
//! The live player: the engine of a project that plays in real time. It schedules
//! the song a little ahead of the audio clock, renders block by block, and takes
//! keys played live. Everything it plays is the offline render's: it drives the same
//! scheduler and renderer.

use crate::render::{Engine, BLOCK};
use crate::timeline::{pattern_length, song_length, PPQ_F};
use las_project::{Project, Sample};
use std::collections::BTreeMap;
use std::sync::Arc;

/// How far ahead of the audio clock the song is scheduled, seconds.
const LOOKAHEAD: f64 = 0.2;
/// The longest stretch scheduled at once, seconds.
const STEP: f64 = 0.1;

/// What the player plays.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayMode {
    /// The playlist, once.
    Song,
    /// One pattern (by index), over and over.
    Pattern(usize),
}

/// A player is built for one project and one play: after [`Player::stop`] it keeps
/// sounding the tails, and a new player (built off the audio thread) starts the next
/// play. Building is the slow part: instruments, samples and reverb responses.
pub struct Player {
    engine: Engine,
    project: Arc<Project>,
    sr: f64,
    mode: PlayMode,
    /// The frame the next block renders.
    frame: usize,
    playing: bool,
    /// Where scheduling stops (ticks) and where a loop comes back to.
    end_tick: f64,
    scheduled_all: bool,
    /// Engine time when the last scheduled note ends, for the tail.
    out: [Vec<f32>; 2],
    /// Rendered frames not yet taken.
    ready: usize,
    used: bool,
    /// Click on every beat while playing.
    metronome: bool,
    /// The click sounding: frames left and the frequency.
    click: Option<(usize, f32)>,
    /// In song mode, a stretch (ticks) that plays over and over.
    song_loop: Option<(f64, f64)>,
}

impl Player {
    pub fn new(
        project: Arc<Project>,
        samples: &BTreeMap<String, Sample>,
        sample_rate: f64,
        mode: PlayMode,
    ) -> Result<Self, String> {
        let (song, pattern) = match mode {
            PlayMode::Song => (true, None),
            PlayMode::Pattern(i) => (false, Some(i.min(project.patterns.len().saturating_sub(1)))),
        };
        let engine = Engine::new(project.clone(), samples, sample_rate, song, pattern)?;
        let end_tick = match &mode {
            PlayMode::Song => song_length(&project),
            PlayMode::Pattern(_) => pattern_length(
                pattern.and_then(|i| project.patterns.get(i)),
                project.time_sig,
            ),
        };
        Ok(Self {
            engine,
            project,
            sr: sample_rate,
            mode,
            frame: 0,
            playing: false,
            end_tick,
            scheduled_all: false,
            out: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
            ready: 0,
            used: false,
            metronome: false,
            click: None,
            song_loop: None,
        })
    }

    /// In song mode, play the stretch from `a` to `b` (ticks) over and over. Set before
    /// [`Player::play`]; a stretch shorter than a sixteenth is ignored.
    pub fn set_song_loop(&mut self, range: Option<(f64, f64)>) {
        if self.mode != PlayMode::Song || self.used {
            return;
        }
        self.song_loop = range.filter(|(a, b)| *a >= 0.0 && b - a >= PPQ_F / 4.0);
        if let Some((_, b)) = self.song_loop {
            self.end_tick = b;
        }
    }

    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// Start playing from tick `from`. A player plays once; this says whether it
    /// could (a stopped player cannot start again).
    pub fn play(&mut self, from: f64) -> bool {
        if self.used {
            return false;
        }
        self.used = true;
        self.playing = true;
        self.scheduled_all = false;
        let at = self.frame as f64 / self.sr;
        self.engine.begin_run(from.clamp(0.0, self.end_tick), at);
        true
    }

    /// Stop: what has not happened yet is forgotten, what sounds is released.
    pub fn stop(&mut self) {
        if !self.playing {
            return;
        }
        self.playing = false;
        let now = self.now();
        self.engine.stop_instruments(now);
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The channels and the mixer inserts that are running; the rest sleep (smart
    /// disable) because they have nothing to play.
    pub fn awake(&self) -> (Vec<usize>, Vec<usize>) {
        self.engine.awake()
    }

    /// The peak level of each mixer insert (master first), falling slowly.
    pub fn insert_peaks(&self) -> &[f32] {
        self.engine.insert_peaks()
    }

    /// Switch the metronome on or off.
    pub fn set_metronome(&mut self, on: bool) {
        self.metronome = on;
        if !on {
            self.click = None;
        }
    }

    /// Mix the click of a beat that starts in the block of `BLOCK` frames at `frame`.
    fn add_click(&mut self, frame: usize) {
        if !self.metronome || !self.playing {
            return;
        }
        let t0 = frame as f64 / self.sr;
        let t1 = (frame + BLOCK) as f64 / self.sr;
        let (a, b) = (self.engine.tick_at(t0), self.engine.tick_at(t1));
        let beat = PPQ_F * 4.0 / f64::from(self.project.time_sig.1.max(1));
        let bar = beat * f64::from(self.project.time_sig.0.max(1));
        let (na, nb) = ((a / beat).ceil(), (b / beat).ceil());
        if nb > na || (a <= 0.0 && frame == 0) {
            // a beat line lies in this block: the first frame at or after it
            let tick = na * beat;
            let at = if b > a {
                (((tick - a) / (b - a)) * BLOCK as f64).clamp(0.0, (BLOCK - 1) as f64) as usize
            } else {
                0
            };
            let accent = (tick / bar).fract().abs() < 1e-9;
            let f = if accent { 1500.0 } else { 1000.0 };
            self.click = Some(((self.sr * 0.03) as usize, f));
            self.render_click(at);
        } else if self.click.is_some() {
            self.render_click(0);
        }
    }

    /// Add the sounding click to the output block from frame `from`.
    fn render_click(&mut self, from: usize) {
        let Some((left, f)) = self.click else {
            return;
        };
        let total = (self.sr * 0.03) as usize;
        let n = left.min(BLOCK - from);
        for i in 0..n {
            let k = (total - left + i) as f32;
            let env = (1.0 - k / total as f32).powi(2);
            let v = (k * f * std::f32::consts::TAU / self.sr as f32).sin() * env * 0.35;
            self.out[0][from + i] += v;
            self.out[1][from + i] += v;
        }
        self.click = (left > n).then_some((left - n, f));
    }

    /// Whether this player can still be started.
    pub fn can_play(&self) -> bool {
        !self.used
    }

    /// Engine time of the next block, seconds.
    fn now(&self) -> f64 {
        self.frame as f64 / self.sr
    }

    /// The tick playing now (what the playhead shows).
    pub fn position(&self) -> f64 {
        let t = (self.frame.saturating_sub(self.ready)) as f64 / self.sr;
        self.engine.tick_at(t).min(self.end_tick)
    }

    /// A key played live on a channel (by index), from now until [`Player::note_off`].
    pub fn note_on(&mut self, channel: usize, key: f64, vel: f64) {
        let t = self.now();
        self.engine.note_on(channel, key, vel, t);
    }

    pub fn note_off(&mut self, channel: usize, key: f64) {
        let t = self.now();
        self.engine.note_off(channel, key, t);
    }

    /// A performance controller (mod wheel, pitch bend, aftertouch) moved on a channel.
    pub fn controller(&mut self, channel: usize, cc: las_dsp::engine::Cc, value: f64) {
        let t = self.now();
        self.engine.controller(channel, cc, value, t);
    }

    /// Schedule the song up to the lookahead.
    fn schedule(&mut self) {
        if !self.playing || self.scheduled_all {
            return;
        }
        let horizon = self.now() + LOOKAHEAD;
        while self.engine.cursor_time() < horizon {
            let bpm = self.engine.current_bpm();
            let max_ticks = (STEP * bpm * PPQ_F / 60.0).max(1.0);
            self.engine.schedule_step(self.end_tick, max_ticks);
            if self.engine.cursor_tick() >= self.end_tick - 1e-9 {
                match self.mode {
                    PlayMode::Pattern(_) => {
                        // back to the start, in time
                        let at = self.engine.cursor_time();
                        self.engine.begin_run(0.0, at);
                    }
                    PlayMode::Song => {
                        if let Some((a, _)) = self.song_loop {
                            let at = self.engine.cursor_time();
                            self.engine.begin_run(a, at);
                            continue;
                        }
                        self.scheduled_all = true;
                        break;
                    }
                }
            }
        }
        let keep = self.now() - 1.0;
        if keep > 0.0 {
            self.engine.forget_before(keep);
        }
    }

    /// Render `l.len()` frames of the master's output (any number of frames).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let mut done = 0;
        while done < n {
            if self.ready == 0 {
                self.schedule();
                let [bl, br] = &mut self.out;
                self.engine.render_block(self.frame, bl, br);
                let at = self.frame;
                self.add_click(at);
                self.frame += BLOCK;
                self.ready = BLOCK;
            }
            let take = self.ready.min(n - done);
            let from = BLOCK - self.ready;
            l[done..done + take].copy_from_slice(&self.out[0][from..from + take]);
            r[done..done + take].copy_from_slice(&self.out[1][from..from + take]);
            self.ready -= take;
            done += take;
        }
    }

    /// Whether a song that has been scheduled to the end has finished sounding its
    /// notes (the tails of effects may go on a little longer).
    pub fn finished(&self) -> bool {
        self.playing && self.scheduled_all && self.now() >= self.engine.cursor_time()
    }

    /// How much later than the song the master's output sounds: the plugin delay of
    /// the mixer. A DAW with delay compensation starts the song that much early.
    pub fn output_latency(&self) -> f64 {
        self.engine.output_latency()
    }

    /// The engine time of the next block, in frames.
    pub fn frame(&self) -> usize {
        self.frame
    }
}
