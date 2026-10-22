// SPDX-License-Identifier: GPL-3.0-or-later
//! Tempo-synced arpeggiator. Notes are registered ahead of time by the
//! scheduler; `generate` turns the held chord into steps for a window of time.
//! Port of the web version's `instruments/arp.ts`.

use las_dsp::engine::Rng;
use serde_json::Value;

/// Tempo and a point on the beat grid, for turning time into beats.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub bpm: f64,
    pub anchor_time: f64,
    pub anchor_beat: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArpState {
    pub on: bool,
    pub mode: String,
    /// Step length in beats.
    pub div: f64,
    pub octaves: f64,
    pub gate: f64,
    /// How many steps each note of the pattern plays before the next (FL's repeat).
    pub repeat: f64,
    /// A chord every note becomes (see [`CHORDS`]); empty for none.
    pub chord: String,
}

/// FL's chord presets: id, name, and the notes above the root in semitones.
pub const CHORDS: [(&str, &str, &[i32]); 13] = [
    ("", "None", &[0]),
    ("major", "Major", &[0, 4, 7]),
    ("minor", "Minor", &[0, 3, 7]),
    ("maj7", "Major 7th", &[0, 4, 7, 11]),
    ("min7", "Minor 7th", &[0, 3, 7, 10]),
    ("dom7", "Dominant 7th", &[0, 4, 7, 10]),
    ("sus2", "Sus 2", &[0, 2, 7]),
    ("sus4", "Sus 4", &[0, 5, 7]),
    ("dim", "Diminished", &[0, 3, 6]),
    ("aug", "Augmented", &[0, 4, 8]),
    ("add9", "Add 9", &[0, 4, 7, 14]),
    ("power", "Power (5th)", &[0, 7]),
    ("octave", "Octave", &[0, 12]),
];

/// The notes above the root of a chord preset (just the root for none or one not known).
pub fn chord_intervals(id: &str) -> &'static [i32] {
    CHORDS
        .iter()
        .find(|c| c.0 == id)
        .map_or(&[0], |c| c.2)
}

impl Default for ArpState {
    fn default() -> Self {
        Self {
            on: false,
            mode: "up".into(),
            div: 0.25,
            octaves: 1.0,
            gate: 0.7,
            repeat: 1.0,
            chord: String::new(),
        }
    }
}

impl ArpState {
    /// The defaults with the keys of a saved `arp` object on top.
    pub fn from_json(v: Option<&Value>) -> Self {
        let mut s = Self::default();
        let Some(o) = v.and_then(Value::as_object) else {
            return s;
        };
        if let Some(b) = o.get("on").and_then(Value::as_bool) {
            s.on = b;
        }
        if let Some(m) = o.get("mode").and_then(Value::as_str) {
            s.mode = m.into();
        }
        if let Some(c) = o.get("chord").and_then(Value::as_str) {
            s.chord = c.into();
        }
        for (key, slot) in [
            ("div", &mut s.div),
            ("octaves", &mut s.octaves),
            ("gate", &mut s.gate),
            ("repeat", &mut s.repeat),
        ] {
            if let Some(x) = o.get(key).and_then(Value::as_f64) {
                *slot = x;
            }
        }
        s
    }
}

#[derive(Clone, Copy)]
struct Held {
    key: f64,
    vel: f64,
    start: f64,
    end: f64,
    order: u64,
}

pub struct Arpeggiator {
    held: Vec<Held>,
    counter: usize,
    order: u64,
    done_until_step: f64,
    pub state: ArpState,
    rng: Rng,
    /// The random pick, kept while it repeats.
    picked: usize,
}

impl Arpeggiator {
    pub fn new(state: ArpState) -> Self {
        Self {
            held: Vec::new(),
            counter: 0,
            order: 0,
            done_until_step: f64::NEG_INFINITY,
            state,
            rng: Rng::new(1),
            picked: 0,
        }
    }

    pub fn play_note(&mut self, key: f64, vel: f64, time: f64, duration: f64) {
        self.held.push(Held {
            key,
            vel,
            start: time,
            end: time + duration,
            order: self.order,
        });
        self.order += 1;
    }

    pub fn clear(&mut self) {
        self.held.clear();
        self.counter = 0;
        self.done_until_step = f64::NEG_INFINITY;
    }

    /// Steps starting in `[from, to)` seconds, as `(key, vel, time, duration)`.
    pub fn generate(
        &mut self,
        from: f64,
        to: f64,
        t: Transport,
        emit: &mut dyn FnMut(f64, f64, f64, f64),
    ) {
        let div = self.state.div.max(1.0 / 32.0);
        let sec_per_beat = 60.0 / t.bpm;
        let beat_at = |time: f64| t.anchor_beat + (time - t.anchor_time) / sec_per_beat;
        let time_at = |beat: f64| t.anchor_time + (beat - t.anchor_beat) * sec_per_beat;
        let mut step = (beat_at(from) / div - 1e-6).ceil();
        if step <= self.done_until_step {
            step = self.done_until_step + 1.0;
        }
        let last_step = (beat_at(to) / div - 1e-6).ceil() - 1.0;
        while step <= last_step {
            let time = time_at(step * div);
            let mut active: Vec<Held> = self
                .held
                .iter()
                .filter(|h| h.start <= time + 0.002 && h.end > time + 0.002)
                .copied()
                .collect();
            if active.is_empty() {
                self.counter = 0;
                step += 1.0;
                continue;
            }
            let dur = div * sec_per_beat * self.state.gate.clamp(0.05, 1.0);
            if self.state.mode == "order" {
                active.sort_by_key(|h| h.order);
            } else {
                active.sort_by(|a, b| a.key.total_cmp(&b.key));
            }
            if self.state.mode == "chord" {
                for h in &active {
                    emit(h.key, h.vel, time, dur);
                }
            } else {
                let mut seq = Vec::new();
                for o in 0..self.state.octaves.max(1.0).ceil() as usize {
                    for h in &active {
                        seq.push((h.key + o as f64 * 12.0, h.vel));
                    }
                }
                let n = seq.len();
                let repeat = (self.state.repeat.round().max(1.0)) as usize;
                let c = self.counter / repeat;
                let idx = match self.state.mode.as_str() {
                    "down" => n - 1 - (c % n),
                    "updown" => {
                        let period = (n * 2).saturating_sub(2).max(1);
                        let p = c % period;
                        if p < n {
                            p
                        } else {
                            period - p
                        }
                    }
                    "random" => {
                        if self.counter % repeat == 0 {
                            self.picked = (self.rng.next_f64() * n as f64).floor() as usize;
                        }
                        self.picked
                    }
                    _ => c % n,
                };
                let (key, vel) = seq[idx.min(n - 1)];
                emit(key, vel, time, dur);
            }
            self.counter += 1;
            step += 1.0;
        }
        self.done_until_step = self.done_until_step.max(last_step);
        // drop finished notes
        self.held.retain(|h| h.end > from - 1.0);
    }
}
