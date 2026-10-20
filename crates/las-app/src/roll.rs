// SPDX-License-Identifier: GPL-3.0-or-later
//! The piano roll: the notes of one channel in one pattern, drawn on a grid and
//! edited with the mouse. Drawing only reports what the user asked for ([`Edit`]);
//! [`apply`] changes the project, so editing is testable without a window.

use crate::widgets::note_name;
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, ticks_per_bar};
use las_project::create::uid;
use las_project::{Note, Pattern, Project, PPQ, TICKS_PER_STEP};
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
    /// Snap every note's start and length to the grid.
    Quantize {
        snap: f64,
    },
    /// One of the Tools menu's operations, on the given notes (all notes when empty).
    Tool {
        op: NoteTool,
        ids: Vec<String>,
    },
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
}

/// The Tools menu's entries.
pub const NOTE_TOOLS: [(&str, NoteTool, &str); 9] = [
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
        }
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
    let notes = p.notes.entry(channel.to_string()).or_default();
    match edit {
        Edit::Begin | Edit::Audition { .. } => {}
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
        Edit::Velocity { id, vel } => {
            if let Some(n) = notes.iter_mut().find(|n| &n.id == id) {
                n.vel = vel.clamp(0.0, 1.0);
            }
        }
        Edit::Tool { op, ids } => note_tool(notes, *op, ids),
        Edit::Quantize { snap } => {
            let g = snap.max(1.0);
            for n in notes.iter_mut() {
                n.pos = (n.pos / g).round() * g;
                n.len = ((n.len / g).round() * g).max(g);
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

/// Keys that edit the selection while the pointer is over the roll: Delete, Ctrl+A, copy,
/// cut, paste, Ctrl+D (duplicate) and the arrows (nudge and transpose).
fn keyboard(ui: &mut Ui, area: Rect, notes: &[Note], state: &mut RollState, edits: &mut Vec<Edit>) {
    if !ui.rect_contains_pointer(area) || ui.memory(|m| m.focused().is_some()) {
        return;
    }
    let (del, all, copy, cut, paste, dup, up, down, left, right, shift) = ui.input(|i| {
        let c = i.modifiers.command;
        let k = |key| i.key_pressed(key);
        (
            k(egui::Key::Delete) || k(egui::Key::Backspace),
            c && k(egui::Key::A),
            i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
            i.events.iter().any(|e| matches!(e, egui::Event::Cut)),
            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
            c && k(egui::Key::D),
            k(egui::Key::ArrowUp),
            k(egui::Key::ArrowDown),
            k(egui::Key::ArrowLeft),
            k(egui::Key::ArrowRight),
            i.modifiers.shift,
        )
    });
    let ids: Vec<String> = notes
        .iter()
        .filter(|n| state.selected.contains(&n.id))
        .map(|n| n.id.clone())
        .collect();
    if all {
        state.selected = notes.iter().map(|n| n.id.clone()).collect();
    }
    if copy || cut {
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
    if (del || cut) && !ids.is_empty() {
        edits.push(Edit::Begin);
        edits.push(Edit::DeleteMany { ids: ids.clone() });
        state.selected.clear();
    }
    if paste && !state.clipboard.is_empty() {
        let fresh: Vec<Note> = state
            .clipboard
            .iter()
            .map(|n| Note {
                id: uid("n"),
                pos: n.pos + state.paste_at,
                ..n.clone()
            })
            .collect();
        state.selected = fresh.iter().map(|n| n.id.clone()).collect();
        edits.push(Edit::Begin);
        edits.push(Edit::Paste { notes: fresh });
    }
    if dup && !ids.is_empty() {
        let sel: Vec<&Note> = notes
            .iter()
            .filter(|n| state.selected.contains(&n.id))
            .collect();
        let start = sel.iter().map(|n| n.pos).fold(f64::MAX, f64::min);
        let end = sel.iter().map(|n| n.pos + n.len).fold(0.0, f64::max);
        let g = state.snap.max(1.0);
        let shift_by = ((end - start) / g).ceil().max(1.0) * g;
        let fresh: Vec<Note> = sel
            .iter()
            .map(|n| Note {
                id: uid("n"),
                pos: n.pos + shift_by,
                ..(*n).clone()
            })
            .collect();
        state.selected = fresh.iter().map(|n| n.id.clone()).collect();
        edits.push(Edit::Begin);
        edits.push(Edit::Paste { notes: fresh });
    }
    if !ids.is_empty() && (up || down || left || right) {
        let dkey = i32::from(up) * if shift { 12 } else { 1 }
            - i32::from(down) * if shift { 12 } else { 1 };
        let dpos = (f64::from(i32::from(right)) - f64::from(i32::from(left))) * state.snap.max(1.0);
        edits.push(Edit::Begin);
        edits.push(Edit::MoveMany { ids, dpos, dkey });
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

    // the grid runs at least as wide as the window, like FL's
    let width = (GUTTER + (ticks as f32 + bar as f32) * state.zoom).max(ui.available_width());
    let height = KEYS as f32 * state.row;
    const LANE: f32 = 74.0;
    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - LANE - 10.0).max(120.0))
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
        let y = ((KEYS - 1 - mid) as f32 * state.row - 140.0).max(0.0);
        area = area.vertical_scroll_offset(y);
        state.centred = Some((pattern, channel));
    }
    let out = area.show_viewport(ui, |ui, viewport| {
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
        let origin = resp.rect.min;
        let grid_left = origin.x + GUTTER;
        let x_of = |t: f64| grid_left + t as f32 * state.zoom;
        let y_of = |k: i32| origin.y + (KEYS - 1 - k) as f32 * state.row;
        let tick_at = |x: f32| f64::from((x - grid_left) / state.zoom);
        let key_at =
            |y: f32| (KEYS - 1 - ((y - origin.y) / state.row).floor() as i32).clamp(0, KEYS - 1);

        // rows
        let first = (viewport.min.y / state.row).floor() as i32;
        let last = ((viewport.max.y / state.row).ceil() as i32 + 1).min(KEYS);
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
                egui::vec2(width - GUTTER, state.row),
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
        let end = f64::from((width - GUTTER) / state.zoom).max(ticks + bar);
        let mut t = 0.0;
        while t <= end {
            let on_bar = (t / bar).fract().abs() < 1e-9;
            let on_beat = (t / beat).fract().abs() < 1e-9;
            if on_bar || on_beat || state.zoom > 0.9 {
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
                                y_of(n.key) + state.row - 1.0,
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
                    y_of(n.key) + state.row,
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
        // the keys, pinned to the left of the view
        let kx = origin.x + viewport.min.x;
        for r in first.max(0)..last {
            let k = KEYS - 1 - r;
            let black = matches!(k % 12, 1 | 3 | 6 | 8 | 10);
            let cell = Rect::from_min_size(egui::pos2(kx, y_of(k)), egui::vec2(GUTTER, state.row));
            painter.rect_filled(cell, 0.0, if black { pl.key_black } else { pl.key_white });
            if k % 12 == 0 || state.row >= 14.0 && !black {
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
        let edge = f64::from(EDGE / state.zoom);
        let pos = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let ptr = |p: egui::Pos2| (tick_at(p.x), key_at(p.y));
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(origin_pos) = ui.input(|i| i.pointer.press_origin()) {
                if origin_pos.x > grid_left {
                    let (t, k) = ptr(origin_pos);
                    edits.push(Edit::Begin);
                    let shift = ui.input(|i| i.modifiers.shift);
                    match hit(notes, t, k, edge) {
                        Some((i, false)) if state.tool == Tool::Select => {
                            let id = notes[i].id.clone();
                            if !state.selected.contains(&id) {
                                if !shift {
                                    state.selected.clear();
                                }
                                state.selected.insert(id);
                            }
                            state.drag = Some(Drag::Group);
                            state.group_last = Some((snapped(t, state.snap), k));
                            edits.push(Edit::Audition {
                                key: notes[i].key,
                                on: true,
                            });
                        }
                        None if state.tool == Tool::Select => {
                            if !shift {
                                state.selected.clear();
                            }
                            state.drag = Some(Drag::Marquee);
                            state.marquee = Some((origin_pos, origin_pos));
                            // nothing to undo in a selection
                            edits.pop();
                        }
                        Some((i, _)) if state.tool == Tool::Delete => {
                            edits.push(Edit::Delete {
                                id: notes[i].id.clone(),
                            });
                        }
                        Some((i, true)) => {
                            state.drag = Some(Drag::Resize {
                                id: notes[i].id.clone(),
                            });
                        }
                        Some((i, false)) => {
                            let n = &notes[i];
                            state.selected.clear();
                            state.selected.insert(n.id.clone());
                            state.drag = Some(Drag::Move {
                                id: n.id.clone(),
                                grab: t - n.pos,
                                key: n.key,
                            });
                            edits.push(Edit::Audition {
                                key: n.key,
                                on: true,
                            });
                        }
                        None if state.tool != Tool::Draw => {}
                        None => {
                            let at = snapped(t, state.snap).max(0.0);
                            let n = new_note(k, at, state.snap.max(1.0));
                            state.drag = Some(Drag::Resize { id: n.id.clone() });
                            edits.push(Edit::Audition { key: k, on: true });
                            edits.push(Edit::Add(n));
                        }
                    }
                }
            }
        } else if resp.dragged_by(egui::PointerButton::Primary) {
            if let (Some(d), Some(p)) = (state.drag.clone(), pos) {
                let (t, k) = ptr(p);
                match d {
                    Drag::Resize { id } => {
                        if let Some(n) = notes.iter().find(|n| n.id == id) {
                            let end = snapped(t, state.snap) + state.snap.max(1.0);
                            edits.push(Edit::Resize {
                                id,
                                len: (end - n.pos).max(state.snap.max(1.0)),
                            });
                        } else {
                            // the note was only just added; follow it through its first length
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
            if let Some(p) = resp.interact_pointer_pos() {
                if p.x > grid_left {
                    let (t, k) = ptr(p);
                    state.paste_at = snapped(t, state.snap).max(0.0);
                    if state.tool == Tool::Select {
                        let shift = ui.input(|i| i.modifiers.shift);
                        match hit(notes, t, k, edge) {
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
                        }
                    }
                    if state.tool == Tool::Draw && hit(notes, t, k, edge).is_none() {
                        edits.push(Edit::Begin);
                        let at = snapped(t, state.snap).max(0.0);
                        for key in chord_keys(k, state.stamp) {
                            edits.push(Edit::Add(new_note(key, at, state.len)));
                        }
                        edits.push(Edit::Audition { key: k, on: true });
                        edits.push(Edit::Audition { key: k, on: false });
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
                                egui::pos2(x_of(n.pos + n.len), y_of(n.key) + state.row),
                            );
                            if r.intersects(area) {
                                state.selected.insert(n.id.clone());
                            }
                        }
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
                None => {}
            }
        }
        if resp.clicked_by(egui::PointerButton::Secondary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let (t, k) = ptr(p);
                if let Some((i, _)) = hit(notes, t, k, edge) {
                    edits.push(Edit::Begin);
                    edits.push(Edit::Delete {
                        id: notes[i].id.clone(),
                    });
                }
            }
        }
    });
    state.scroll_x = out.state.clamped_offset().x;
    keyboard(ui, out.inner_rect, notes, state, &mut edits);
    // the velocity lane under the grid
    let (lane, lresp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), LANE),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(lane);
    painter.rect_filled(lane, 0.0, pl.well_deep);
    painter.text(
        lane.left_top() + egui::vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        "VELOCITY",
        egui::FontId::proportional(9.5),
        pl.muted,
    );
    let lx = |t: f64| lane.left() + GUTTER + t as f32 * state.zoom - state.scroll_x;
    let bar_h = LANE - 16.0;
    for n in notes {
        let x = lx(n.pos);
        if x > lane.right() || lx(n.pos + n.len) < lane.left() + GUTTER {
            continue;
        }
        let h = bar_h * n.vel.clamp(0.0, 1.0) as f32;
        let r = Rect::from_min_max(
            egui::pos2(x.max(lane.left() + GUTTER), lane.bottom() - 4.0 - h),
            egui::pos2(
                (lx(n.pos + n.len) - 1.0).max(x + 3.0).min(lane.right()),
                lane.bottom() - 4.0,
            ),
        );
        painter.rect_filled(r, 2.0, pl.accent.gamma_multiply(0.85));
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
            let tick = f64::from((p.x - lane.left() - GUTTER + state.scroll_x) / state.zoom);
            let vel = f64::from(((lane.bottom() - 4.0 - p.y) / bar_h).clamp(0.0, 1.0));
            if let Some(n) = notes
                .iter()
                .find(|n| tick >= n.pos && tick < n.pos + n.len.max(1.0))
            {
                edits.push(Edit::Velocity {
                    id: n.id.clone(),
                    vel,
                });
            }
        }
    }
    edits
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
            &Edit::Velocity {
                id: id.clone(),
                vel: 0.3,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].vel, 0.3);
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Velocity {
                id: id.clone(),
                vel: 7.0,
            },
        );
        assert_eq!(p.patterns[0].notes[&ch][0].vel, 1.0);
        apply(
            &mut p,
            0,
            &ch,
            &Edit::Velocity {
                id: "nope".into(),
                vel: 0.1,
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
        apply(&mut p, 0, &ch, &Edit::Quantize { snap: 24.0 });
        let (pos, len, id) = {
            let n = &p.patterns[0].notes[&ch][0];
            (n.pos, n.len, n.id.clone())
        };
        assert_eq!((pos, len), (48.0, 24.0));
        // a very short note never vanishes
        apply(&mut p, 0, &ch, &Edit::Resize { id, len: 1.0 });
        apply(&mut p, 0, &ch, &Edit::Quantize { snap: 24.0 });
        assert_eq!(p.patterns[0].notes[&ch][0].len, 24.0);
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
}
