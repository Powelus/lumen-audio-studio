// SPDX-License-Identifier: GPL-3.0-or-later
//! The piano roll: the notes of one channel in one pattern, drawn on a grid and
//! edited with the mouse. Drawing only reports what the user asked for ([`Edit`]);
//! [`apply`] changes the project, so editing is testable without a window.

use crate::widgets::note_name;
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, ticks_per_bar};
use las_project::create::uid;
use las_project::{Controllers, CtlKind, Note, Pattern, Project, PPQ, TICKS_PER_STEP};
use std::collections::HashSet;

const KEYS: i32 = 128;
const GUTTER: f32 = 56.0;
/// The width, in points, of the right edge of a note that resizes it.
const EDGE: f32 = 6.0;

/// A change the user asked for.
#[derive(Clone, Debug)]
pub enum Edit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    Add(Note),
    Move {
        id: String,
        pos: f64,
        key: i32,
    },
    Resize {
        id: String,
        len: f64,
    },
    Delete {
        id: String,
    },
    /// Move several notes by the same amount in time and key.
    MoveMany {
        ids: Vec<String>,
        dpos: f64,
        dkey: i32,
    },
    DeleteMany {
        ids: Vec<String>,
    },
    /// Add notes that already have their places and new ids.
    Paste {
        notes: Vec<Note>,
    },
    /// Set a note's velocity (0 to 1).
    Velocity {
        id: String,
        vel: f64,
    },
    /// Set a note's pan (-1 to 1).
    NotePan {
        id: String,
        pan: f64,
    },
    /// Set a note's fine pitch (cents, -100 to 100).
    Fine {
        id: String,
        cents: f64,
    },
    /// Snap every note's start and length to the grid.
    Quantize {
        snap: f64,
        /// The notes to snap (all when empty).
        ids: Vec<String>,
        /// How far towards the grid starts move, 0 to 1 (lengths snap only at 1).
        strength: f64,
    },
    /// One of the Tools menu's operations, on the given notes (all notes when empty).
    Tool {
        op: NoteTool,
        ids: Vec<String>,
    },
    /// Set one event lane's value of a note (0 to 1 on the lane's scale).
    Lane {
        id: String,
        lane: Lane,
        value: f64,
    },
    /// Put a controller value (mod wheel, pitch bend, aftertouch) at a tick.
    CtlSet {
        kind: CtlKind,
        pos: f64,
        value: f64,
    },
    /// Remove a controller's points in ticks `[a, b)`.
    CtlClear {
        kind: CtlKind,
        a: f64,
        b: f64,
    },
    /// Replace a note's properties (found by its id), from the note properties window.
    SetNote(Box<Note>),
    /// Sound a key (a note being placed or moved) or let go of it.
    Audition {
        key: i32,
        on: bool,
    },
}

/// The piano roll's Tools menu, after FL's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteTool {
    /// Stretch each note to the start of the next one.
    Legato,
    /// Offset the notes of each chord, lowest first, by this many ticks each.
    Strum(f64),
    /// Mirror the keys upside down around the middle of their range.
    Flip,
    /// Mirror the notes in time.
    Reverse,
    /// Move starts and velocities by small random amounts (the seed makes it repeatable).
    Humanize(u64),
    /// Cut each note into pieces of this many ticks.
    Chop(f64),
    /// Move keys outside the scale to the nearest key inside it.
    ToScale(i32, &'static [i32]),
    /// Velocities rising from the first note to the last.
    Ramp,
    /// Double or halve the lengths and places, from the first note.
    Stretch(f64),
    /// Multiply the velocities.
    VelScale(f64),
    /// Spread the velocities around their values at random (the seed makes it repeatable).
    VelRandom(u64),
}

/// The Tools menu's entries.
pub const NOTE_TOOLS: [(&str, NoteTool, &str); 12] = [
    (
        "Legato",
        NoteTool::Legato,
        "Stretch each note to the next one",
    ),
    (
        "Strum",
        NoteTool::Strum(6.0),
        "Spread each chord, lowest note first",
    ),
    ("Flip", NoteTool::Flip, "Turn the keys upside down"),
    (
        "Reverse",
        NoteTool::Reverse,
        "Play the notes backwards in time",
    ),
    (
        "Humanize",
        NoteTool::Humanize(0),
        "Small random changes to timing and velocity",
    ),
    ("Chop", NoteTool::Chop(24.0), "Cut notes into sixteenths"),
    (
        "Velocity ramp",
        NoteTool::Ramp,
        "Velocities rising from first to last",
    ),
    (
        "Double length",
        NoteTool::Stretch(2.0),
        "Twice as long and twice as far apart",
    ),
    (
        "Half length",
        NoteTool::Stretch(0.5),
        "Half as long and half as far apart",
    ),
    (
        "Softer",
        NoteTool::VelScale(0.8),
        "Velocities down by a fifth",
    ),
    (
        "Louder",
        NoteTool::VelScale(1.25),
        "Velocities up by a quarter",
    ),
    (
        "Random velocity",
        NoteTool::VelRandom(0),
        "Spread the velocities at random",
    ),
];

/// Carry out a tool on the notes it picks.
pub fn note_tool(notes: &mut Vec<Note>, op: NoteTool, ids: &[String]) {
    let pick = |n: &Note| ids.is_empty() || ids.contains(&n.id);
    let mut idx: Vec<usize> = (0..notes.len()).filter(|&i| pick(&notes[i])).collect();
    if idx.is_empty() {
        return;
    }
    idx.sort_by(|&a, &b| {
        notes[a]
            .pos
            .total_cmp(&notes[b].pos)
            .then(notes[a].key.cmp(&notes[b].key))
    });
    let start = notes[idx[0]].pos;
    let end = idx
        .iter()
        .map(|&i| notes[i].pos + notes[i].len)
        .fold(start, f64::max);
    match op {
        NoteTool::Legato => {
            for (k, &i) in idx.iter().enumerate() {
                let next = idx[k + 1..]
                    .iter()
                    .map(|&j| notes[j].pos)
                    .find(|&p| p > notes[i].pos + 0.5);
                if let Some(p) = next {
                    notes[i].len = (p - notes[i].pos).max(1.0);
                }
            }
        }
        NoteTool::Strum(step) => {
            let mut k = 0;
            while k < idx.len() {
                let at = notes[idx[k]].pos;
                let mut chord: Vec<usize> = idx[k..]
                    .iter()
                    .copied()
                    .take_while(|&j| (notes[j].pos - at).abs() < 0.5)
                    .collect();
                k += chord.len();
                chord.sort_by_key(|&j| notes[j].key);
                for (n, &j) in chord.iter().enumerate() {
                    let d = step * n as f64;
                    notes[j].pos += d;
                    notes[j].len = (notes[j].len - d).max(1.0);
                }
            }
        }
        NoteTool::Flip => {
            let lo = idx.iter().map(|&i| notes[i].key).min().unwrap_or(0);
            let hi = idx.iter().map(|&i| notes[i].key).max().unwrap_or(0);
            for &i in &idx {
                notes[i].key = (lo + hi - notes[i].key).clamp(0, KEYS - 1);
            }
        }
        NoteTool::Reverse => {
            for &i in &idx {
                notes[i].pos = (start + end - notes[i].pos - notes[i].len).max(0.0);
            }
        }
        NoteTool::Humanize(seed) => {
            let mut x = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407)
                | 1;
            let mut rnd = || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            };
            for &i in &idx {
                notes[i].pos = (notes[i].pos + rnd() * 3.0).max(0.0);
                notes[i].vel = (notes[i].vel + rnd() * 0.08).clamp(0.05, 1.0);
            }
        }
        NoteTool::Chop(piece) => {
            let piece = piece.max(1.0);
            let mut add = Vec::new();
            for &i in &idx {
                let n = &mut notes[i];
                let mut p = n.pos + piece;
                let stop = n.pos + n.len;
                if n.len > piece {
                    n.len = piece;
                }
                while p < stop - 0.5 {
                    let mut c = n.clone();
                    c.id = uid("n");
                    c.pos = p;
                    c.len = piece.min(stop - p);
                    add.push(c);
                    p += piece;
                }
            }
            notes.extend(add);
        }
        NoteTool::ToScale(root, steps) => {
            let inside = |k: i32| steps.contains(&(k - root).rem_euclid(12));
            for &i in &idx {
                let k = notes[i].key;
                let to = (0..12)
                    .flat_map(|d| [k - d, k + d])
                    .find(|&c| inside(c))
                    .unwrap_or(k);
                notes[i].key = to.clamp(0, KEYS - 1);
            }
        }
        NoteTool::Ramp => {
            let n = idx.len().max(2) - 1;
            for (k, &i) in idx.iter().enumerate() {
                notes[i].vel = 0.3 + 0.7 * k as f64 / n as f64;
            }
        }
        NoteTool::VelScale(f) => {
            for &i in &idx {
                notes[i].vel = (notes[i].vel * f).clamp(0.02, 1.0);
            }
        }
        NoteTool::VelRandom(seed) => {
            let mut x = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
            for &i in &idx {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let r = (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
                notes[i].vel = (notes[i].vel + r * 0.25).clamp(0.05, 1.0);
            }
        }
        NoteTool::Stretch(f) => {
            for &i in &idx {
                notes[i].pos = start + (notes[i].pos - start) * f;
                notes[i].len = (notes[i].len * f).max(1.0);
            }
        }
    }
}

/// What a click on the roll does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    /// Place, move and resize notes.
    #[default]
    Draw,
    /// FL's brush: drag to draw a note on every grid step the pointer passes.
    Paint,
    /// Move and resize notes without placing new ones.
    Select,
    /// Delete the note that is clicked.
    Delete,
}

/// What is being dragged.
#[derive(Clone, Debug)]
enum Drag {
    Move {
        id: String,
        /// Ticks from the note's start to where it was grabbed.
        grab: f64,
        key: i32,
    },
    Resize {
        id: String,
    },
    /// The selected notes together.
    Group,
    /// A rectangle that selects what it touches.
    Marquee,
    /// The paint tool laying down notes.
    Paint,
    /// A right drag deleting notes (`begun` once it has made the gesture undoable).
    Erase {
        begun: bool,
    },
}

/// The event lanes under the grid, after FL's: what the bars under the notes show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lane {
    #[default]
    Velocity,
    Pan,
    Fine,
    Release,
    /// The channel's controllers over time, kept apart from the notes.
    ModWheel,
    PitchBend,
    Aftertouch,
}

/// The lanes, their names and tooltips.
pub const LANES: [(Lane, &str, &str); 7] = [
    (Lane::Velocity, "Velocity", "How hard each note is played"),
    (
        Lane::Pan,
        "Pan",
        "Where each note sits between left and right",
    ),
    (
        Lane::Fine,
        "Fine pitch",
        "Tune each note up or down by up to a semitone",
    ),
    (
        Lane::Release,
        "Release",
        "Release velocity of each note (for instruments that use it)",
    ),
    (
        Lane::ModWheel,
        "Mod wheel",
        "Draw the mod wheel (MIDI CC 1) over time, for instruments that use it",
    ),
    (
        Lane::PitchBend,
        "Pitch bend",
        "Draw pitch bends over time; the middle line is no bend",
    ),
    (
        Lane::Aftertouch,
        "Aftertouch",
        "Draw key pressure over time, for instruments that use it",
    ),
];

/// The fine pitch a note can be tuned by, in cents either way.
pub const FINE_RANGE: f64 = 100.0;

impl Lane {
    pub fn name(self) -> &'static str {
        LANES.iter().find(|l| l.0 == self).map_or("", |l| l.1)
    }

    /// Whether the lane's bars grow from the middle (pan, fine pitch, pitch bend).
    pub fn centred(self) -> bool {
        matches!(self, Lane::Pan | Lane::Fine | Lane::PitchBend)
    }

    /// The controller a lane draws, for the controller lanes.
    pub fn ctl(self) -> Option<CtlKind> {
        match self {
            Lane::ModWheel => Some(CtlKind::ModWheel),
            Lane::PitchBend => Some(CtlKind::PitchBend),
            Lane::Aftertouch => Some(CtlKind::Aftertouch),
            _ => None,
        }
    }

    /// A note's value on this lane, 0 to 1.
    pub fn get(self, n: &Note) -> f64 {
        match self {
            Lane::Velocity => n.vel.clamp(0.0, 1.0),
            Lane::Pan => ((n.pan + 1.0) / 2.0).clamp(0.0, 1.0),
            Lane::Fine => ((n.fine.unwrap_or(0.0) / FINE_RANGE + 1.0) / 2.0).clamp(0.0, 1.0),
            Lane::Release => n.release.unwrap_or(0.5).clamp(0.0, 1.0),
            Lane::ModWheel | Lane::PitchBend | Lane::Aftertouch => 0.0,
        }
    }

    /// Set a note's value on this lane from 0 to 1 (values near the middle of a centred
    /// lane snap to the middle, so centre is easy to hit).
    pub fn set(self, n: &mut Note, v: f64) {
        let v = v.clamp(0.0, 1.0);
        let mid = |v: f64| {
            let c = v * 2.0 - 1.0;
            if c.abs() < 0.03 {
                0.0
            } else {
                c
            }
        };
        match self {
            Lane::Velocity => n.vel = v,
            Lane::Pan => n.pan = mid(v),
            Lane::Fine => {
                let c = (mid(v) * FINE_RANGE).round();
                n.fine = (c != 0.0).then_some(c);
            }
            Lane::Release => n.release = Some(v),
            Lane::ModWheel | Lane::PitchBend | Lane::Aftertouch => {}
        }
    }

    /// The value in words, for the lane's readout.
    pub fn text(self, n: &Note) -> String {
        match self {
            Lane::Velocity => format!("Velocity {:.0}%", n.vel * 100.0),
            Lane::Pan => format!("Pan {}", pan_text(n.pan)),
            Lane::Fine => format!("Fine pitch {:+.0} cents", n.fine.unwrap_or(0.0)),
            Lane::Release => format!("Release {:.0}%", n.release.unwrap_or(0.5) * 100.0),
            Lane::ModWheel | Lane::PitchBend | Lane::Aftertouch => String::new(),
        }
    }
}

/// A controller value in words: "Mod wheel 64%", "Pitch bend +50%".
pub fn ctl_text(kind: CtlKind, v: f64) -> String {
    match kind {
        CtlKind::ModWheel => format!("Mod wheel {:.0}%", v * 100.0),
        CtlKind::PitchBend if v.abs() < 0.005 => "Pitch bend: none".into(),
        CtlKind::PitchBend => format!("Pitch bend {:+.0}%", v * 100.0),
        CtlKind::Aftertouch => format!("Aftertouch {:.0}%", v * 100.0),
    }
}

/// The controller values a drag from `(t0, v0)` to `(t1, v1)` lays down: one on each
/// grid step it passes (the value along the line between), at least at `t1`.
pub fn ctl_line(t0: f64, v0: f64, t1: f64, v1: f64, snap: f64) -> Vec<(f64, f64)> {
    let g = snap.max(1.0);
    let s = |t: f64| (t / g).floor() * g;
    let (a, b) = (s(t0.min(t1)).max(0.0), s(t0.max(t1)).max(0.0));
    let mut out = Vec::new();
    let mut t = a;
    while t <= b + 1e-9 {
        let k = if (t1 - t0).abs() < 1e-9 {
            1.0
        } else {
            ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
        };
        out.push((t, v0 + (v1 - v0) * k));
        t += g;
        if out.len() > 4096 {
            break;
        }
    }
    out
}

/// A pan in words: "Centre", "40% L", "100% R".
pub fn pan_text(pan: f64) -> String {
    let p = (pan * 100.0).round();
    if p == 0.0 {
        "Centre".into()
    } else if p < 0.0 {
        format!("{:.0}% L", -p)
    } else {
        format!("{p:.0}% R")
    }
}

/// The notes the paint tool lays down as the pointer moves from `last` (the grid step
/// and key painted last, if any) to `tick` on `key`: one note a grid step long on each
/// step passed, where that key has no note yet.
pub fn paint_span(
    notes: &[Note],
    last: Option<(f64, i32)>,
    tick: f64,
    key: i32,
    snap: f64,
) -> Vec<Note> {
    let g = snap.max(1.0);
    let cur = (tick.max(0.0) / g).floor() * g;
    let cells: Vec<f64> = match last {
        Some((from, k)) if k == key && from != cur => {
            let (a, b) = if from < cur {
                (from + g, cur)
            } else {
                (cur, from - g)
            };
            let n = ((b - a) / g).round().max(0.0) as usize;
            (0..=n).map(|i| a + g * i as f64).collect()
        }
        Some((from, k)) if k == key && from == cur => Vec::new(),
        _ => vec![cur],
    };
    cells
        .into_iter()
        .filter(|&c| {
            !notes
                .iter()
                .any(|n| n.key == key && c + 0.5 > n.pos && c < n.pos + n.len - 0.5)
        })
        .map(|c| new_note(key, c, g))
        .collect()
}

/// Copies of the notes picked by `ids` (every note when it is empty), placed right after
/// the last of them ends (rounded up to the grid), with new ids: FL's Ctrl+B.
pub fn duplicate(notes: &[Note], ids: &HashSet<String>, snap: f64) -> Vec<Note> {
    let sel: Vec<&Note> = notes
        .iter()
        .filter(|n| ids.is_empty() || ids.contains(&n.id))
        .collect();
    if sel.is_empty() {
        return Vec::new();
    }
    let start = sel.iter().map(|n| n.pos).fold(f64::MAX, f64::min);
    let end = sel.iter().map(|n| n.pos + n.len).fold(0.0, f64::max);
    let g = snap.max(1.0);
    let by = ((end - start) / g).ceil().max(1.0) * g;
    sel.iter()
        .map(|n| Note {
            id: uid("n"),
            pos: n.pos + by,
            ..(*n).clone()
        })
        .collect()
}

/// Where Ctrl+V puts the clipboard: right after the selection when there is one, else at
/// the playhead while playing, else where the roll was last clicked.
pub fn paste_place(
    notes: &[Note],
    selected: &HashSet<String>,
    playhead: Option<f64>,
    clicked: f64,
    snap: f64,
) -> f64 {
    let end = notes
        .iter()
        .filter(|n| selected.contains(&n.id))
        .map(|n| n.pos + n.len)
        .fold(None, |m: Option<f64>, e| Some(m.map_or(e, |m| m.max(e))));
    let g = snap.max(1.0);
    match (end, playhead) {
        (Some(e), _) => (e / g).ceil() * g,
        (None, Some(t)) => (t / g).floor() * g,
        (None, None) => clicked,
    }
    .max(0.0)
}

/// The clipboard's notes, with new ids, starting at `at`.
pub fn place_clipboard(clipboard: &[Note], at: f64) -> Vec<Note> {
    clipboard
        .iter()
        .map(|n| Note {
            id: uid("n"),
            pos: n.pos + at,
            ..n.clone()
        })
        .collect()
}

/// The scroll offset that keeps the point under the pointer still while zooming from
/// `zoom` to `to` (points per unit). `pointer` is the pointer's distance from the view's
/// edge and `fixed` the width of a pinned part (the keyboard) before the content.
pub fn zoom_about(scroll: f32, pointer: f32, fixed: f32, zoom: f32, to: f32) -> f32 {
    let at = (pointer + scroll - fixed) / zoom.max(1e-6);
    (fixed + at * to - pointer).max(0.0)
}

/// How the roll looks and what it is doing; kept between frames.
pub struct RollState {
    /// Points per tick.
    pub zoom: f32,
    pub row: f32,
    /// Snap in ticks (1 for off).
    pub snap: f64,
    /// The length a new note gets from a click.
    pub len: f64,
    pub tool: Tool,
    /// Show the other channels' notes faintly behind this one's.
    pub ghost: bool,
    /// Highlight a scale: its root (0 = C) and steps.
    pub scale: Option<(i32, &'static [i32])>,
    /// Draw a chord with each click instead of one note (FL's stamps).
    pub stamp: Option<&'static [i32]>,
    /// The notes selected (by id) and the ones copied.
    pub selected: HashSet<String>,
    pub clipboard: Vec<Note>,
    /// Where a paste lands: the last place that was clicked, in ticks.
    paste_at: f64,
    /// A rectangle being dragged to select notes (pixel corners).
    marquee: Option<(egui::Pos2, egui::Pos2)>,
    /// The place a group of notes was last dragged to (tick and key).
    group_last: Option<(f64, i32)>,
    /// How far the grid is scrolled to the right, for the velocity lane under it.
    scroll_x: f32,
    drag: Option<Drag>,
    /// The pattern and channel the view was last centred for.
    centred: Option<(usize, usize)>,
    /// The event lane shown under the grid.
    pub lane: Lane,
    /// How far Quantize moves starts towards the grid, 0 to 1.
    pub q_strength: f64,
    /// Where a drag on a controller lane last was (tick and value).
    ctl_last: Option<(f64, f64)>,
    /// The note whose properties window is open.
    pub props: Option<String>,
    /// Whether the properties window's change in progress has been made undoable.
    props_gesture: bool,
    /// The key of the keyboard at the left held down with the mouse.
    key_down: Option<i32>,
    /// The grid step and key the paint tool painted last.
    paint_last: Option<(f64, i32)>,
    /// The notes the last click added (a double click on them does not open them).
    just_added: HashSet<String>,
    scroll_y: f32,
    /// Scroll offsets to apply on the next frame (after a zoom around the pointer).
    want_x: Option<f32>,
    want_y: Option<f32>,
}

/// Scales, as steps above the root.
pub const SCALES: [(&str, &[i32]); 7] = [
    ("Major", &[0, 2, 4, 5, 7, 9, 11]),
    ("Minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("Harmonic minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("Dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("Phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("Pentatonic major", &[0, 2, 4, 7, 9]),
    ("Pentatonic minor", &[0, 3, 5, 7, 10]),
];

/// Chords a click can stamp, as intervals above the clicked key.
pub const STAMPS: [(&str, &[i32]); 10] = [
    ("Major", &[0, 4, 7]),
    ("Minor", &[0, 3, 7]),
    ("Major 7", &[0, 4, 7, 11]),
    ("Minor 7", &[0, 3, 7, 10]),
    ("Dominant 7", &[0, 4, 7, 10]),
    ("Sus 2", &[0, 2, 7]),
    ("Sus 4", &[0, 5, 7]),
    ("Diminished", &[0, 3, 6]),
    ("Power", &[0, 7, 12]),
    ("Octave", &[0, 12]),
];

/// The keys a click on `key` adds: the key, or the stamp's chord on it (keys that fall
/// off the keyboard are left out).
pub fn chord_keys(key: i32, stamp: Option<&[i32]>) -> Vec<i32> {
    match stamp {
        None => vec![key],
        Some(iv) => iv
            .iter()
            .map(|i| key + i)
            .filter(|k| (0..KEYS).contains(k))
            .collect(),
    }
}

impl Default for RollState {
    fn default() -> Self {
        Self {
            zoom: 0.5,
            row: 14.0,
            snap: f64::from(TICKS_PER_STEP),
            len: f64::from(TICKS_PER_STEP) * 4.0,
            tool: Tool::Draw,
            ghost: true,
            scale: None,
            stamp: None,
            selected: HashSet::new(),
            clipboard: Vec::new(),
            paste_at: 0.0,
            marquee: None,
            group_last: None,
            scroll_x: 0.0,
            drag: None,
            centred: None,
            lane: Lane::Velocity,
            q_strength: 1.0,
            ctl_last: None,
            props: None,
            props_gesture: false,
            key_down: None,
            paint_last: None,
            just_added: HashSet::new(),
            scroll_y: 0.0,
            want_x: None,
            want_y: None,
        }
    }
}

impl RollState {
    /// Another project was opened: what pointed into the old one goes, the way the
    /// roll is set up (zoom, snap, tool, the clipboard) stays.
    pub fn forget_project(&mut self) {
        let old = std::mem::take(self);
        *self = Self {
            zoom: old.zoom,
            row: old.row,
            snap: old.snap,
            len: old.len,
            tool: old.tool,
            ghost: old.ghost,
            scale: old.scale,
            stamp: old.stamp,
            clipboard: old.clipboard,
            lane: old.lane,
            q_strength: old.q_strength,
            ..Self::default()
        };
    }
}

fn snapped(t: f64, snap: f64) -> f64 {
    if snap <= 1.0 {
        t.round()
    } else {
        (t / snap).floor() * snap
    }
}

/// The note at a tick and key, and whether the tick is at its right edge.
fn hit(notes: &[Note], tick: f64, key: i32, edge_ticks: f64) -> Option<(usize, bool)> {
    notes
        .iter()
        .enumerate()
        .rev()
        .find(|(_, n)| n.key == key && tick >= n.pos && tick < n.pos + n.len.max(1.0))
        .map(|(i, n)| {
            (
                i,
                tick >= n.pos + n.len - edge_ticks && n.len > edge_ticks * 1.5,
            )
        })
}

/// Carry out one edit on a project.
pub fn apply(project: &mut Project, pattern: usize, channel: &str, edit: &Edit) {
    let Some(p) = project.patterns.get_mut(pattern) else {
        return;
    };
    match edit {
        Edit::CtlSet { kind, pos, value } => {
            p.controllers
                .entry(channel.to_string())
                .or_default()
                .set(*kind, *pos, *value);
            return;
        }
        Edit::CtlClear { kind, a, b } => {
            if let Some(c) = p.controllers.get_mut(channel) {
                c.clear_range(*kind, *a, *b);
                if c.is_empty() {
                    p.controllers.remove(channel);
                }
            }
            return;
        }
        _ => {}
    }
    let notes = p.notes.entry(channel.to_string()).or_default();
    match edit {
        Edit::Begin | Edit::Audition { .. } | Edit::CtlSet { .. } | Edit::CtlClear { .. } => {}
        Edit::Add(n) => notes.push(n.clone()),
        Edit::Move { id, pos, key } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                n.pos = pos.max(0.0);
                n.key = (*key).clamp(0, KEYS - 1);
            }
        }
        Edit::Resize { id, len } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                n.len = len.max(1.0);
            }
        }
        Edit::Delete { id } => notes.retain(|n| &n.id != id),
        Edit::MoveMany { ids, dpos, dkey } => {
            // never move a group so that one of its notes would leave the grid
            let first = notes
                .iter()
                .filter(|n| ids.contains(&n.id))
                .map(|n| n.pos)
                .fold(f64::MAX, f64::min);
            let dpos = if first == f64::MAX {
                0.0
            } else {
                dpos.max(-first)
            };
            for n in notes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.pos += dpos;
                n.key = (n.key + dkey).clamp(0, KEYS - 1);
            }
        }
        Edit::DeleteMany { ids } => notes.retain(|n| !ids.contains(&n.id)),
        Edit::Paste { notes: add } => notes.extend(add.iter().cloned()),
        Edit::Tool { op, ids } => note_tool(notes, *op, ids),
        Edit::Lane { id, lane, value } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                lane.set(n, *value);
            }
        }
        Edit::SetNote(m) => {
            if let Some(n) = notes.iter_mut().find(|n| n.id == m.id) {
                let fine = m.fine.unwrap_or(0.0).clamp(-FINE_RANGE, FINE_RANGE);
                *n = Note {
                    key: m.key.clamp(0, KEYS - 1),
                    pos: m.pos.max(0.0),
                    len: m.len.max(1.0),
                    vel: m.vel.clamp(0.0, 1.0),
                    pan: m.pan.clamp(-1.0, 1.0),
                    fine: (fine != 0.0).then_some(fine),
                    release: m.release.map(|r| r.clamp(0.0, 1.0)),
                    ..(**m).clone()
                };
            }
        }
        Edit::Velocity { id, vel } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                n.vel = vel.clamp(0.0, 1.0);
            }
        }
        Edit::NotePan { id, pan } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                n.pan = pan.clamp(-1.0, 1.0);
            }
        }
        Edit::Fine { id, cents } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                let c = cents.clamp(-100.0, 100.0);
                n.fine = (c.abs() > 1e-9).then_some(c);
            }
        }
        Edit::Quantize {
            snap,
            ids,
            strength,
        } => {
            let g = snap.max(1.0);
            let k = strength.clamp(0.0, 1.0);
            for n in notes
                .iter_mut()
                .filter(|n| ids.is_empty() || ids.contains(&n.id))
            {
                let to = (n.pos / g).round() * g;
                n.pos += (to - n.pos) * k;
                if k >= 1.0 {
                    n.pos = to;
                    n.len = ((n.len / g).round() * g).max(g);
                }
            }
        }
    }
    if notes.is_empty() {
        p.notes.remove(channel);
    }
}

/// A new note, with the web version's defaults.
pub fn new_note(key: i32, pos: f64, len: f64) -> Note {
    Note {
        id: uid("n"),
        key,
        pos,
        len,
        vel: 0.78,
        ..Default::default()
    }
}

/// The keys the roll answers, read from one frame's input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Keys {
    pub delete: bool,
    pub all: bool,
    pub deselect: bool,
    pub copy: bool,
    pub cut: bool,
    pub paste: bool,
    pub duplicate: bool,
    /// Ctrl+U: quantize the selection (or all).
    pub quantize: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub shift: bool,
}

impl Keys {
    /// FL's keys: Delete, Ctrl+A (all), Ctrl+D (deselect), Ctrl+C, Ctrl+X, Ctrl+V,
    /// Ctrl+B (duplicate), the arrows (Shift for octaves).
    pub fn read(i: &egui::InputState) -> Self {
        let c = i.modifiers.command;
        let k = |key| i.key_pressed(key);
        let ev = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
        Self {
            delete: k(egui::Key::Delete) || k(egui::Key::Backspace),
            all: c && k(egui::Key::A),
            deselect: c && k(egui::Key::D),
            copy: ev(|e| matches!(e, egui::Event::Copy)) || c && k(egui::Key::C),
            cut: ev(|e| matches!(e, egui::Event::Cut)) || c && k(egui::Key::X),
            paste: ev(|e| matches!(e, egui::Event::Paste(_) | egui::Event::PasteImage(_)))
                || c && k(egui::Key::V),
            duplicate: c && k(egui::Key::B),
            quantize: c && k(egui::Key::U),
            up: k(egui::Key::ArrowUp),
            down: k(egui::Key::ArrowDown),
            left: k(egui::Key::ArrowLeft),
            right: k(egui::Key::ArrowRight),
            shift: i.modifiers.shift,
        }
    }
}

/// What the keys do to the notes and the selection: the pure part of the roll's
/// keyboard, so it can be tested without a window. Returns whether something was copied.
pub fn keys_edit(
    keys: Keys,
    notes: &[Note],
    state: &mut RollState,
    playhead: Option<f64>,
    edits: &mut Vec<Edit>,
) -> bool {
    if keys.all {
        state.selected = notes.iter().map(|n| n.id.clone()).collect();
    }
    if keys.deselect {
        state.selected.clear();
    }
    let ids: Vec<String> = notes
        .iter()
        .filter(|n| state.selected.contains(&n.id))
        .map(|n| n.id.clone())
        .collect();
    let copied = (keys.copy || keys.cut) && !ids.is_empty();
    if copied {
        let first = notes
            .iter()
            .filter(|n| state.selected.contains(&n.id))
            .map(|n| n.pos)
            .fold(f64::MAX, f64::min);
        state.clipboard = notes
            .iter()
            .filter(|n| state.selected.contains(&n.id))
            .map(|n| Note {
                pos: n.pos - first,
                ..n.clone()
            })
            .collect();
    }
    if (keys.delete || keys.cut) && !ids.is_empty() {
        edits.push(Edit::Begin);
        edits.push(Edit::DeleteMany { ids: ids.clone() });
        state.selected.clear();
        return copied;
    }
    if keys.paste && !state.clipboard.is_empty() {
        let at = paste_place(notes, &state.selected, playhead, state.paste_at, state.snap);
        let fresh = place_clipboard(&state.clipboard, at);
        state.selected = fresh.iter().map(|n| n.id.clone()).collect();
        edits.push(Edit::Begin);
        edits.push(Edit::Paste { notes: fresh });
        return copied;
    }
    if keys.quantize && !notes.is_empty() {
        edits.push(Edit::Begin);
        edits.push(Edit::Quantize {
            snap: state.snap,
            ids: ids.clone(),
            strength: state.q_strength,
        });
        return copied;
    }
    if keys.duplicate && !notes.is_empty() {
        let fresh = duplicate(notes, &state.selected, state.snap);
        state.selected = fresh.iter().map(|n| n.id.clone()).collect();
        edits.push(Edit::Begin);
        edits.push(Edit::Paste { notes: fresh });
        return copied;
    }
    if !ids.is_empty() && (keys.up || keys.down || keys.left || keys.right) {
        let oct = if keys.shift { 12 } else { 1 };
        let dkey = (i32::from(keys.up) - i32::from(keys.down)) * oct;
        let dpos = f64::from(i32::from(keys.right) - i32::from(keys.left)) * state.snap.max(1.0);
        edits.push(Edit::Begin);
        edits.push(Edit::MoveMany { ids, dpos, dkey });
    }
    copied
}

/// The roll's keys, while the pointer is over it and no text field has the keyboard.
fn keyboard(
    ui: &mut Ui,
    area: Rect,
    notes: &[Note],
    state: &mut RollState,
    playhead: Option<f64>,
    edits: &mut Vec<Edit>,
) {
    if !ui.rect_contains_pointer(area) || ui.memory(|m| m.focused().is_some()) {
        return;
    }
    let keys = ui.input(Keys::read);
    if keys_edit(keys, notes, state, playhead, edits) {
        // something on the system clipboard, so that Ctrl+V comes back to us
        ui.ctx().copy_text(format!(
            "{} notes from Lumen Audio Studio",
            state.clipboard.len()
        ));
    }
}

/// The height of the event lane under the grid.
const LANE: f32 = 84.0;

/// Ctrl+wheel zooms the roll in time around the pointer, Alt+wheel (or Shift+wheel)
/// zooms the keys; `region` is where the roll is about to be drawn.
fn wheel_zoom(ui: &mut Ui, region: Rect, state: &mut RollState) {
    let Some(p) = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| region.contains(*p) && crate::widgets::on_top(ui, *p))
    else {
        return;
    };
    let (z, scroll, m) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers));
    if m.command && (z - 1.0).abs() > 1e-4 {
        let to = (state.zoom * z).clamp(0.1, 2.0);
        state.want_x = Some(zoom_about(
            state.scroll_x,
            p.x - region.left(),
            GUTTER,
            state.zoom,
            to,
        ));
        state.zoom = to;
    } else if (m.alt || m.shift) && !m.command && scroll != egui::Vec2::ZERO {
        let to = (state.row * ((scroll.x + scroll.y) * 0.004).exp()).clamp(6.0, 40.0);
        state.want_y = Some(zoom_about(
            state.scroll_y,
            p.y - region.top(),
            0.0,
            state.row,
            to,
        ));
        state.row = to;
        ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }
}

/// Draw the roll of `channel` in `pattern`. Returns the edits asked for this frame.
pub fn draw(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    channel: usize,
    state: &mut RollState,
    playhead: Option<f64>,
) -> Vec<Edit> {
    let mut edits = Vec::new();
    let pl = crate::theme::pal();
    let Some(ch) = project.channels.get(channel) else {
        return edits;
    };
    let pat: Option<&Pattern> = project.patterns.get(pattern);
    let ticks = pattern_length(pat, project.time_sig);
    let empty: Vec<Note> = Vec::new();
    let notes: &[Note] = pat.and_then(|p| p.notes.get(&ch.id)).unwrap_or(&empty);
    let bar = ticks_per_bar(project.time_sig);
    let beat = f64::from(PPQ);

    wheel_zoom(ui, ui.available_rect_before_wrap(), state);
    let (zoom, rowh) = (state.zoom, state.row);
    // the grid runs at least as wide as the window, like FL's
    // (less the scroll bar beside it, or the window would widen by that every frame)
    let room = ui.available_width() - ui.spacing().scroll.allocated_width() - 2.0;
    let width = (GUTTER + (ticks as f32 + bar as f32) * zoom).max(room);
    let height = KEYS as f32 * rowh;
    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - LANE - 36.0).max(120.0))
        .id_salt("roll");
    if state.centred != Some((pattern, channel)) {
        // bring the notes into view: the middle of their keys, or middle C
        let mid = if notes.is_empty() {
            60
        } else {
            (notes.iter().map(|n| n.key).min().unwrap_or(60)
                + notes.iter().map(|n| n.key).max().unwrap_or(60))
                / 2
        };
        let y = ((KEYS - 1 - mid) as f32 * rowh - 140.0).max(0.0);
        area = area.vertical_scroll_offset(y);
        state.centred = Some((pattern, channel));
    }
    if let Some(x) = state.want_x.take() {
        area = area.horizontal_scroll_offset(x);
    }
    if let Some(y) = state.want_y.take() {
        area = area.vertical_scroll_offset(y);
    }
    let out = area.show_viewport(ui, |ui, viewport| {
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
        let origin = resp.rect.min;
        let grid_left = origin.x + GUTTER;
        let x_of = |t: f64| grid_left + t as f32 * zoom;
        let y_of = |k: i32| origin.y + (KEYS - 1 - k) as f32 * rowh;
        let tick_at = |x: f32| f64::from((x - grid_left) / zoom);
        let key_at =
            |y: f32| (KEYS - 1 - ((y - origin.y) / rowh).floor() as i32).clamp(0, KEYS - 1);

        // rows
        let first = (viewport.min.y / rowh).floor() as i32;
        let last = ((viewport.max.y / rowh).ceil() as i32 + 1).min(KEYS);
        for r in first.max(0)..last {
            let k = KEYS - 1 - r;
            let black = matches!(k % 12, 1 | 3 | 6 | 8 | 10);
            let mut shade = if black { pl.roll_row_b } else { pl.roll_row_a };
            if let Some((root, steps)) = state.scale {
                // keys outside the scale are dimmed, the root is lit
                let deg = (k - root).rem_euclid(12);
                if deg == 0 {
                    shade = shade.lerp_to_gamma(pl.accent, 0.16);
                } else if !steps.contains(&deg) {
                    shade = shade.gamma_multiply(0.55);
                }
            }
            let row = Rect::from_min_size(
                egui::pos2(grid_left, y_of(k)),
                egui::vec2(width - GUTTER, rowh),
            );
            painter.rect_filled(row, 0.0, shade);
            if k % 12 == 0 {
                painter.line_segment(
                    [row.left_bottom(), row.right_bottom()],
                    Stroke::new(1.0, pl.grid_bar),
                );
            }
        }
        // beat and bar lines (and steps when zoomed in)
        let step = f64::from(TICKS_PER_STEP);
        let end = f64::from((width - GUTTER) / zoom).max(ticks + bar);
        let mut t = 0.0;
        while t <= end {
            let on_bar = (t / bar).fract().abs() < 1e-9;
            let on_beat = (t / beat).fract().abs() < 1e-9;
            if on_bar || on_beat || zoom > 0.9 {
                let g = if on_bar {
                    pl.grid_bar
                } else if on_beat {
                    crate::theme::mix(pl.grid, pl.grid_bar, 0.5)
                } else {
                    pl.grid
                };
                painter.line_segment(
                    [
                        egui::pos2(x_of(t), origin.y + viewport.min.y),
                        egui::pos2(x_of(t), origin.y + viewport.max.y),
                    ],
                    Stroke::new(1.0, g),
                );
            }
            t += step;
        }
        // the end of the pattern
        painter.line_segment(
            [
                egui::pos2(x_of(ticks), origin.y + viewport.min.y),
                egui::pos2(x_of(ticks), origin.y + viewport.max.y),
            ],
            Stroke::new(2.0, crate::theme::accent().gamma_multiply(0.7)),
        );
        // the other channels' notes, faintly
        if state.ghost {
            if let Some(pat) = pat {
                for (i, c) in project.channels.iter().enumerate() {
                    if i == channel {
                        continue;
                    }
                    let col = crate::rack::channel_colour(c);
                    for n in pat.notes.get(&c.id).into_iter().flatten() {
                        let r = Rect::from_min_max(
                            egui::pos2(x_of(n.pos), y_of(n.key) + 2.0),
                            egui::pos2(
                                (x_of(n.pos + n.len) - 1.0).max(x_of(n.pos) + 3.0),
                                y_of(n.key) + rowh - 1.0,
                            ),
                        );
                        painter.rect_filled(r, 2.0, col.gamma_multiply(0.22));
                    }
                }
            }
        }
        // notes
        for n in notes {
            let r = Rect::from_min_max(
                egui::pos2(x_of(n.pos), y_of(n.key) + 1.0),
                egui::pos2(
                    (x_of(n.pos + n.len) - 1.0).max(x_of(n.pos) + 3.0),
                    y_of(n.key) + rowh,
                ),
            );
            if r.right() < origin.x + viewport.min.x || r.left() > origin.x + viewport.max.x {
                continue;
            }
            let a = (0.5 + 0.5 * n.vel.clamp(0.0, 1.0)) as f32;
            painter.rect_filled(r, 2.0, crate::theme::accent().gamma_multiply(a));
            let sel = state.selected.contains(&n.id);
            painter.rect_stroke(
                r,
                2.0,
                if sel {
                    Stroke::new(2.0, Color32::WHITE)
                } else {
                    Stroke::new(1.0, Color32::from_black_alpha(140))
                },
                egui::StrokeKind::Inside,
            );
            if rowh >= 11.0 && r.width() > 26.0 {
                // the key's name on the note, as FL shows it
                painter.with_clip_rect(r).text(
                    r.left_center() + egui::vec2(3.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    note_name(n.key.clamp(0, 127) as u8),
                    egui::FontId::proportional((rowh - 4.0).min(10.0)),
                    Color32::from_black_alpha(200),
                );
            }
        }
        if let Some((a, b)) = state.marquee {
            let m = Rect::from_two_pos(a, b);
            painter.rect_filled(m, 0.0, pl.accent.gamma_multiply(0.18));
            painter.rect_stroke(
                m,
                0.0,
                Stroke::new(1.0, pl.accent),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(t) = playhead {
            painter.line_segment(
                [
                    egui::pos2(x_of(t), origin.y + viewport.min.y),
                    egui::pos2(x_of(t), origin.y + viewport.max.y),
                ],
                Stroke::new(1.5, Color32::WHITE),
            );
        }
        // the keys, pinned to the left of the view; the one held down is lit
        let kx = origin.x + viewport.min.x;
        for r in first.max(0)..last {
            let k = KEYS - 1 - r;
            let black = matches!(k % 12, 1 | 3 | 6 | 8 | 10);
            let cell = Rect::from_min_size(egui::pos2(kx, y_of(k)), egui::vec2(GUTTER, rowh));
            let fill = if state.key_down == Some(k) {
                pl.accent
            } else if black {
                pl.key_black
            } else {
                pl.key_white
            };
            painter.rect_filled(cell, 0.0, fill);
            if k % 12 == 0 || rowh >= 14.0 && !black {
                painter.text(
                    cell.right_center() - egui::vec2(4.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    note_name(k as u8),
                    egui::FontId::proportional(9.5),
                    if black {
                        crate::theme::mix(pl.key_white, pl.key_black, 0.45)
                    } else {
                        crate::theme::mix(pl.key_black, pl.key_white, 0.35)
                    },
                );
            }
        }

        // the mouse
        let in_keys = |p: egui::Pos2| p.x < kx + GUTTER;
        let edge = f64::from(EDGE / zoom);
        let snap = state.snap.max(1.0);
        let pos = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let ptr = |p: egui::Pos2| (tick_at(p.x), key_at(p.y));
        let shift = ui.input(|i| i.modifiers.shift);
        let press = ui.input(|i| i.pointer.press_origin());
        let on_keys = press.is_some_and(in_keys);

        // the cursor tells what a press would do
        if let Some(h) = resp.hover_pos().filter(|h| !in_keys(*h)) {
            let (t, k) = ptr(h);
            let icon = match (&state.drag, hit(notes, t, k, edge)) {
                (Some(Drag::Resize { .. }), _) | (None, Some((_, true))) => {
                    Some(egui::CursorIcon::ResizeHorizontal)
                }
                (Some(Drag::Move { .. } | Drag::Group), _) => Some(egui::CursorIcon::Grabbing),
                (None, Some((_, false))) if state.tool != Tool::Delete => {
                    Some(egui::CursorIcon::Grab)
                }
                _ => None,
            };
            if let Some(icon) = icon {
                ui.ctx().set_cursor_icon(icon);
            }
        } else if resp.hover_pos().is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // the keyboard at the left plays its keys through the channel
        let primary_down = ui.input(|i| i.pointer.primary_down());
        if primary_down && on_keys && resp.is_pointer_button_down_on() {
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                let k = key_at(p.y);
                if state.key_down != Some(k) {
                    if let Some(o) = state.key_down {
                        edits.push(Edit::Audition { key: o, on: false });
                    }
                    edits.push(Edit::Audition { key: k, on: true });
                    state.key_down = Some(k);
                }
            }
        } else if let Some(o) = state.key_down.take() {
            edits.push(Edit::Audition { key: o, on: false });
        }

        // the notes on the line from `a` to `b`, so a quick swipe of the delete tool
        // misses none between the frames
        let swept = |a: egui::Pos2, b: egui::Pos2| -> Vec<usize> {
            let steps = ((b - a).length() / 3.0).ceil().max(1.0) as usize;
            let mut out: Vec<usize> = Vec::new();
            for s in 0..=steps {
                let (t, k) = ptr(a + (b - a) * (s as f32 / steps as f32));
                if let Some((i, _)) = hit(notes, t, k, edge) {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
            out
        };
        if resp.drag_started_by(egui::PointerButton::Primary) && !on_keys {
            if let Some(op) = press {
                let (t, k) = ptr(op);
                state.just_added.clear();
                match (state.tool, hit(notes, t, k, edge)) {
                    (Tool::Delete, _) => {
                        let gone = swept(op, pos.unwrap_or(op));
                        if !gone.is_empty() {
                            edits.push(Edit::Begin);
                            for i in &gone {
                                edits.push(Edit::Delete {
                                    id: notes[*i].id.clone(),
                                });
                            }
                        }
                        state.drag = Some(Drag::Erase {
                            begun: !gone.is_empty(),
                        });
                    }
                    (_, Some((i, false))) if shift => {
                        // Shift+drag copies, as in FL: the copy moves, the original stays
                        let n = &notes[i];
                        edits.push(Edit::Begin);
                        if state.selected.contains(&n.id) && state.selected.len() > 1 {
                            let fresh: Vec<Note> = notes
                                .iter()
                                .filter(|m| state.selected.contains(&m.id))
                                .map(|m| Note {
                                    id: uid("n"),
                                    ..m.clone()
                                })
                                .collect();
                            state.selected = fresh.iter().map(|m| m.id.clone()).collect();
                            edits.push(Edit::Paste { notes: fresh });
                            state.drag = Some(Drag::Group);
                            state.group_last = Some((snapped(t, state.snap), k));
                        } else {
                            let c = Note {
                                id: uid("n"),
                                ..n.clone()
                            };
                            state.selected = HashSet::from([c.id.clone()]);
                            state.drag = Some(Drag::Move {
                                id: c.id.clone(),
                                grab: t - n.pos,
                                key: n.key,
                            });
                            edits.push(Edit::Add(c));
                        }
                        edits.push(Edit::Audition {
                            key: n.key,
                            on: true,
                        });
                    }
                    (_, Some((i, true))) => {
                        edits.push(Edit::Begin);
                        state.drag = Some(Drag::Resize {
                            id: notes[i].id.clone(),
                        });
                    }
                    (tool, Some((i, false))) => {
                        let n = &notes[i];
                        edits.push(Edit::Begin);
                        state.len = n.len;
                        let many = state.selected.contains(&n.id) && state.selected.len() > 1;
                        if many || tool == Tool::Select {
                            if !state.selected.contains(&n.id) {
                                state.selected.clear();
                                state.selected.insert(n.id.clone());
                            }
                            state.drag = Some(Drag::Group);
                            state.group_last = Some((snapped(t, state.snap), k));
                        } else {
                            state.selected = HashSet::from([n.id.clone()]);
                            state.drag = Some(Drag::Move {
                                id: n.id.clone(),
                                grab: t - n.pos,
                                key: n.key,
                            });
                        }
                        edits.push(Edit::Audition {
                            key: n.key,
                            on: true,
                        });
                    }
                    (Tool::Select, None) => {
                        if !shift {
                            state.selected.clear();
                        }
                        state.drag = Some(Drag::Marquee);
                        state.marquee = Some((op, op));
                    }
                    (Tool::Paint, None) => {
                        edits.push(Edit::Begin);
                        for n in paint_span(notes, None, t, k, snap) {
                            edits.push(Edit::Add(n));
                        }
                        state.paint_last = Some(((t.max(0.0) / snap).floor() * snap, k));
                        state.drag = Some(Drag::Paint);
                        edits.push(Edit::Audition { key: k, on: true });
                    }
                    (Tool::Draw, None) => {
                        // a new note with the last length, that follows the pointer
                        edits.push(Edit::Begin);
                        let at = snapped(t, state.snap).max(0.0);
                        let fresh: Vec<Note> = chord_keys(k, state.stamp)
                            .into_iter()
                            .map(|key| new_note(key, at, state.len))
                            .collect();
                        state.selected = fresh.iter().map(|n| n.id.clone()).collect();
                        state.drag = Some(if fresh.len() == 1 {
                            Drag::Move {
                                id: fresh[0].id.clone(),
                                grab: t - at,
                                key: k,
                            }
                        } else {
                            state.group_last = Some((snapped(t, state.snap), k));
                            Drag::Group
                        });
                        for n in fresh {
                            edits.push(Edit::Add(n));
                        }
                        edits.push(Edit::Audition { key: k, on: true });
                    }
                }
            }
        } else if resp.dragged_by(egui::PointerButton::Primary) && !on_keys {
            if let (Some(d), Some(p)) = (state.drag.clone(), pos) {
                let (t, k) = ptr(p);
                match d {
                    Drag::Paint => {
                        for n in paint_span(notes, state.paint_last, t, k, snap) {
                            edits.push(Edit::Add(n));
                        }
                        if let Some((_, was)) = state.paint_last {
                            if was != k {
                                edits.push(Edit::Audition {
                                    key: was,
                                    on: false,
                                });
                                edits.push(Edit::Audition { key: k, on: true });
                            }
                        }
                        state.paint_last = Some(((t.max(0.0) / snap).floor() * snap, k));
                    }
                    Drag::Resize { id } => {
                        if let Some(n) = notes.iter().find(|n| n.id == id) {
                            let end = snapped(t, state.snap) + snap;
                            edits.push(Edit::Resize {
                                id,
                                len: (end - n.pos).max(snap),
                            });
                        }
                    }
                    Drag::Group => {
                        let cur = (snapped(t, state.snap), k);
                        if let Some(last) = state.group_last {
                            let (dpos, dkey) = (cur.0 - last.0, cur.1 - last.1);
                            if dpos != 0.0 || dkey != 0 {
                                edits.push(Edit::MoveMany {
                                    ids: state.selected.iter().cloned().collect(),
                                    dpos,
                                    dkey,
                                });
                                if dkey != 0 {
                                    edits.push(Edit::Audition {
                                        key: last.1,
                                        on: false,
                                    });
                                    edits.push(Edit::Audition {
                                        key: cur.1,
                                        on: true,
                                    });
                                }
                                state.group_last = Some(cur);
                            }
                        }
                    }
                    Drag::Marquee => {
                        if let Some((a, _)) = state.marquee {
                            state.marquee = Some((a, p));
                        }
                    }
                    Drag::Erase { begun } => {
                        // every note on the way from where the pointer was last frame,
                        // so a quick swipe misses none
                        let delta = ui.input(|i| i.pointer.delta());
                        let gone = swept(p - delta, p);
                        let _ = (t, k);
                        if !gone.is_empty() {
                            if !begun {
                                edits.push(Edit::Begin);
                            }
                            for i in gone {
                                edits.push(Edit::Delete {
                                    id: notes[i].id.clone(),
                                });
                            }
                            state.drag = Some(Drag::Erase { begun: true });
                        }
                    }
                    Drag::Move { id, grab, key } => {
                        let at = snapped(t - grab, state.snap).max(0.0);
                        if k != key {
                            edits.push(Edit::Audition { key, on: false });
                            edits.push(Edit::Audition { key: k, on: true });
                            state.drag = Some(Drag::Move {
                                id: id.clone(),
                                grab,
                                key: k,
                            });
                        }
                        edits.push(Edit::Move {
                            id,
                            pos: at,
                            key: k,
                        });
                    }
                }
            }
        } else if resp.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().filter(|p| !in_keys(*p)) {
                let (t, k) = ptr(p);
                state.paste_at = snapped(t, state.snap).max(0.0);
                let under = hit(notes, t, k, edge);
                let added_before = std::mem::take(&mut state.just_added);
                match state.tool {
                    Tool::Select => match under {
                        Some((i, _)) => {
                            let id = notes[i].id.clone();
                            if shift && state.selected.contains(&id) {
                                state.selected.remove(&id);
                            } else {
                                if !shift {
                                    state.selected.clear();
                                }
                                state.selected.insert(id);
                            }
                        }
                        None if !shift => state.selected.clear(),
                        None => {}
                    },
                    Tool::Delete => {
                        if let Some((i, _)) = under {
                            edits.push(Edit::Begin);
                            edits.push(Edit::Delete {
                                id: notes[i].id.clone(),
                            });
                        }
                    }
                    Tool::Draw | Tool::Paint => match under {
                        None => {
                            edits.push(Edit::Begin);
                            let at = snapped(t, state.snap).max(0.0);
                            let len = if state.tool == Tool::Paint {
                                snap
                            } else {
                                state.len
                            };
                            state.selected.clear();
                            for key in chord_keys(k, state.stamp) {
                                let n = new_note(key, at, len);
                                state.just_added.insert(n.id.clone());
                                edits.push(Edit::Add(n));
                            }
                            edits.push(Edit::Audition { key: k, on: true });
                            edits.push(Edit::Audition { key: k, on: false });
                        }
                        Some((i, _)) => {
                            // a click on a note picks up its length for the next ones
                            state.len = notes[i].len;
                            state.just_added = added_before;
                        }
                    },
                }
            }
        }
        if resp.double_clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().filter(|p| !in_keys(*p)) {
                let (t, k) = ptr(p);
                if let Some((i, _)) = hit(notes, t, k, edge) {
                    if !state.just_added.contains(&notes[i].id) {
                        state.props = Some(notes[i].id.clone());
                        state.props_gesture = false;
                    }
                }
            }
        }
        // a right click deletes the note under it; a right drag deletes all it passes
        if resp.dragged_by(egui::PointerButton::Secondary)
            || resp.clicked_by(egui::PointerButton::Secondary)
        {
            if let Some(p) = pos.filter(|p| !in_keys(*p)) {
                // a right drag deletes all it passed since the last frame
                let from = if resp.dragged_by(egui::PointerButton::Secondary) {
                    p - ui.input(|i| i.pointer.delta())
                } else {
                    p
                };
                let gone = swept(from, p);
                if !gone.is_empty() {
                    let begun = matches!(state.drag, Some(Drag::Erase { begun: true }));
                    if !begun {
                        edits.push(Edit::Begin);
                    }
                    if resp.dragged_by(egui::PointerButton::Secondary) {
                        state.drag = Some(Drag::Erase { begun: true });
                    }
                    for i in gone {
                        state.selected.remove(&notes[i].id);
                        edits.push(Edit::Delete {
                            id: notes[i].id.clone(),
                        });
                    }
                }
            }
        }
        if resp.drag_stopped() {
            match state.drag.take() {
                Some(Drag::Group) => {
                    if let Some((_, key)) = state.group_last.take() {
                        edits.push(Edit::Audition { key, on: false });
                    }
                }
                Some(Drag::Marquee) => {
                    if let Some((a, b)) = state.marquee.take() {
                        let area = Rect::from_two_pos(a, b);
                        for n in notes {
                            let r = Rect::from_min_max(
                                egui::pos2(x_of(n.pos), y_of(n.key)),
                                egui::pos2(x_of(n.pos + n.len), y_of(n.key) + rowh),
                            );
                            if r.intersects(area) {
                                state.selected.insert(n.id.clone());
                            }
                        }
                    }
                }
                Some(Drag::Paint) => {
                    if let Some((_, key)) = state.paint_last.take() {
                        edits.push(Edit::Audition { key, on: false });
                    }
                }
                Some(Drag::Move { key, .. }) => edits.push(Edit::Audition { key, on: false }),
                Some(Drag::Resize { id }) => {
                    if let Some(n) = notes.iter().find(|n| n.id == id) {
                        state.len = n.len;
                        edits.push(Edit::Audition {
                            key: n.key,
                            on: false,
                        });
                    }
                }
                Some(Drag::Erase { .. }) | None => {}
            }
        }
    });
    state.scroll_x = out.state.clamped_offset().x;
    state.scroll_y = out.state.clamped_offset().y;
    keyboard(ui, out.inner_rect, notes, state, playhead, &mut edits);
    let ctl = pat.and_then(|p| p.controllers.get(&ch.id));
    lanes(ui, notes, ctl, state, &mut edits);
    properties(ui, notes, state, &mut edits);
    edits
}

/// The event lane under the grid: a row of lane names to pick from, and a bar for each
/// note that a drag sets (only the selected notes' bars when some are selected).
fn lanes(
    ui: &mut Ui,
    notes: &[Note],
    ctl: Option<&Controllers>,
    state: &mut RollState,
    edits: &mut Vec<Edit>,
) {
    let pl = crate::theme::pal();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Event lane").weak().size(11.0));
        for (lane, name, tip) in LANES {
            ui.selectable_value(&mut state.lane, lane, name)
                .on_hover_text(tip);
        }
        // cut short where the window ends, so it never widens the window
        let help = if state.lane.ctl().is_some() {
            "Drag to draw. Right drag to erase."
        } else {
            "Drag over the bars to set them. Double click a note for all its properties."
        };
        ui.add(egui::Label::new(egui::RichText::new(help).weak().size(11.0)).truncate())
            .on_hover_text(help);
    });
    let lane = state.lane;
    if let Some(kind) = lane.ctl() {
        ctl_lane(ui, kind, ctl, state, edits);
        return;
    }
    let (rect, lresp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), LANE),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, pl.well_deep);
    painter.text(
        rect.left_top() + egui::vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        lane.name().to_uppercase(),
        egui::FontId::proportional(9.5),
        pl.muted,
    );
    let (top, bottom) = (rect.top() + 6.0, rect.bottom() - 4.0);
    let bar_h = bottom - top;
    let y_of = |v: f64| bottom - bar_h * v as f32;
    if lane.centred() {
        painter.line_segment(
            [
                egui::pos2(rect.left() + GUTTER, y_of(0.5)),
                egui::pos2(rect.right(), y_of(0.5)),
            ],
            Stroke::new(1.0, pl.grid_bar),
        );
    }
    let lx = |t: f64| rect.left() + GUTTER + t as f32 * state.zoom - state.scroll_x;
    let some_selected = notes.iter().any(|n| state.selected.contains(&n.id));
    for n in notes {
        let x = lx(n.pos);
        if x > rect.right() || x < rect.left() + GUTTER - 2.0 {
            continue;
        }
        let v = lane.get(n);
        let base = if lane.centred() { 0.5 } else { 0.0 };
        let (a, b) = (y_of(base), y_of(v));
        let picked = !some_selected || state.selected.contains(&n.id);
        let col = if picked {
            pl.accent
        } else {
            pl.accent.gamma_multiply(0.35)
        };
        // a stem from the note's start and a knob at its value, like FL's lanes
        painter.line_segment(
            [egui::pos2(x + 1.0, a), egui::pos2(x + 1.0, b)],
            Stroke::new(2.0, col),
        );
        let w = (n.len as f32 * state.zoom - 2.0).clamp(3.0, 14.0);
        painter.rect_filled(
            Rect::from_min_max(egui::pos2(x, b - 1.5), egui::pos2(x + w, b + 1.5)),
            1.0,
            col,
        );
    }
    let (zoom, scroll) = (state.zoom, state.scroll_x);
    let at = |p: egui::Pos2| f64::from((p.x - rect.left() - GUTTER + scroll) / zoom);
    let under = |tick: f64| {
        notes.iter().filter(move |n| {
            let w = f64::from(4.0 / zoom).max(n.len);
            tick >= n.pos - f64::from(2.0 / zoom) && tick < n.pos + w
        })
    };
    if let Some(p) = lresp.hover_pos() {
        if let Some(n) = under(at(p)).next() {
            lresp.clone().on_hover_text_at_pointer(lane.text(n));
        }
    }
    if lresp.drag_started_by(egui::PointerButton::Primary)
        || lresp.clicked_by(egui::PointerButton::Primary)
    {
        edits.push(Edit::Begin);
    }
    if lresp.dragged_by(egui::PointerButton::Primary)
        || lresp.clicked_by(egui::PointerButton::Primary)
        || lresp.drag_started_by(egui::PointerButton::Primary)
    {
        if let Some(p) = lresp.interact_pointer_pos() {
            let value = f64::from(((bottom - p.y) / bar_h).clamp(0.0, 1.0));
            for n in under(at(p)).filter(|n| !some_selected || state.selected.contains(&n.id)) {
                edits.push(Edit::Lane {
                    id: n.id.clone(),
                    lane,
                    value,
                });
            }
        }
    }
}

/// A controller lane: the values as a line that holds from point to point, drawn
/// with the mouse (snapped to the grid) and erased with a right drag.
fn ctl_lane(
    ui: &mut Ui,
    kind: CtlKind,
    ctl: Option<&Controllers>,
    state: &mut RollState,
    edits: &mut Vec<Edit>,
) {
    let pl = crate::theme::pal();
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), LANE),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, pl.well_deep);
    painter.text(
        rect.left_top() + egui::vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        kind_name(kind).to_uppercase(),
        egui::FontId::proportional(9.5),
        pl.muted,
    );
    let (top, bottom) = (rect.top() + 6.0, rect.bottom() - 4.0);
    let h = bottom - top;
    // the lane's scale, 0 to 1 on screen (pitch bend's middle is no bend)
    let to01 = |v: f64| if kind.bipolar() { (v + 1.0) / 2.0 } else { v };
    let from01 = |u: f64| if kind.bipolar() { u * 2.0 - 1.0 } else { u };
    let y_of = |v: f64| bottom - h * to01(v).clamp(0.0, 1.0) as f32;
    let left = rect.left() + GUTTER;
    if kind.bipolar() {
        painter.line_segment(
            [
                egui::pos2(left, y_of(0.0)),
                egui::pos2(rect.right(), y_of(0.0)),
            ],
            Stroke::new(1.0, pl.grid_bar),
        );
    }
    let (zoom, scroll) = (state.zoom, state.scroll_x);
    let lx = |t: f64| left + t as f32 * zoom - scroll;
    let at = |p: egui::Pos2| f64::from((p.x - left + scroll) / zoom).max(0.0);
    let empty = Vec::new();
    let pts = ctl.map_or(&empty, |c| c.lane(kind));
    // the held line, filled under it
    let mut prev = (left, y_of(kind.rest()));
    let fill = pl.accent.gamma_multiply(0.25);
    let base = y_of(if kind.bipolar() { 0.0 } else { kind.rest() });
    let seg = |x0: f32, x1: f32, y: f32| {
        let (x0, x1) = (x0.max(left), x1.min(rect.right()));
        if x1 > x0 {
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(x0, y.min(base)), egui::pos2(x1, y.max(base))),
                0.0,
                fill,
            );
            painter.line_segment(
                [egui::pos2(x0, y), egui::pos2(x1, y)],
                Stroke::new(1.5, pl.accent),
            );
        }
    };
    for p in pts {
        let x = lx(p.pos);
        seg(prev.0, x, prev.1);
        prev = (x, y_of(p.v));
    }
    seg(prev.0, rect.right(), prev.1);
    for p in pts {
        let x = lx(p.pos);
        if x >= left - 2.0 && x <= rect.right() {
            painter.circle_filled(egui::pos2(x, y_of(p.v)), 2.5, pl.accent);
        }
    }
    let value_at = |p: egui::Pos2| from01(f64::from(((bottom - p.y) / h).clamp(0.0, 1.0)));
    if let Some(p) = resp.hover_pos() {
        let v = ctl.map_or(kind.rest(), |c| c.value_at(kind, at(p)));
        resp.clone().on_hover_text_at_pointer(ctl_text(kind, v));
    }
    let snap = state.snap;
    let started = resp.drag_started() || resp.clicked() || resp.secondary_clicked();
    if started {
        edits.push(Edit::Begin);
        state.ctl_last = None;
    }
    let primary = resp.dragged_by(egui::PointerButton::Primary)
        || resp.clicked_by(egui::PointerButton::Primary);
    let secondary = resp.dragged_by(egui::PointerButton::Secondary)
        || resp.clicked_by(egui::PointerButton::Secondary);
    if let Some(p) = resp.interact_pointer_pos().filter(|_| primary || secondary) {
        let t = at(p);
        if primary {
            let mut v = value_at(p);
            if kind.bipolar() && v.abs() < 0.04 {
                v = 0.0;
            }
            let (t0, v0) = state.ctl_last.unwrap_or((t, v));
            for (pos, value) in ctl_line(t0, v0, t, v, snap) {
                edits.push(Edit::CtlSet { kind, pos, value });
            }
            state.ctl_last = Some((t, v));
        } else {
            let g = snap.max(1.0);
            let (t0, _) = state.ctl_last.unwrap_or((t, 0.0));
            let a = ((t0.min(t)) / g).floor() * g;
            let b = ((t0.max(t)) / g).floor() * g + g;
            edits.push(Edit::CtlClear { kind, a, b });
            state.ctl_last = Some((t, 0.0));
        }
    }
    if resp.drag_stopped() {
        state.ctl_last = None;
    }
}

fn kind_name(kind: CtlKind) -> &'static str {
    match kind {
        CtlKind::ModWheel => "Mod wheel",
        CtlKind::PitchBend => "Pitch bend",
        CtlKind::Aftertouch => "Aftertouch",
    }
}

/// The note properties window a double click on a note opens.
fn properties(ui: &mut Ui, notes: &[Note], state: &mut RollState, edits: &mut Vec<Edit>) {
    let Some(id) = state.props.clone() else {
        return;
    };
    let Some(n) = notes.iter().find(|n| n.id == id) else {
        state.props = None;
        return;
    };
    let mut m = n.clone();
    let mut open = true;
    let mut changed = false;
    let step = f64::from(TICKS_PER_STEP);
    egui::Window::new("Note properties")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            egui::Grid::new("roll-note-props")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let mut row = |ui: &mut Ui, name: &str, tip: &str, w: egui::DragValue| {
                        ui.label(name).on_hover_text(tip);
                        changed |= ui.add(w).on_hover_text(tip).changed();
                        ui.end_row();
                    };
                    let mut vel = m.vel * 100.0;
                    row(
                        ui,
                        "Velocity",
                        "How hard the note is played",
                        egui::DragValue::new(&mut vel)
                            .range(0.0..=100.0)
                            .suffix("%")
                            .speed(0.5),
                    );
                    m.vel = vel / 100.0;
                    let mut pan = m.pan * 100.0;
                    row(
                        ui,
                        "Pan",
                        "Left (negative) to right (positive)",
                        egui::DragValue::new(&mut pan)
                            .range(-100.0..=100.0)
                            .speed(0.5)
                            .custom_formatter(|v, _| pan_text(v / 100.0))
                            .custom_parser(|s| {
                                let t = s.trim().to_lowercase();
                                if t.starts_with('c') {
                                    return Some(0.0);
                                }
                                let num: f64 = t
                                    .trim_end_matches(['l', 'r', ' ', '%'])
                                    .trim_end_matches('%')
                                    .trim()
                                    .parse()
                                    .ok()?;
                                Some(if t.ends_with('l') { -num } else { num })
                            }),
                    );
                    m.pan = pan / 100.0;
                    let mut fine = m.fine.unwrap_or(0.0);
                    row(
                        ui,
                        "Fine pitch",
                        "Tune the note up or down, in cents (100 is a semitone)",
                        egui::DragValue::new(&mut fine)
                            .range(-FINE_RANGE..=FINE_RANGE)
                            .suffix(" cents")
                            .speed(0.5),
                    );
                    m.fine = Some(fine);
                    let mut rel = m.release.unwrap_or(0.5) * 100.0;
                    let was = rel;
                    row(
                        ui,
                        "Release",
                        "Release velocity (for instruments that use it)",
                        egui::DragValue::new(&mut rel)
                            .range(0.0..=100.0)
                            .suffix("%")
                            .speed(0.5),
                    );
                    if rel != was {
                        m.release = Some(rel / 100.0);
                    }
                    let mut start = m.pos / step + 1.0;
                    row(
                        ui,
                        "Start (step)",
                        "Where the note starts, in steps from the start of the pattern (step 1)",
                        egui::DragValue::new(&mut start)
                            .range(1.0..=100_000.0)
                            .speed(0.05)
                            .max_decimals(2),
                    );
                    m.pos = (start - 1.0) * step;
                    let mut len = m.len / step;
                    row(
                        ui,
                        "Length (steps)",
                        "How long the note is, in steps (4 steps make a beat)",
                        egui::DragValue::new(&mut len)
                            .range(0.05..=10_000.0)
                            .speed(0.05)
                            .max_decimals(2),
                    );
                    m.len = len * step;
                    row(
                        ui,
                        "Key",
                        "The note's key",
                        egui::DragValue::new(&mut m.key)
                            .range(0..=KEYS - 1)
                            .speed(0.2)
                            .custom_formatter(|v, _| note_name(v.round().clamp(0.0, 127.0) as u8)),
                    );
                });
            ui.label(
                egui::RichText::new("Drag a value or double click it to type. Ctrl+Z undoes.")
                    .weak()
                    .size(11.0),
            );
        });
    if !open {
        state.props = None;
    }
    if changed {
        if !state.props_gesture {
            edits.push(Edit::Begin);
            state.props_gesture = true;
        }
        edits.push(Edit::SetNote(Box::new(m)));
    } else if !ui.input(|i| i.pointer.any_down()) {
        state.props_gesture = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_tools_reshape_notes() {
        let n = |key, pos, len| new_note(key, pos, len);
        let mut v = vec![n(60, 0.0, 10.0), n(64, 0.0, 10.0), n(67, 48.0, 10.0)];
        note_tool(&mut v, NoteTool::Legato, &[]);
        assert_eq!((v[0].len, v[1].len, v[2].len), (48.0, 48.0, 10.0));
        note_tool(&mut v, NoteTool::Strum(6.0), &[]);
        assert_eq!((v[0].pos, v[1].pos, v[2].pos), (0.0, 6.0, 48.0));
        note_tool(&mut v, NoteTool::Flip, &[]);
        assert_eq!((v[0].key, v[1].key, v[2].key), (67, 63, 60));
        let mut r = vec![n(60, 0.0, 24.0), n(62, 72.0, 24.0)];
        note_tool(&mut r, NoteTool::Reverse, &[]);
        assert_eq!((r[0].pos, r[1].pos), (72.0, 0.0));
        let mut c = vec![n(60, 0.0, 96.0)];
        note_tool(&mut c, NoteTool::Chop(24.0), &[]);
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|x| x.len == 24.0));
        let mut s = vec![n(61, 0.0, 24.0), n(66, 24.0, 24.0)];
        note_tool(&mut s, NoteTool::ToScale(0, SCALES[0].1), &[]);
        assert_eq!((s[0].key, s[1].key), (60, 65));
        let mut h = vec![n(60, 48.0, 24.0); 8];
        let before = h.clone();
        note_tool(&mut h, NoteTool::Humanize(7), &[]);
        assert!(h
            .iter()
            .zip(&before)
            .all(|(a, b)| (a.pos - b.pos).abs() <= 3.0 && (a.vel - b.vel).abs() <= 0.08));
        assert!(h.iter().any(|a| a.pos != 48.0));
        let mut m = vec![n(60, 0.0, 24.0), n(60, 24.0, 24.0), n(60, 48.0, 24.0)];
        let only = vec![m[1].id.clone()];
        note_tool(&mut m, NoteTool::Stretch(2.0), &only);
        assert_eq!((m[0].len, m[1].len, m[2].len), (24.0, 48.0, 24.0));
        note_tool(&mut m, NoteTool::Ramp, &[]);
        assert!(m[0].vel < m[2].vel);
        let before: Vec<f64> = m.iter().map(|x| x.vel).collect();
        note_tool(&mut m, NoteTool::VelScale(0.5), &[]);
        assert!(m
            .iter()
            .zip(&before)
            .all(|(a, b)| (a.vel - (b * 0.5).max(0.02)).abs() < 1e-9));
        note_tool(&mut m, NoteTool::VelScale(10.0), &[]);
        assert!(m.iter().all(|x| x.vel <= 1.0));
        let mut r = vec![n(60, 0.0, 24.0); 16];
        note_tool(&mut r, NoteTool::VelRandom(9), &[]);
        assert!(r.iter().all(|x| (0.05..=1.0).contains(&x.vel)));
        assert!(r.iter().any(|x| (x.vel - 0.78).abs() > 1e-6));
    }

    #[test]
    fn a_stamp_draws_its_chord_and_a_plain_click_one_note() {
        assert_eq!(chord_keys(60, None), vec![60]);
        assert_eq!(chord_keys(60, Some(&[0, 4, 7])), vec![60, 64, 67]);
        // keys past the top of the keyboard are left out
        assert_eq!(chord_keys(KEYS - 2, Some(&[0, 12])), vec![KEYS - 2]);
        for (name, iv) in STAMPS.iter().chain(SCALES.iter()) {
            assert_eq!(iv[0], 0, "{name} starts on its root");
        }
    }

    fn project() -> Project {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/engines.las");
        las_project::decode(bytes).unwrap().project
    }

    #[test]
    fn notes_are_added_moved_resized_and_deleted() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        let before = p.patterns[0].notes.get(&ch).map_or(0, Vec::len);
        let n = new_note(64, 96.0, 48.0);
        let id = n.id.clone();
        apply(&mut p, 0, &ch, &Edit::Add(n));
        let notes = &p.patterns[0].notes[&ch];
        assert_eq!(notes.len(), before + 1);
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Move {
                id: id.clone(),
                pos: 120.0,
                key: 67,
            },
        );
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Resize {
                id: id.clone(),
                len: 24.0,
            },
        );
        let n = p.patterns[0].notes[&ch]
            .iter()
            .find(|n| n.id == id)
            .unwrap();
        assert_eq!((n.pos, n.key, n.len), (120.0, 67, 24.0));
        apply(&mut p, 0, &ch, &Edit::Delete { id });
        assert_eq!(p.patterns[0].notes.get(&ch).map_or(0, Vec::len), before);
    }

    #[test]
    fn an_emptied_channel_leaves_no_entry_and_bad_ids_change_nothing() {
        let mut p = project();
        let n = new_note(60, 0.0, 24.0);
        let id = n.id.clone();
        apply(&mut p, 0, "nobody", &Edit::Add(n));
        assert!(p.patterns[0].notes.contains_key("nobody"));
        apply(
            &mut p,
            0,
            "nobody",
            &Edit::Move {
                id: "other".into(),
                pos: 5.0,
                key: 1,
            },
        );
        assert_eq!(p.patterns[0].notes["nobody"][0].pos, 0.0);
        apply(&mut p, 0, "nobody", &Edit::Delete { id });
        assert!(!p.patterns[0].notes.contains_key("nobody"));
        // a pattern that does not exist is ignored
        apply(&mut p, 999, "nobody", &Edit::Add(new_note(60, 0.0, 1.0)));
    }

    #[test]
    fn groups_move_together_and_stay_on_the_grid() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let (a, b) = (new_note(60, 24.0, 24.0), new_note(64, 96.0, 24.0));
        let ids = vec![a.id.clone(), b.id.clone()];
        apply(&mut p, 0, &ch, &Edit::Add(a));
        apply(&mut p, 0, &ch, &Edit::Add(b));
        apply(
            &mut p,
            0,
            &ch,
            &Edit::MoveMany {
                ids: ids.clone(),
                dpos: 48.0,
                dkey: 2,
            },
        );
        let n = &p.patterns[0].notes[&ch];
        assert_eq!(
            (n[0].pos, n[0].key, n[1].pos, n[1].key),
            (72.0, 62.0 as i32, 144.0, 66)
        );
        // pulled left past the start, the whole group stops with its first note at 0
        apply(
            &mut p,
            0,
            &ch,
            &Edit::MoveMany {
                ids: ids.clone(),
                dpos: -1000.0,
                dkey: 0,
            },
        );
        let n = &p.patterns[0].notes[&ch];
        assert_eq!((n[0].pos, n[1].pos), (0.0, 72.0));
        // keys stay within 0 to 127
        apply(
            &mut p,
            0,
            &ch,
            &Edit::MoveMany {
                ids: ids.clone(),
                dpos: 0.0,
                dkey: 500,
            },
        );
        assert!(p.patterns[0].notes[&ch].iter().all(|n| n.key == 127));
        // delete many, and paste adds notes as they are
        apply(&mut p, 0, &ch, &Edit::DeleteMany { ids: ids.clone() });
        assert!(!p.patterns[0].notes.contains_key(&ch));
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Paste {
                notes: vec![new_note(60, 5.0, 10.0)],
            },
        );
        assert_eq!(p.patterns[0].notes[&ch].len(), 1);
    }

    #[test]
    fn velocity_is_set_and_held_between_zero_and_one() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let n = new_note(60, 0.0, 24.0);
        let id = n.id.clone();
        apply(&mut p, 0, &ch, &Edit::Add(n));
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Lane {
                id: id.clone(),
                lane: Lane::Velocity,
                value: 0.3,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].vel, 0.3);
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Lane {
                id: id.clone(),
                lane: Lane::Velocity,
                value: 7.0,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].vel, 1.0);
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Lane {
                id: "nope".into(),
                lane: Lane::Velocity,
                value: 0.1,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].vel, 1.0);
    }

    #[test]
    fn quantize_snaps_starts_and_lengths_to_the_grid() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let mut n = new_note(60, 50.0, 30.0);
        n.step = None;
        apply(&mut p, 0, &ch, &Edit::Add(n));
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Quantize {
                snap: 24.0,
                ids: Vec::new(),
                strength: 1.0,
            },
        );
        let (pos, len, id) = {
            let n = &p.patterns[0].notes[&ch][0];
            (n.pos, n.len, n.id.clone())
        };
        assert_eq!((pos, len), (48.0, 24.0));
        // a very short note never vanishes
        apply(&mut p, 0, &ch, &Edit::Resize { id, len: 1.0 });
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Quantize {
                snap: 24.0,
                ids: Vec::new(),
                strength: 1.0,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].len, 24.0);
        // half strength moves a selected start half way, and leaves the others
        let mut a = new_note(60, 60.0, 30.0);
        a.step = None;
        let a_id = a.id.clone();
        let mut b = new_note(62, 60.0, 30.0);
        b.step = None;
        apply(&mut p, 0, &ch, &Edit::Add(a));
        apply(&mut p, 0, &ch, &Edit::Add(b));
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Quantize {
                snap: 24.0,
                ids: vec![a_id.clone()],
                strength: 0.5,
            },
        );
        let ns = &p.patterns[0].notes[&ch];
        let a = ns.iter().find(|n| n.id == a_id).unwrap();
        assert_eq!((a.pos, a.len), (66.0, 30.0));
        assert!(ns.iter().any(|n| n.key == 62 && n.pos == 60.0));
    }

    #[test]
    fn positions_snap_down_to_the_grid() {
        assert_eq!(snapped(50.0, 24.0), 48.0);
        assert_eq!(snapped(47.9, 24.0), 24.0);
        assert_eq!(snapped(50.4, 1.0), 50.0);
    }

    #[test]
    fn hits_find_the_note_and_its_edge() {
        let notes = vec![new_note(60, 0.0, 96.0), new_note(62, 0.0, 96.0)];
        assert_eq!(hit(&notes, 10.0, 62, 6.0), Some((1, false)));
        assert_eq!(hit(&notes, 94.0, 60, 6.0), Some((0, true)));
        assert_eq!(hit(&notes, 10.0, 61, 6.0), None);
        assert_eq!(hit(&notes, 96.0, 60, 6.0), None);
    }

    #[test]
    fn the_roll_draws() {
        let p = project();
        let ctx = egui::Context::default();
        let mut state = RollState::default();
        let mut out = Vec::new();
        for _ in 0..2 {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                out = draw(ui, &p, 0, 0, &mut state, Some(10.0));
            });
        }
        assert!(out.is_empty(), "no mouse, no edits");
    }

    /// Run frames with the given pointer events and collect every edit.
    fn drive(p: &Project, state: &mut RollState, frames: Vec<Vec<egui::Event>>) -> Vec<Edit> {
        let ctx = egui::Context::default();
        let mut all = Vec::new();
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 600.0));
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                all.extend(draw(ui, p, 0, 0, state, None));
            });
        }
        all
    }

    fn at(x: f32, y: f32) -> egui::Pos2 {
        egui::pos2(x, y)
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn a_click_on_an_empty_cell_adds_a_note_and_a_drag_stretches_one() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let mut state = RollState::default();
        let p0 = at(GUTTER + 20.0, 300.0);
        let click = vec![
            vec![egui::Event::PointerMoved(p0)],
            vec![button(p0, true)],
            vec![button(p0, false)],
            vec![],
        ];
        let edits = drive(&p, &mut state, click);
        let added: Vec<_> = edits.iter().filter(|e| matches!(e, Edit::Add(_))).collect();
        assert_eq!(added.len(), 1, "{edits:?}");
        assert!(edits.iter().any(|e| matches!(e, Edit::Begin)));

        let mut state = RollState::default();
        let p1 = at(GUTTER + 20.0 + 100.0, 300.0);
        let drag = vec![
            vec![egui::Event::PointerMoved(p0)],
            vec![button(p0, true)],
            vec![egui::Event::PointerMoved(at(p0.x + 30.0, p0.y))],
            vec![egui::Event::PointerMoved(p1)],
            vec![button(p1, false)],
            vec![],
        ];
        let edits = drive(&p, &mut state, drag);
        assert!(
            edits.iter().any(|e| matches!(e, Edit::Add(_))),
            "a drag on empty space starts a note: {edits:?}"
        );
    }
    #[test]
    fn paint_lays_a_note_on_every_step_passed_and_skips_taken_ones() {
        let snap = 24.0;
        let first = paint_span(&[], None, 30.0, 60, snap);
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].pos, first[0].len, first[0].key), (24.0, 24.0, 60));
        // a jump of three steps fills the steps between
        let more = paint_span(&first, Some((24.0, 60)), 100.0, 60, snap);
        assert_eq!(
            more.iter().map(|n| n.pos).collect::<Vec<_>>(),
            [48.0, 72.0, 96.0]
        );
        // backwards too
        let back = paint_span(&[], Some((96.0, 60)), 30.0, 60, snap);
        assert_eq!(
            back.iter().map(|n| n.pos).collect::<Vec<_>>(),
            [24.0, 48.0, 72.0]
        );
        // the same step adds nothing; a note already there is not doubled
        assert!(paint_span(&first, Some((24.0, 60)), 40.0, 60, snap).is_empty());
        assert!(paint_span(&first, None, 30.0, 60, snap).is_empty());
        // another key starts afresh on the step under the pointer
        let other = paint_span(&first, Some((24.0, 60)), 100.0, 62, snap);
        assert_eq!(other.len(), 1);
        assert_eq!((other[0].pos, other[0].key), (96.0, 62));
    }

    #[test]
    fn ctrl_b_duplicates_the_selection_or_everything_right_after_itself() {
        let a = new_note(60, 0.0, 24.0);
        let b = new_note(64, 48.0, 30.0);
        let notes = vec![a.clone(), b.clone()];
        let all = duplicate(&notes, &HashSet::new(), 24.0);
        // 0 to 78 rounds up to 96
        assert_eq!(all.iter().map(|n| n.pos).collect::<Vec<_>>(), [96.0, 144.0]);
        assert!(all.iter().all(|n| n.id != a.id && n.id != b.id));
        let one = duplicate(&notes, &HashSet::from([b.id.clone()]), 24.0);
        assert_eq!(one.len(), 1);
        assert_eq!((one[0].pos, one[0].key, one[0].len), (96.0, 64, 30.0));
        assert!(duplicate(&[], &HashSet::new(), 24.0).is_empty());
    }

    #[test]
    fn paste_goes_after_the_selection_else_the_playhead_else_the_last_click() {
        let a = new_note(60, 0.0, 30.0);
        let notes = vec![a.clone()];
        let sel = HashSet::from([a.id.clone()]);
        assert_eq!(paste_place(&notes, &sel, Some(500.0), 7.0, 24.0), 48.0);
        assert_eq!(
            paste_place(&notes, &HashSet::new(), Some(500.0), 7.0, 24.0),
            480.0
        );
        assert_eq!(paste_place(&notes, &HashSet::new(), None, 7.0, 24.0), 7.0);
        let placed = place_clipboard(&[new_note(60, 24.0, 24.0)], 96.0);
        assert_eq!(placed[0].pos, 120.0);
    }

    #[test]
    fn the_keys_select_copy_paste_duplicate_delete_and_move() {
        let notes = vec![new_note(60, 0.0, 24.0), new_note(64, 24.0, 24.0)];
        let mut st = RollState::default();
        let mut e = Vec::new();
        let press = |f: fn(&mut Keys)| {
            let mut k = Keys::default();
            f(&mut k);
            k
        };
        keys_edit(press(|k| k.all = true), &notes, &mut st, None, &mut e);
        assert_eq!(st.selected.len(), 2);
        assert!(e.is_empty(), "selecting changes nothing");
        assert!(keys_edit(
            press(|k| k.copy = true),
            &notes,
            &mut st,
            None,
            &mut e
        ));
        assert_eq!(st.clipboard.len(), 2);
        keys_edit(press(|k| k.paste = true), &notes, &mut st, None, &mut e);
        let Some(Edit::Paste { notes: got }) = e.last() else {
            panic!("{e:?}")
        };
        // right after the selection, which ends at 48
        assert_eq!(got.iter().map(|n| n.pos).collect::<Vec<_>>(), [48.0, 72.0]);
        assert!(got.iter().all(|n| st.selected.contains(&n.id)));
        e.clear();
        st.selected.clear();
        keys_edit(press(|k| k.duplicate = true), &notes, &mut st, None, &mut e);
        assert!(matches!(&e[..], [Edit::Begin, Edit::Paste { notes }] if notes.len() == 2));
        e.clear();
        st.selected = HashSet::from([notes[0].id.clone()]);
        keys_edit(
            press(|k| {
                k.up = true;
                k.shift = true;
            }),
            &notes,
            &mut st,
            None,
            &mut e,
        );
        assert!(matches!(&e[1], Edit::MoveMany { dkey: 12, dpos, .. } if *dpos == 0.0));
        e.clear();
        keys_edit(press(|k| k.right = true), &notes, &mut st, None, &mut e);
        assert!(matches!(&e[1], Edit::MoveMany { dkey: 0, dpos, .. } if *dpos == 24.0));
        e.clear();
        keys_edit(press(|k| k.delete = true), &notes, &mut st, None, &mut e);
        assert!(matches!(&e[1], Edit::DeleteMany { ids } if ids.len() == 1));
        assert!(st.selected.is_empty());
        st.selected = HashSet::from([notes[0].id.clone()]);
        keys_edit(press(|k| k.deselect = true), &notes, &mut st, None, &mut e);
        assert!(st.selected.is_empty(), "Ctrl+D deselects, as in FL");
    }

    #[test]
    fn lanes_read_and_set_velocity_pan_fine_pitch_and_release() {
        let mut n = new_note(60, 0.0, 24.0);
        assert_eq!(Lane::Pan.get(&n), 0.5);
        assert_eq!(Lane::Fine.get(&n), 0.5);
        assert_eq!(Lane::Release.get(&n), 0.5);
        Lane::Pan.set(&mut n, 0.0);
        assert_eq!(n.pan, -1.0);
        Lane::Pan.set(&mut n, 0.51);
        assert_eq!(n.pan, 0.0, "the middle is easy to hit");
        Lane::Fine.set(&mut n, 1.0);
        assert_eq!(n.fine, Some(FINE_RANGE));
        Lane::Fine.set(&mut n, 0.5);
        assert_eq!(n.fine, None);
        Lane::Release.set(&mut n, 0.2);
        assert_eq!(n.release, Some(0.2));
        Lane::Velocity.set(&mut n, 3.0);
        assert_eq!(n.vel, 1.0);
        assert_eq!(pan_text(-0.4), "40% L");
        assert_eq!(pan_text(0.0), "Centre");
        assert!(Lane::Fine.text(&n).contains("cents"));
        for (l, name, _) in LANES {
            assert_eq!(l.name(), name);
        }
    }

    #[test]
    fn note_properties_replace_a_note_within_range() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let n = new_note(60, 0.0, 24.0);
        apply(&mut p, 0, &ch, &Edit::Add(n.clone()));
        let m = Note {
            key: 300,
            pos: -5.0,
            len: 0.0,
            vel: 0.5,
            pan: -3.0,
            fine: Some(250.0),
            release: Some(0.25),
            ..n.clone()
        };
        apply(&mut p, 0, &ch, &Edit::SetNote(Box::new(m)));
        let got = &p.patterns[0].notes[&ch][0];
        assert_eq!(
            (
                got.key,
                got.pos,
                got.len,
                got.vel,
                got.pan,
                got.fine,
                got.release
            ),
            (127, 0.0, 1.0, 0.5, -1.0, Some(FINE_RANGE), Some(0.25))
        );
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Lane {
                id: n.id.clone(),
                lane: Lane::Pan,
                value: 1.0,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].pan, 1.0);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer_still() {
        // tick under the pointer: (300 + 100 - 56) / 0.5 = 688
        let s = zoom_about(100.0, 300.0, GUTTER, 0.5, 1.0);
        assert!(((300.0 + s - GUTTER) / 1.0 - 688.0).abs() < 1e-3);
        assert_eq!(
            zoom_about(0.0, 100.0, GUTTER, 1.0, 0.1),
            0.0,
            "never past the start"
        );
    }

    fn drive_with(
        p: &Project,
        state: &mut RollState,
        frames: Vec<Vec<egui::Event>>,
        modifiers: egui::Modifiers,
    ) -> Vec<Edit> {
        let ctx = egui::Context::default();
        let mut all = Vec::new();
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 600.0));
        for mut events in frames {
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                all.extend(draw(ui, p, 0, 0, state, None));
            });
        }
        all
    }

    fn drag_frames(a: egui::Pos2, b: egui::Pos2) -> Vec<Vec<egui::Event>> {
        let mid = egui::pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        vec![
            vec![egui::Event::PointerMoved(a)],
            vec![button(a, true)],
            vec![egui::Event::PointerMoved(egui::pos2(a.x + 12.0, a.y))],
            vec![egui::Event::PointerMoved(mid)],
            vec![egui::Event::PointerMoved(b)],
            vec![button(b, false)],
            vec![],
        ]
    }

    #[test]
    fn the_paint_tool_draws_notes_along_a_drag() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let mut state = RollState {
            tool: Tool::Paint,
            ..Default::default()
        };
        // 200 points at half a point a tick is 400 ticks: many sixteenths
        let a = at(GUTTER + 4.0, 300.0);
        let edits = drive(&p, &mut state, drag_frames(a, at(a.x + 200.0, a.y)));
        let added = edits.iter().filter(|e| matches!(e, Edit::Add(_))).count();
        assert!(added >= 10, "{added}: {edits:?}");
        assert_eq!(
            edits.iter().filter(|e| matches!(e, Edit::Begin)).count(),
            1,
            "one undo step for the whole stroke"
        );
    }

    #[test]
    fn shift_drag_copies_a_note_and_a_double_click_opens_its_properties() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        let n = new_note(100, 0.0, 96.0);
        p.patterns[0].notes.insert(ch.clone(), vec![n.clone()]);
        // the view is not scrolled, so key 100 is drawn 27 rows down
        let fresh = || RollState {
            centred: Some((0, 0)),
            ..Default::default()
        };
        let mut state = fresh();
        let y = (KEYS - 1 - 100) as f32 * state.row + state.row / 2.0;
        let a = at(GUTTER + 10.0, y);
        let edits = drive_with(
            &p,
            &mut state,
            drag_frames(a, at(a.x + 100.0, a.y)),
            egui::Modifiers::SHIFT,
        );
        let copy = edits.iter().find_map(|e| match e {
            Edit::Add(c) => Some(c.clone()),
            _ => None,
        });
        let copy = copy.unwrap_or_else(|| panic!("{edits:?}"));
        assert_ne!(copy.id, n.id);
        assert!(edits
            .iter()
            .any(|e| matches!(e, Edit::Move { id, .. } if *id == copy.id)));

        let mut state = fresh();
        let frames = vec![
            vec![egui::Event::PointerMoved(a)],
            vec![button(a, true)],
            vec![button(a, false)],
            vec![button(a, true)],
            vec![button(a, false)],
            vec![],
        ];
        let got = drive(&p, &mut state, frames);
        assert_eq!(state.props, Some(n.id.clone()), "{got:?}");
        assert_eq!(state.props.as_deref(), Some(n.id.as_str()));
    }

    #[test]
    fn a_click_on_the_keyboard_plays_the_key() {
        let p = project();
        let mut state = RollState::default();
        let k = at(20.0, 300.0);
        let frames = vec![
            vec![egui::Event::PointerMoved(k)],
            vec![button(k, true)],
            vec![],
            vec![button(k, false)],
            vec![],
        ];
        let edits = drive(&p, &mut state, frames);
        assert!(edits
            .iter()
            .any(|e| matches!(e, Edit::Audition { on: true, .. })));
        assert!(edits
            .iter()
            .any(|e| matches!(e, Edit::Audition { on: false, .. })));
        assert!(
            !edits
                .iter()
                .any(|e| matches!(e, Edit::Add(_) | Edit::Begin)),
            "{edits:?}"
        );
    }
}
