// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording notes, as FL Studio does: with Record armed and the song or pattern
//! playing, the keys played on a MIDI keyboard (or the computer keyboard) are written
//! into the current pattern at the time they were played. A looping pattern wraps notes
//! round to its start. The whole take is one undo step.

use las_project::create::uid;
use las_project::{Note, Project};

/// A note that has been played and let go: where it goes in the pattern, ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct RecNote {
    pub key: u8,
    pub pos: f64,
    pub len: f64,
    pub vel: f64,
}

/// How a take places its notes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placing {
    /// The pattern loops this many ticks (pattern mode), or `None` (song mode).
    pub loop_len: Option<f64>,
    /// Ticks subtracted from the song position (song mode: where the take started).
    pub origin: f64,
    /// Quantize the starts to this grid (ticks); 1 or less for none.
    pub snap: f64,
}

/// The notes of a take being recorded.
#[derive(Default)]
pub struct NoteTake {
    /// Keys held down: key, the tick it went down at, velocity.
    held: Vec<(u8, f64, f64)>,
    /// Whether the take has written anything yet (the undo step is made then).
    pub written: usize,
    /// How notes are placed, while a take is running.
    pub placing: Option<Placing>,
    /// The pattern the take writes into, and where it started in the song (song mode).
    pub pattern: usize,
    pub song_origin: Option<f64>,
}

/// Round a start to the nearest grid line.
pub fn quantize(pos: f64, snap: f64) -> f64 {
    if snap <= 1.0 {
        pos
    } else {
        (pos / snap).round() * snap
    }
}

impl NoteTake {
    /// Start a take.
    pub fn begin(&mut self, placing: Placing) {
        self.held.clear();
        self.written = 0;
        self.placing = Some(placing);
    }

    pub fn is_running(&self) -> bool {
        self.placing.is_some()
    }

    /// A key went down at song tick `tick`.
    pub fn press(&mut self, key: u8, vel: f64, tick: f64) {
        if self.placing.is_none() {
            return;
        }
        self.held.retain(|h| h.0 != key);
        self.held.push((key, tick, vel));
    }

    /// A key came up at song tick `tick`: the finished note, if that key was down.
    pub fn release(&mut self, key: u8, tick: f64) -> Option<RecNote> {
        let pl = self.placing?;
        let i = self.held.iter().position(|h| h.0 == key)?;
        let (key, start, vel) = self.held.remove(i);
        Some(place(key, start, tick, vel, pl))
    }

    /// End the take: keys still held end at `tick`.
    pub fn end(&mut self, tick: f64) -> Vec<RecNote> {
        let Some(pl) = self.placing.take() else {
            return Vec::new();
        };
        self.held
            .drain(..)
            .map(|(key, start, vel)| place(key, start, tick, vel, pl))
            .collect()
    }
}

/// Where a note from `start` to `end` (song ticks) goes in the pattern.
fn place(key: u8, start: f64, end: f64, vel: f64, pl: Placing) -> RecNote {
    let (mut pos, mut len) = (start - pl.origin, end - start);
    if let Some(l) = pl.loop_len.filter(|l| *l > 0.0) {
        // the pattern loops: a note let go after the loop came round is still one note
        pos = pos.rem_euclid(l);
        let e = (end - pl.origin).rem_euclid(l);
        len = if e >= pos { e - pos } else { e + l - pos };
    }
    let mut q = quantize(pos.max(0.0), pl.snap);
    if let Some(l) = pl.loop_len.filter(|l| *l > 0.0) {
        if q >= l - 1e-9 {
            q = 0.0;
        }
    }
    RecNote {
        key,
        pos: q,
        len: len.max(6.0),
        vel: vel.clamp(0.0, 1.0),
    }
}

/// Write recorded notes into a pattern for a channel.
pub fn write(p: &mut Project, pattern: usize, channel_id: &str, notes: &[RecNote]) {
    let Some(pat) = p.patterns.get_mut(pattern) else {
        return;
    };
    let list = pat.notes.entry(channel_id.to_string()).or_default();
    for n in notes {
        list.push(Note {
            id: uid("n"),
            key: i32::from(n.key),
            pos: n.pos,
            len: n.len,
            vel: n.vel,
            ..Note::default()
        });
    }
}

/// Make sure the song plays the pattern a song mode take was written into: a clip of it
/// where the take started, unless one is there already.
pub fn ensure_clip(p: &mut Project, pattern: usize, at: f64) {
    let Some(pat) = p.patterns.get(pattern) else {
        return;
    };
    let id = pat.id.clone();
    let len = las_engine::timeline::pattern_length(Some(pat), p.time_sig);
    let covered = p.playlist.clips.iter().any(|c| {
        c.kind == las_project::ClipKind::Pattern
            && c.ref_id == id
            && c.start <= at
            && at < c.start + c.length
    });
    if covered {
        return;
    }
    let track = crate::record::free_track(p, at);
    p.playlist.clips.push(las_project::Clip {
        id: uid("clip_"),
        kind: las_project::ClipKind::Pattern,
        ref_id: id,
        track,
        start: at,
        length: len,
        ..Default::default()
    });
}

impl crate::app::App {
    /// Start a take of notes, if the project can take them.
    pub(crate) fn begin_note_take(&mut self, from: f64) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let p = &l.project;
        let pattern = self.project.pattern.min(p.patterns.len().saturating_sub(1));
        let snap = if self.settings.quantize_recording {
            self.project.roll.snap
        } else {
            0.0
        };
        let placing = if self.project.looping {
            Placing {
                loop_len: Some(las_engine::timeline::pattern_length(
                    p.patterns.get(pattern),
                    p.time_sig,
                )),
                origin: 0.0,
                snap,
            }
        } else {
            // the take starts at the bar it was started in
            let bar = las_engine::timeline::ticks_per_bar(p.time_sig);
            Placing {
                loop_len: None,
                origin: (from / bar).floor() * bar,
                snap,
            }
        };
        self.take.begin(placing);
        self.take.pattern = pattern;
        self.take.song_origin = placing.loop_len.is_none().then_some(placing.origin);
    }

    /// The song position now, when the song is playing (not counting in).
    fn take_tick(&self) -> Option<f64> {
        let a = self.audio.as_ref()?;
        (a.meters.playing() && a.meters.counting() == 0).then(|| a.meters.position())
    }

    /// A key played live: into the take, when one is running.
    pub(crate) fn record_key(&mut self, key: u8, vel: f64, on: bool) {
        if !self.take.is_running() {
            return;
        }
        let Some(tick) = self.take_tick() else {
            return;
        };
        if on {
            self.take.press(key, vel, tick);
        } else if let Some(n) = self.take.release(key, tick) {
            self.write_take(&[n]);
        }
    }

    /// Write notes of the take: the first ones start the undo step that covers the take.
    fn write_take(&mut self, notes: &[RecNote]) {
        if notes.is_empty() {
            return;
        }
        let Some(channel_id) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(self.project.channel))
            .map(|c| c.id.clone())
        else {
            return;
        };
        let first = self.take.written == 0;
        let pattern = self.take.pattern;
        self.project
            .modify(first, |p| write(p, pattern, &channel_id, notes));
        self.take.written += notes.len();
    }

    /// End the take (playing stopped): keys still held end now, and a song mode take
    /// gets a clip of its pattern where it started.
    pub(crate) fn end_note_take(&mut self) {
        if !self.take.is_running() {
            return;
        }
        let tick = self.audio.as_ref().map_or(0.0, |a| a.meters.position());
        let rest = self.take.end(tick);
        self.write_take(&rest);
        let n = self.take.written;
        if n > 0 {
            if let Some(at) = self.take.song_origin {
                let pattern = self.take.pattern;
                self.project.modify(false, |p| ensure_clip(p, pattern, at));
            }
            self.shell.status = format!(
                "Recorded {n} note{} into the pattern. Ctrl+Z removes the whole take.",
                if n == 1 { "" } else { "s" }
            );
        }
        self.take.written = 0;
    }

    /// Hear from MIDI input: where keys go, recording, and linked controls.
    pub(crate) fn poll_midi(&mut self) {
        let to_project = self.shell.keys_to_project() && self.project.has_player;
        self.midi
            .set_channel(to_project.then_some(self.project.channel));
        for e in self.midi.drain() {
            match e {
                crate::midi::MidiEvent::Note { key, vel, on } => {
                    if to_project {
                        self.record_key(key, vel, on);
                    }
                }
                crate::midi::MidiEvent::Control { channel, cc, value } => {
                    self.midi_control(channel, cc, value);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_mode(snap: f64) -> Placing {
        Placing {
            loop_len: Some(384.0),
            origin: 0.0,
            snap,
        }
    }

    #[test]
    fn a_note_is_placed_where_it_was_played() {
        let mut t = NoteTake::default();
        t.begin(pattern_mode(0.0));
        t.press(60, 0.8, 100.0);
        let n = t.release(60, 150.0).unwrap();
        assert_eq!(
            n,
            RecNote {
                key: 60,
                pos: 100.0,
                len: 50.0,
                vel: 0.8
            }
        );
        // a key let go that was never pressed is nothing
        assert!(t.release(61, 160.0).is_none());
    }

    #[test]
    fn nothing_is_recorded_without_a_take() {
        let mut t = NoteTake::default();
        t.press(60, 0.8, 100.0);
        assert!(t.release(60, 150.0).is_none());
    }

    #[test]
    fn a_looping_pattern_wraps_notes_round() {
        let mut t = NoteTake::default();
        t.begin(pattern_mode(0.0));
        // the second time round
        t.press(62, 0.5, 384.0 + 96.0);
        let n = t.release(62, 384.0 + 120.0).unwrap();
        assert_eq!((n.pos, n.len), (96.0, 24.0));
        // held across the loop's end
        t.press(64, 0.5, 360.0);
        let n = t.release(64, 384.0 + 12.0).unwrap();
        assert_eq!((n.pos, n.len), (360.0, 36.0));
    }

    #[test]
    fn quantize_moves_starts_to_the_snap() {
        assert_eq!(quantize(30.0, 24.0), 24.0);
        assert_eq!(quantize(40.0, 24.0), 48.0);
        assert_eq!(quantize(40.0, 0.0), 40.0);
        let mut t = NoteTake::default();
        t.begin(pattern_mode(24.0));
        t.press(60, 1.0, 379.0);
        // rounds to the end of the loop, which is its start
        let n = t.release(60, 383.0).unwrap();
        assert_eq!(n.pos, 0.0);
    }

    #[test]
    fn song_mode_places_notes_from_the_take_start_and_the_end_lets_keys_go() {
        let mut t = NoteTake::default();
        t.begin(Placing {
            loop_len: None,
            origin: 768.0,
            snap: 0.0,
        });
        t.press(60, 1.0, 800.0);
        t.press(67, 1.0, 1300.0);
        let n = t.release(60, 900.0).unwrap();
        assert_eq!((n.pos, n.len), (32.0, 100.0));
        let rest = t.end(1400.0);
        assert_eq!(rest.len(), 1);
        assert_eq!((rest[0].key, rest[0].pos, rest[0].len), (67, 532.0, 100.0));
        assert!(!t.is_running());
    }

    #[test]
    fn a_song_take_gets_a_clip_of_its_pattern_once() {
        let mut p = las_project::create::empty_project("x");
        ensure_clip(&mut p, 0, 768.0);
        assert_eq!(p.playlist.clips.len(), 1);
        assert_eq!(p.playlist.clips[0].start, 768.0);
        assert_eq!(p.playlist.clips[0].ref_id, p.patterns[0].id);
        ensure_clip(&mut p, 0, 800.0);
        assert_eq!(p.playlist.clips.len(), 1, "already covered");
    }

    #[test]
    fn notes_are_written_into_the_pattern_for_the_channel() {
        let mut p = las_project::create::empty_project("x");
        let n = RecNote {
            key: 60,
            pos: 48.0,
            len: 24.0,
            vel: 0.7,
        };
        write(&mut p, 0, "ch_a", &[n.clone(), n]);
        let notes = &p.patterns[0].notes["ch_a"];
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].key, notes[0].pos, notes[0].len), (60, 48.0, 24.0));
        assert_ne!(notes[0].id, notes[1].id);
    }
}
