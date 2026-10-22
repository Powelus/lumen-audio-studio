// SPDX-License-Identifier: GPL-3.0-or-later
//! Capture MIDI, as in Ableton Live: the app always remembers the notes played on the
//! keyboard (a MIDI keyboard, the computer keys or the on-screen piano), recording or
//! not, so that a good idea played by chance can be turned into notes afterwards.

use las_project::{Note, PPQ};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A key going down or up, at a time (seconds since the log was made).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyEvent {
    pub time: f64,
    pub key: u8,
    pub vel: f64,
    pub on: bool,
}

/// Silence longer than this ends a phrase; capture takes the last phrase.
pub const PHRASE_GAP: f64 = 4.0;
/// How many events are kept.
const KEEP: usize = 4096;

/// The notes played lately. Shared with the MIDI thread, which only ever holds the
/// lock to push one event.
pub struct CaptureLog {
    start: Instant,
    events: VecDeque<KeyEvent>,
}

pub type SharedLog = Arc<Mutex<CaptureLog>>;

impl Default for CaptureLog {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            events: VecDeque::new(),
        }
    }
}

impl CaptureLog {
    pub fn shared() -> SharedLog {
        Arc::new(Mutex::new(Self::default()))
    }

    /// Seconds since the log was made.
    pub fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    pub fn push(&mut self, key: u8, vel: f64, on: bool) {
        let time = self.now();
        self.push_at(KeyEvent { time, key, vel, on });
    }

    pub fn push_at(&mut self, e: KeyEvent) {
        if self.events.len() >= KEEP {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }

    pub fn events(&self) -> Vec<KeyEvent> {
        self.events.iter().copied().collect()
    }

    /// How many notes the last phrase has (for the button's label).
    pub fn phrase_notes(&self) -> usize {
        last_phrase(&self.events()).iter().filter(|e| e.on).count()
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// The events of the last phrase: after the last silence of [`PHRASE_GAP`] or more.
pub fn last_phrase(events: &[KeyEvent]) -> Vec<KeyEvent> {
    let mut from = 0;
    let mut held: Vec<u8> = Vec::new();
    let mut last_time: Option<f64> = None;
    for (i, e) in events.iter().enumerate() {
        if e.on && held.is_empty() && last_time.is_some_and(|t| e.time - t >= PHRASE_GAP) {
            from = i;
        }
        if e.on {
            held.push(e.key);
        } else if let Some(p) = held.iter().position(|k| *k == e.key) {
            held.remove(p);
        }
        last_time = Some(e.time);
    }
    events[from..].to_vec()
}

/// How the captured notes are placed in time.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// The time (log seconds) capture happens.
    pub now: f64,
    /// Tempo used to turn seconds into ticks.
    pub bpm: f64,
    /// When playing: the song position at `now`, ticks. The notes then go where they
    /// were played against the music.
    pub playhead: Option<f64>,
    /// The pattern's length, ticks (0: unknown); positions wrap into it.
    pub pattern_len: f64,
}

/// Turn the last phrase into notes. With a playhead the notes keep their place against
/// the music (wrapped into the pattern); without one the first note starts the pattern
/// and starts are snapped to sixteenths at the project's tempo.
pub fn capture(events: &[KeyEvent], t: &Timing) -> Vec<Note> {
    let phrase = last_phrase(events);
    let tps = t.bpm.max(1.0) / 60.0 * f64::from(PPQ);
    // pair each key down with the next key up of the same key
    let mut raw: Vec<(f64, f64, u8, f64)> = Vec::new();
    for (i, e) in phrase.iter().enumerate() {
        if !e.on {
            continue;
        }
        let end = phrase[i + 1..]
            .iter()
            .find(|x| x.key == e.key)
            .map_or(t.now, |x| x.time);
        raw.push((e.time, end.max(e.time), e.key, e.vel));
    }
    let Some(first) = raw.first().map(|r| r.0) else {
        return Vec::new();
    };
    let sixteenth = f64::from(PPQ) / 4.0;
    let mut notes: Vec<Note> = raw
        .iter()
        .map(|&(a, b, key, vel)| {
            let len = ((b - a) * tps).max(sixteenth / 4.0);
            let pos = match t.playhead {
                Some(head) => {
                    let p = head - (t.now - a) * tps;
                    if t.pattern_len > 0.0 {
                        p.rem_euclid(t.pattern_len)
                    } else {
                        p.max(0.0)
                    }
                }
                None => (((a - first) * tps) / sixteenth).round() * sixteenth,
            };
            Note {
                id: las_project::create::uid("n"),
                key: i32::from(key),
                pos: pos.round().max(0.0),
                len: len.round().max(1.0),
                vel: vel.clamp(0.0, 1.0),
                ..Default::default()
            }
        })
        .collect();
    notes.sort_by(|a, b| a.pos.total_cmp(&b.pos).then(a.key.cmp(&b.key)));
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(time: f64, key: u8, on: bool) -> KeyEvent {
        KeyEvent {
            time,
            key,
            vel: if on { 0.8 } else { 0.0 },
            on,
        }
    }

    #[test]
    fn only_the_last_phrase_is_taken() {
        let events = vec![
            ev(0.0, 40, true),
            ev(0.2, 40, false),
            // a long pause, then the phrase that counts
            ev(10.0, 60, true),
            ev(10.25, 60, false),
            ev(10.5, 64, true),
            ev(10.75, 64, false),
        ];
        let p = last_phrase(&events);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].key, 60);
    }

    #[test]
    fn notes_played_while_stopped_start_the_pattern_on_sixteenths() {
        // 120 BPM: a beat is half a second, a sixteenth an eighth of one
        let events = vec![
            ev(5.0, 60, true),
            ev(5.24, 60, false),
            ev(5.51, 64, true), // a hair late for the second beat
            ev(5.75, 64, false),
            ev(6.0, 67, true), // still held at capture
        ];
        let notes = capture(
            &events,
            &Timing {
                now: 6.5,
                bpm: 120.0,
                playhead: None,
                pattern_len: 0.0,
            },
        );
        let ppq = f64::from(PPQ);
        assert_eq!(notes.len(), 3);
        assert_eq!(
            notes.iter().map(|n| (n.key, n.pos)).collect::<Vec<_>>(),
            vec![(60, 0.0), (64, ppq), (67, 2.0 * ppq)]
        );
        assert!((notes[0].len - ppq * 0.48).abs() <= 1.0);
        assert!(
            (notes[2].len - ppq).abs() <= 1.0,
            "a held note ends at capture"
        );
        assert!((notes[0].vel - 0.8).abs() < 1e-9);
    }

    #[test]
    fn notes_played_along_with_the_song_keep_their_place() {
        let ppq = f64::from(PPQ);
        // capture at 10 s with the playhead at bar 3 beat 1 (8 beats in), 120 BPM;
        // the note was played a beat and a half earlier
        let events = vec![ev(9.25, 62, true), ev(9.5, 62, false)];
        let notes = capture(
            &events,
            &Timing {
                now: 10.0,
                bpm: 120.0,
                playhead: Some(8.0 * ppq),
                pattern_len: 4.0 * ppq,
            },
        );
        assert_eq!(notes.len(), 1);
        // 6.5 beats into the song is 2.5 beats into a one-bar pattern
        assert!((notes[0].pos - 2.5 * ppq).abs() <= 1.0);
        assert!((notes[0].len - 0.5 * ppq).abs() <= 1.0);
    }

    #[test]
    fn the_log_keeps_a_bounded_history_and_nothing_is_captured_from_nothing() {
        let mut log = CaptureLog::default();
        for i in 0..(KEEP + 10) {
            log.push_at(ev(i as f64 * 0.01, 60, i % 2 == 0));
        }
        assert_eq!(log.events().len(), KEEP);
        log.clear();
        assert_eq!(log.phrase_notes(), 0);
        let t = Timing {
            now: 0.0,
            bpm: 120.0,
            playhead: None,
            pattern_len: 0.0,
        };
        assert!(capture(&[], &t).is_empty());
    }
}
