// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruments and effects made outside the engine: hosted plugins. The engine does not
//! load plugin libraries itself; whoever does (the `las-host` crate) installs a
//! [`PluginHost`] that turns a channel's or an insert slot's state into a running
//! plugin. Without one, such a channel is silent and such a slot is skipped.

use serde_json::Value;
use std::sync::{Arc, RwLock};

pub use las_dsp::fx::registry::External;

/// The `pluginId` of a channel or an effect slot that is a hosted plugin.
pub const PLUGIN_ID: &str = "clap";

/// A note for a hosted instrument, `offset` frames into the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Note {
    On { offset: u32, key: u8, vel: f32 },
    Off { offset: u32, key: u8 },
}

impl Note {
    pub fn offset(&self) -> u32 {
        match *self {
            Self::On { offset, .. } | Self::Off { offset, .. } => offset,
        }
    }
}

/// A hosted instrument.
pub trait Instrument: Send {
    /// Play a block that starts at `t0` seconds: `notes` are in order of their offset;
    /// what the plugin makes replaces `l` and `r`.
    fn process(&mut self, t0: f64, notes: &[Note], l: &mut [f32], r: &mut [f32]);

    /// Timed automation of the parameter `id` (`points` are time in seconds and 0 to 1):
    /// whether the plugin has it.
    fn automate(&mut self, _id: u32, _points: &[(f64, f64)], _first: bool) -> bool {
        false
    }
}

/// The lines automation draws on a plugin's parameters, read once a block.
#[derive(Default)]
pub struct ParamLines {
    lines: Vec<Line>,
}

struct Line {
    id: u32,
    points: Vec<(f64, f64)>,
    sent: Option<f64>,
}

impl ParamLines {
    /// Add points to a parameter's line; `first` starts the line again.
    pub fn automate(&mut self, id: u32, points: &[(f64, f64)], first: bool) {
        let at = match self.lines.iter().position(|l| l.id == id) {
            Some(i) => i,
            None => {
                self.lines.push(Line {
                    id,
                    points: Vec::new(),
                    sent: None,
                });
                self.lines.len() - 1
            }
        };
        let line = &mut self.lines[at];
        if first {
            line.points.clear();
        }
        line.points.extend_from_slice(points);
        line.points.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    /// The parameters whose line has a new value at time `t`: `(id, 0 to 1)`.
    pub fn due(&mut self, t: f64) -> Vec<(u32, f64)> {
        let mut out = Vec::new();
        for l in &mut self.lines {
            let Some(v) = value_at(&l.points, t) else {
                continue;
            };
            if l.sent != Some(v) {
                l.sent = Some(v);
                out.push((l.id, v));
            }
            // points long past are no longer needed, except the last one before now
            let keep = l.points.partition_point(|p| p.0 <= t).saturating_sub(1);
            l.points.drain(..keep);
        }
        out
    }
}

fn value_at(points: &[(f64, f64)], t: f64) -> Option<f64> {
    let i = points.partition_point(|p| p.0 <= t);
    match (i.checked_sub(1).map(|k| points[k]), points.get(i)) {
        (None, None) => None,
        // before the first point the line has not begun
        (None, Some(_)) => None,
        (Some(a), None) => Some(a.1),
        (Some(a), Some(b)) => {
            let w = if b.0 > a.0 {
                (t - a.0) / (b.0 - a.0)
            } else {
                1.0
            };
            Some(a.1 + (b.1 - a.1) * w)
        }
    }
}

/// Makes hosted plugins from the state a project keeps for them.
pub trait PluginHost: Send + Sync {
    fn instrument(&self, state: &Value, sample_rate: f64) -> Option<Box<dyn Instrument>>;
    fn effect(&self, state: &Value, sample_rate: f64) -> Option<Box<dyn External>>;
}

static HOST: RwLock<Option<Arc<dyn PluginHost>>> = RwLock::new(None);

/// Install the host that loads plugins (or remove it).
pub fn set_plugin_host(host: Option<Arc<dyn PluginHost>>) {
    *HOST.write().unwrap_or_else(|e| e.into_inner()) = host;
}

pub(crate) fn plugin_host() -> Option<Arc<dyn PluginHost>> {
    HOST.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A hosted instrument with the notes it is yet to play.
pub(crate) struct Hosted {
    plugin: Box<dyn Instrument>,
    /// Notes by time (seconds on the engine clock): `(time, key, velocity)`, velocity 0
    /// for a note off.
    queue: Vec<(f64, u8, f32)>,
    held: Vec<u8>,
    sr: f64,
}

impl Hosted {
    pub fn new(plugin: Box<dyn Instrument>, sr: f64) -> Self {
        Self {
            plugin,
            queue: Vec::new(),
            held: Vec::new(),
            sr,
        }
    }

    pub fn on(&mut self, time: f64, key: f64, vel: f64) {
        let key = key.round().clamp(0.0, 127.0) as u8;
        self.queue
            .push((time, key, vel.clamp(0.0, 1.0).max(0.001) as f32));
    }

    pub fn off(&mut self, time: f64, key: f64) {
        let key = key.round().clamp(0.0, 127.0) as u8;
        self.queue.push((time, key, 0.0));
    }

    /// Forget notes yet to come and release the ones sounding, at `time`.
    /// Automate the parameter `id` of the plugin.
    pub fn automate(&mut self, id: u32, points: &[(f64, f64)], first: bool) -> bool {
        self.plugin.automate(id, points, first)
    }

    pub fn stop(&mut self, time: f64) {
        self.queue.retain(|&(t, _, v)| t < time && v == 0.0);
        let held = std::mem::take(&mut self.held);
        for key in held {
            self.queue.push((time, key, 0.0));
        }
    }

    pub fn render(&mut self, t0: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let end = t0 + n as f64 / self.sr;
        self.queue.sort_by(|a, b| a.0.total_cmp(&b.0));
        let due = self.queue.partition_point(|e| e.0 < end);
        let mut notes = Vec::with_capacity(due);
        for (time, key, vel) in self.queue.drain(..due) {
            let offset = (((time - t0) * self.sr).round().max(0.0) as usize).min(n - 1) as u32;
            if vel > 0.0 {
                self.held.push(key);
                notes.push(Note::On { offset, key, vel });
            } else {
                if let Some(i) = self.held.iter().position(|&k| k == key) {
                    self.held.swap_remove(i);
                }
                notes.push(Note::Off { offset, key });
            }
        }
        self.plugin.process(t0, &notes, l, r);
    }
}
