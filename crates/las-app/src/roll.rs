// SPDX-License-Identifier: GPL-3.0-or-later
//! The piano roll: the notes of one channel in one pattern, drawn on a grid and
//! edited with the mouse. Drawing only reports what the user asked for ([`Edit`]);
//! [`apply`] changes the project, so editing is testable without a window.

use crate::widgets::note_name;
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, ticks_per_bar};
use las_project::create::uid;
use las_project::{Note, Pattern, Project, PPQ, TICKS_PER_STEP};

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
    /// Sound a key (a note being placed or moved) or let go of it.
    Audition {
        key: i32,
        on: bool,
    },
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
    drag: Option<Drag>,
    /// The pattern and channel the view was last centred for.
    centred: Option<(usize, usize)>,
}

impl Default for RollState {
    fn default() -> Self {
        Self {
            zoom: 0.5,
            row: 14.0,
            snap: f64::from(TICKS_PER_STEP),
            len: f64::from(TICKS_PER_STEP) * 4.0,
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

    let width = GUTTER + (ticks as f32 + bar as f32) * state.zoom;
    let height = KEYS as f32 * state.row;
    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
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
    area.show_viewport(ui, |ui, viewport| {
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
            let shade = if black { pl.roll_row_b } else { pl.roll_row_a };
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
        let end = ticks + bar;
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
            painter.rect_stroke(
                r,
                2.0,
                Stroke::new(1.0, Color32::from_black_alpha(140)),
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
                    match hit(notes, t, k, edge) {
                        Some((i, true)) => {
                            state.drag = Some(Drag::Resize {
                                id: notes[i].id.clone(),
                            });
                        }
                        Some((i, false)) => {
                            let n = &notes[i];
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
                    if hit(notes, t, k, edge).is_none() {
                        edits.push(Edit::Begin);
                        let at = snapped(t, state.snap).max(0.0);
                        edits.push(Edit::Add(new_note(k, at, state.len)));
                        edits.push(Edit::Audition { key: k, on: true });
                        edits.push(Edit::Audition { key: k, on: false });
                    }
                }
            }
        }
        if resp.drag_stopped() {
            match state.drag.take() {
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
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

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
