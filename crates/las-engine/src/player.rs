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
        })
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
