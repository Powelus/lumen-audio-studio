// SPDX-License-Identifier: GPL-3.0-or-later
//! The playlist: the song laid out in tracks of clips (patterns, audio and
//! automation). Like the piano roll, drawing only reports what the user asked for
//! ([`PlEdit`]) and [`apply`] changes the project.

use crate::rack::channel_colour;
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, song_length, ticks_per_bar};
use las_project::create::uid;
use las_project::{Clip, ClipKind, Project, PPQ};
use std::collections::HashSet;

const RULER: f32 = 24.0;
const HEAD: f32 = 110.0;
const EDGE: f32 = 7.0;
/// Rows shown when the project has fewer tracks.
const MIN_TRACKS: usize = 6;

/// A change the user asked for.
#[derive(Clone, Debug)]
pub enum PlEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    Add(Box<Clip>),
    Move {
        id: String,
        start: f64,
        track: usize,
    },
    Resize {
        id: String,
        length: f64,
    },
    Delete {
        id: String,
    },
    /// Switch a clip's mute on or off.
    Mute {
        id: String,
    },
    /// Cut a clip in two at a tick.
    Split {
        id: String,
        at: f64,
    },
    /// Open the editor of an automation clip (by the clip's own id).
    OpenAuto(String),
    /// Move several clips together in time and between tracks.
    MoveMany {
        ids: Vec<String>,
        dstart: f64,
        dtrack: i32,
    },
    DeleteMany {
        ids: Vec<String>,
    },
    /// Add clips that already have their places and new ids.
    Paste {
        clips: Vec<Clip>,
    },
    /// Start playing from here.
    Seek(f64),
}

/// What a click on the playlist does, as in FL Studio's tool row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    /// Place pattern clips, move and resize clips.
    #[default]
    Draw,
    /// Move and resize clips without placing new ones.
    Select,
    /// Cut the clip that is clicked at the grid line.
    Slice,
    /// Switch a clip's mute on or off.
    Mute,
    /// Delete the clip that is clicked.
    Delete,
}

#[derive(Clone, Debug)]
enum Drag {
    Move {
        id: String,
        grab: f64,
    },
    Resize {
        id: String,
    },
    Seek,
    /// The selected clips together.
    Group,
    /// A rectangle that selects what it touches.
    Marquee,
}

pub struct PlaylistState {
    /// Points per tick.
    pub zoom: f32,
    pub row: f32,
    /// Snap in ticks.
    pub snap: f64,
    pub tool: Tool,
    /// The clips selected (by id) and the ones copied (starts relative to the first).
    pub selected: HashSet<String>,
    pub clipboard: Vec<Clip>,
    marquee: Option<(egui::Pos2, egui::Pos2)>,
    group_last: Option<(f64, usize)>,
    drag: Option<Drag>,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            zoom: 0.12,
            row: 44.0,
            snap: f64::from(PPQ) * 4.0,
            tool: Tool::Draw,
            selected: HashSet::new(),
            clipboard: Vec::new(),
            marquee: None,
            group_last: None,
            drag: None,
        }
    }
}

fn snapped(t: f64, snap: f64) -> f64 {
    if snap <= 1.0 {
        t.round()
    } else {
        (t / snap).round() * snap
    }
}

/// How many rows the playlist shows.
pub fn track_count(p: &Project) -> usize {
    let used = p
        .playlist
        .clips
        .iter()
        .map(|c| c.track + 1)
        .max()
        .unwrap_or(0);
    p.playlist.tracks.len().max(used).max(MIN_TRACKS)
}

/// The clip under a tick and track, and whether the tick is at its right edge.
fn hit(p: &Project, tick: f64, track: usize, edge_ticks: f64) -> Option<(usize, bool)> {
    p.playlist
        .clips
        .iter()
        .enumerate()
        .rev()
        .find(|(_, c)| c.track == track && tick >= c.start && tick < c.start + c.length.max(1.0))
        .map(|(i, c)| {
            (
                i,
                tick >= c.start + c.length - edge_ticks && c.length > edge_ticks * 1.5,
            )
        })
}

/// The name and colour a clip is drawn with.
pub fn describe(p: &Project, c: &Clip) -> (String, Color32) {
    match c.kind {
        ClipKind::Pattern => match p.patterns.iter().find(|x| x.id == c.ref_id) {
            Some(x) => (x.name.clone(), hex(&x.color)),
            None => ("(missing pattern)".into(), Color32::from_gray(90)),
        },
        ClipKind::Audio => match p.channels.iter().find(|x| x.id == c.ref_id) {
            Some(x) => (x.name.clone(), channel_colour(x)),
            None => ("(missing audio)".into(), Color32::from_gray(90)),
        },
        ClipKind::Automation => match p.automation.iter().find(|x| x.id == c.ref_id) {
            Some(x) => (x.name.clone(), hex(&x.color)),
            None => ("(missing automation)".into(), Color32::from_gray(90)),
        },
    }
}

fn hex(s: &str) -> Color32 {
    crate::rack::channel_colour_of(s)
}

/// A pattern clip for the pattern at `index`, starting at `start` on `track`.
pub fn new_pattern_clip(p: &Project, index: usize, start: f64, track: usize) -> Option<Clip> {
    let pat = p.patterns.get(index)?;
    Some(Clip {
        id: uid("c"),
        kind: ClipKind::Pattern,
        ref_id: pat.id.clone(),
        track,
        start,
        length: pattern_length(Some(pat), p.time_sig),
        ..Default::default()
    })
}

/// Carry out one edit on a project.
pub fn apply(project: &mut Project, edit: &PlEdit) {
    let rows = track_count(project);
    let clips = &mut project.playlist.clips;
    match edit {
        PlEdit::Begin | PlEdit::Seek(_) | PlEdit::OpenAuto(_) => {}
        PlEdit::MoveMany {
            ids,
            dstart,
            dtrack,
        } => {
            let first = clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.start)
                .fold(f64::MAX, f64::min);
            let dstart = if first == f64::MAX {
                0.0
            } else {
                dstart.max(-first)
            };
            let lowest = clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.track as i32)
                .min()
                .unwrap_or(0);
            let highest = clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.track as i32)
                .max()
                .unwrap_or(0);
            // the group keeps its shape between the first and the last track
            let dtrack = (*dtrack).clamp(-lowest, rows as i32 - 1 - highest);
            for c in clips.iter_mut().filter(|c| ids.contains(&c.id)) {
                c.start += dstart;
                c.track = (c.track as i32 + dtrack) as usize;
            }
        }
        PlEdit::DeleteMany { ids } => clips.retain(|c| !ids.contains(&c.id)),
        PlEdit::Paste { clips: add } => clips.extend(add.iter().cloned()),
        PlEdit::Add(c) => clips.push((**c).clone()),
        PlEdit::Move { id, start, track } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.start = start.max(0.0);
                c.track = (*track).min(rows - 1);
            }
        }
        PlEdit::Resize { id, length } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.length = length.max(1.0);
            }
        }
        PlEdit::Delete { id } => clips.retain(|c| &c.id != id),
        PlEdit::Mute { id } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.muted = Some(c.muted != Some(true));
            }
        }
        PlEdit::Split { id, at } => {
            let Some(i) = clips.iter().position(|c| &c.id == id) else {
                return;
            };
            let c = &clips[i];
            if *at <= c.start || *at >= c.start + c.length {
                return;
            }
            let mut right = c.clone();
            right.id = uid("c");
            right.start = *at;
            right.length = c.start + c.length - at;
            right.offset = c.offset + (at - c.start);
            clips[i].length = at - clips[i].start;
            clips.insert(i + 1, right);
        }
    }
}

/// Draw the playlist. `pattern` is the pattern a double click places; `start` is where
/// "play song" starts; `playhead` is shown while the song plays.
pub fn draw(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    start: f64,
    state: &mut PlaylistState,
    playhead: Option<f64>,
) -> Vec<PlEdit> {
    let mut edits = Vec::new();
    let pl = crate::theme::pal();
    let rows = track_count(project);
    let bar = ticks_per_bar(project.time_sig);
    let ticks = song_length(project) + bar * 4.0;
    let width = HEAD + ticks as f32 * state.zoom;
    let height = RULER + rows as f32 * state.row;
    let out = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt("playlist")
        .show_viewport(ui, |ui, viewport| {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
            let o = resp.rect.min;
            let left = o.x + HEAD;
            let x_of = |t: f64| left + t as f32 * state.zoom;
            let tick_at = |x: f32| f64::from((x - left) / state.zoom);
            let top = o.y + RULER;
            let track_at =
                |y: f32| (((y - top) / state.row).floor().max(0.0) as usize).min(rows - 1);
            let (vl, vr) = (o.x + viewport.min.x, o.x + viewport.max.x);
            let (vt, vb) = (o.y + viewport.min.y, o.y + viewport.max.y);

            // rows
            for r in 0..rows {
                let y = top + r as f32 * state.row;
                let shade = if r % 2 == 0 { pl.bg2 } else { pl.bg };
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(left, y), egui::vec2(width - HEAD, state.row)),
                    0.0,
                    shade,
                );
            }
            // bar lines and numbers
            let mut t = 0.0;
            let mut n = 1;
            while t <= ticks {
                let x = x_of(t);
                if x >= vl - 40.0 && x <= vr {
                    painter.line_segment(
                        [egui::pos2(x, o.y + RULER - 6.0), egui::pos2(x, vb)],
                        Stroke::new(1.0, pl.grid_bar),
                    );
                    painter.text(
                        egui::pos2(x + 3.0, o.y + 4.0),
                        egui::Align2::LEFT_TOP,
                        n.to_string(),
                        egui::FontId::proportional(10.5),
                        pl.muted,
                    );
                }
                t += bar;
                n += 1;
            }
            // clips
            for c in &project.playlist.clips {
                let (name, col) = describe(project, c);
                let r = Rect::from_min_max(
                    egui::pos2(x_of(c.start), top + c.track as f32 * state.row + 2.0),
                    egui::pos2(
                        (x_of(c.start + c.length) - 1.0).max(x_of(c.start) + 3.0),
                        top + (c.track + 1) as f32 * state.row - 2.0,
                    ),
                );
                if r.right() < vl || r.left() > vr {
                    continue;
                }
                let muted = c.muted == Some(true);
                let fill = col.gamma_multiply(if muted { 0.25 } else { 0.8 });
                painter.rect_filled(r, 3.0, fill);
                painter.rect_stroke(
                    r,
                    3.0,
                    if state.selected.contains(&c.id) {
                        Stroke::new(2.0, Color32::WHITE)
                    } else {
                        Stroke::new(1.0, Color32::from_black_alpha(150))
                    },
                    egui::StrokeKind::Inside,
                );
                if c.kind == ClipKind::Automation {
                    if let Some(a) = project.automation.iter().find(|a| a.id == c.ref_id) {
                        let len = a.length.max(1.0);
                        let line: Vec<egui::Pos2> = a
                            .points
                            .iter()
                            .map(|q| {
                                egui::pos2(
                                    r.left() + (q.t / len) as f32 * r.width(),
                                    r.bottom() - 4.0 - q.v as f32 * (r.height() - 18.0),
                                )
                            })
                            .collect();
                        painter.with_clip_rect(r).add(egui::Shape::line(
                            line,
                            Stroke::new(1.5, Color32::from_black_alpha(190)),
                        ));
                    }
                }
                painter.with_clip_rect(r).text(
                    r.left_top() + egui::vec2(5.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    name,
                    egui::FontId::proportional(11.5),
                    Color32::from_gray(15),
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
            // where "play song" starts, and the playhead
            let mark = |t: f64, col: Color32, w: f32| {
                painter.line_segment(
                    [egui::pos2(x_of(t), o.y), egui::pos2(x_of(t), vb)],
                    Stroke::new(w, col),
                );
            };
            mark(start, crate::theme::accent(), 1.5);
            if let Some(t) = playhead {
                mark(t, Color32::WHITE, 1.5);
            }
            // the track names stay at the left of the view
            for r in 0..rows {
                let y = top + r as f32 * state.row;
                let cell = Rect::from_min_size(egui::pos2(vl, y), egui::vec2(HEAD, state.row));
                painter.rect_filled(cell, 0.0, pl.panel2);
                painter.rect_stroke(
                    cell,
                    0.0,
                    Stroke::new(1.0, pl.border),
                    egui::StrokeKind::Inside,
                );
                let name = project
                    .playlist
                    .tracks
                    .get(r)
                    .map(|t| t.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("Track {}", r + 1));
                painter.text(
                    cell.left_center() + egui::vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(12.0),
                    pl.text2,
                );
            }
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(vl, vt), egui::pos2(vr, o.y + RULER)),
                0.0,
                pl.well_deep,
            );
            // the ruler again on top (it stays in view)
            let mut t = 0.0;
            let mut n = 1;
            while t <= ticks {
                let x = x_of(t);
                if x >= vl + HEAD - 30.0 && x <= vr {
                    painter.text(
                        egui::pos2(x + 3.0, vt + 4.0),
                        egui::Align2::LEFT_TOP,
                        n.to_string(),
                        egui::FontId::proportional(10.5),
                        pl.muted,
                    );
                    painter.line_segment(
                        [egui::pos2(x, vt + RULER - 6.0), egui::pos2(x, vt + RULER)],
                        Stroke::new(1.0, pl.border2),
                    );
                }
                t += bar;
                n += 1;
            }
            mark_ruler(&painter, x_of(start), vt);

            // the mouse
            let edge = f64::from(EDGE / state.zoom);
            let in_ruler = |p: egui::Pos2| p.y < top;
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                    if p.x > left || in_ruler(p) {
                        let (t, tr) = (tick_at(p.x), track_at(p.y));
                        if in_ruler(p) {
                            state.drag = Some(Drag::Seek);
                            edits.push(PlEdit::Seek(snapped(t, state.snap).max(0.0)));
                        } else if !matches!(state.tool, Tool::Draw | Tool::Select) {
                            // the other tools act on a click, not a drag
                        } else if let Some((i, false)) =
                            hit(project, t, tr, edge).filter(|_| state.tool == Tool::Select)
                        {
                            let id = project.playlist.clips[i].id.clone();
                            let shift = ui.input(|i| i.modifiers.shift);
                            if !state.selected.contains(&id) {
                                if !shift {
                                    state.selected.clear();
                                }
                                state.selected.insert(id);
                            }
                            edits.push(PlEdit::Begin);
                            state.drag = Some(Drag::Group);
                            state.group_last = Some((snapped(t, state.snap), tr));
                        } else if state.tool == Tool::Select && hit(project, t, tr, edge).is_none()
                        {
                            if !ui.input(|i| i.modifiers.shift) {
                                state.selected.clear();
                            }
                            state.drag = Some(Drag::Marquee);
                            state.marquee = Some((p, p));
                        } else if let Some((i, at_edge)) = hit(project, t, tr, edge) {
                            let c = &project.playlist.clips[i];
                            state.selected.clear();
                            state.selected.insert(c.id.clone());
                            edits.push(PlEdit::Begin);
                            state.drag = Some(if at_edge {
                                Drag::Resize { id: c.id.clone() }
                            } else {
                                Drag::Move {
                                    id: c.id.clone(),
                                    grab: t - c.start,
                                }
                            });
                        }
                    }
                }
            } else if resp.dragged_by(egui::PointerButton::Primary) {
                if let (Some(d), Some(p)) = (state.drag.clone(), resp.interact_pointer_pos()) {
                    let (t, tr) = (tick_at(p.x), track_at(p.y));
                    match d {
                        Drag::Seek => {
                            edits.push(PlEdit::Seek(snapped(t, state.snap).max(0.0)));
                        }
                        Drag::Resize { id } => {
                            if let Some(c) = project.playlist.clips.iter().find(|c| c.id == id) {
                                let end = snapped(t, state.snap);
                                edits.push(PlEdit::Resize {
                                    id,
                                    length: (end - c.start).max(state.snap.max(1.0)),
                                });
                            }
                        }
                        Drag::Group => {
                            let cur = (snapped(t, state.snap), tr);
                            if let Some(last) = state.group_last {
                                let (dstart, dtrack) =
                                    (cur.0 - last.0, cur.1 as i32 - last.1 as i32);
                                if dstart != 0.0 || dtrack != 0 {
                                    edits.push(PlEdit::MoveMany {
                                        ids: state.selected.iter().cloned().collect(),
                                        dstart,
                                        dtrack,
                                    });
                                    state.group_last = Some(cur);
                                }
                            }
                        }
                        Drag::Marquee => {
                            if let Some((a, _)) = state.marquee {
                                state.marquee = Some((a, p));
                            }
                        }
                        Drag::Move { id, grab } => {
                            edits.push(PlEdit::Move {
                                id,
                                start: snapped(t - grab, state.snap).max(0.0),
                                track: tr,
                            });
                        }
                    }
                }
            } else if resp.clicked_by(egui::PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    if in_ruler(p) {
                        edits.push(PlEdit::Seek(snapped(tick_at(p.x), state.snap).max(0.0)));
                    } else if p.x > left {
                        let (t, tr) = (tick_at(p.x), track_at(p.y));
                        if matches!(state.tool, Tool::Draw | Tool::Select) {
                            let shift = ui.input(|i| i.modifiers.shift);
                            match hit(project, t, tr, edge) {
                                Some((i, _)) => {
                                    let id = project.playlist.clips[i].id.clone();
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
                        if let Some((i, _)) = hit(project, t, tr, edge) {
                            let id = project.playlist.clips[i].id.clone();
                            match state.tool {
                                Tool::Mute => {
                                    edits.push(PlEdit::Begin);
                                    edits.push(PlEdit::Mute { id });
                                }
                                Tool::Delete => {
                                    edits.push(PlEdit::Begin);
                                    edits.push(PlEdit::Delete { id });
                                }
                                Tool::Slice => {
                                    edits.push(PlEdit::Begin);
                                    edits.push(PlEdit::Split {
                                        id,
                                        at: snapped(t, state.snap),
                                    });
                                }
                                Tool::Draw | Tool::Select => {}
                            }
                        }
                    }
                }
            }
            if resp.drag_stopped() {
                if let Some(Drag::Marquee) = state.drag {
                    if let Some((a, b)) = state.marquee.take() {
                        let area = Rect::from_two_pos(a, b);
                        for c in &project.playlist.clips {
                            let r = Rect::from_min_max(
                                egui::pos2(x_of(c.start), top + c.track as f32 * state.row),
                                egui::pos2(
                                    x_of(c.start + c.length),
                                    top + (c.track + 1) as f32 * state.row,
                                ),
                            );
                            if r.intersects(area) {
                                state.selected.insert(c.id.clone());
                            }
                        }
                    }
                }
                state.group_last = None;
                state.drag = None;
            }
            if state.tool == Tool::Draw && resp.double_clicked_by(egui::PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    if p.x > left && !in_ruler(p) {
                        let (t, tr) = (tick_at(p.x), track_at(p.y));
                        if let Some((i, _)) = hit(project, t, tr, edge) {
                            let c = &project.playlist.clips[i];
                            if c.kind == ClipKind::Automation {
                                edits.push(PlEdit::OpenAuto(c.ref_id.clone()));
                            }
                        } else {
                            if let Some(c) =
                                new_pattern_clip(project, pattern, snapped(t, bar).max(0.0), tr)
                            {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::Add(Box::new(c)));
                            }
                        }
                    }
                }
            }
            if resp.clicked_by(egui::PointerButton::Secondary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    if p.x > left && !in_ruler(p) {
                        if let Some((i, _)) = hit(project, tick_at(p.x), track_at(p.y), edge) {
                            edits.push(PlEdit::Begin);
                            edits.push(PlEdit::Delete {
                                id: project.playlist.clips[i].id.clone(),
                            });
                        }
                    }
                }
            }
        });
    keyboard(ui, out.inner_rect, project, start, state, &mut edits);
    edits
}

/// Keys that edit the selection while the pointer is over the playlist: Delete, Ctrl+A,
/// copy, cut, paste (at the start marker), Ctrl+D (duplicate after the selection).
fn keyboard(
    ui: &mut Ui,
    area: Rect,
    project: &Project,
    start: f64,
    state: &mut PlaylistState,
    edits: &mut Vec<PlEdit>,
) {
    if !ui.rect_contains_pointer(area) || ui.memory(|m| m.focused().is_some()) {
        return;
    }
    let (del, all, copy, cut, paste, dup) = ui.input(|i| {
        let c = i.modifiers.command;
        (
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            c && i.key_pressed(egui::Key::A),
            i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
            i.events.iter().any(|e| matches!(e, egui::Event::Cut)),
            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
            c && i.key_pressed(egui::Key::D),
        )
    });
    let clips = &project.playlist.clips;
    if all {
        state.selected = clips.iter().map(|c| c.id.clone()).collect();
    }
    let sel: Vec<&Clip> = clips
        .iter()
        .filter(|c| state.selected.contains(&c.id))
        .collect();
    let first = sel.iter().map(|c| c.start).fold(f64::MAX, f64::min);
    if copy || cut {
        state.clipboard = sel
            .iter()
            .map(|c| Clip {
                start: c.start - first,
                ..(*c).clone()
            })
            .collect();
    }
    if (del || cut) && !sel.is_empty() {
        let ids: Vec<String> = sel.iter().map(|c| c.id.clone()).collect();
        edits.push(PlEdit::Begin);
        edits.push(PlEdit::DeleteMany { ids });
        state.selected.clear();
        return;
    }
    if paste && !state.clipboard.is_empty() {
        let fresh: Vec<Clip> = state
            .clipboard
            .iter()
            .map(|c| Clip {
                id: uid("clip_"),
                start: c.start + start,
                ..c.clone()
            })
            .collect();
        state.selected = fresh.iter().map(|c| c.id.clone()).collect();
        edits.push(PlEdit::Begin);
        edits.push(PlEdit::Paste { clips: fresh });
    }
    if dup && !sel.is_empty() {
        let end = sel.iter().map(|c| c.start + c.length).fold(0.0, f64::max);
        let by = end - first;
        let fresh: Vec<Clip> = sel
            .iter()
            .map(|c| Clip {
                id: uid("clip_"),
                start: c.start + by,
                ..(*c).clone()
            })
            .collect();
        state.selected = fresh.iter().map(|c| c.id.clone()).collect();
        edits.push(PlEdit::Begin);
        edits.push(PlEdit::Paste { clips: fresh });
    }
}

fn mark_ruler(p: &egui::Painter, x: f32, top: f32) {
    let tri = vec![
        egui::pos2(x - 5.0, top),
        egui::pos2(x + 5.0, top),
        egui::pos2(x, top + 9.0),
    ];
    p.add(egui::Shape::convex_polygon(
        tri,
        crate::theme::accent(),
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/engines.las");
        las_project::decode(bytes).unwrap().project
    }

    #[test]
    fn clips_are_added_moved_resized_and_deleted() {
        let mut p = project();
        let before = p.playlist.clips.len();
        let c = new_pattern_clip(&p, 0, 384.0, 2).unwrap();
        let id = c.id.clone();
        apply(&mut p, &PlEdit::Add(Box::new(c)));
        assert_eq!(p.playlist.clips.len(), before + 1);
        apply(
            &mut p,
            &PlEdit::Move {
                id: id.clone(),
                start: 768.0,
                track: 99,
            },
        );
        apply(
            &mut p,
            &PlEdit::Resize {
                id: id.clone(),
                length: 96.0,
            },
        );
        let c = p.playlist.clips.iter().find(|c| c.id == id).unwrap();
        assert_eq!((c.start, c.length), (768.0, 96.0));
        assert!(
            c.track < track_count(&p),
            "a track past the end is held to the last row"
        );
        apply(&mut p, &PlEdit::Delete { id });
        assert_eq!(p.playlist.clips.len(), before);
    }

    #[test]
    fn clips_are_muted_and_sliced() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut c = new_pattern_clip(&p, 0, 0.0, 0).unwrap();
        c.length = 384.0;
        let id = c.id.clone();
        p.playlist.clips.push(c);
        apply(&mut p, &PlEdit::Mute { id: id.clone() });
        assert_eq!(p.playlist.clips[0].muted, Some(true));
        apply(&mut p, &PlEdit::Mute { id: id.clone() });
        assert_eq!(p.playlist.clips[0].muted, Some(false));
        apply(
            &mut p,
            &PlEdit::Split {
                id: id.clone(),
                at: 96.0,
            },
        );
        let [a, b] = &p.playlist.clips[..] else {
            panic!("{:?}", p.playlist.clips.len())
        };
        assert_eq!((a.start, a.length, a.offset), (0.0, 96.0, 0.0));
        assert_eq!((b.start, b.length, b.offset), (96.0, 288.0, 96.0));
        assert_ne!(a.id, b.id);
        // a cut outside the clip, or at its edge, does nothing
        apply(
            &mut p,
            &PlEdit::Split {
                id: id.clone(),
                at: 0.0,
            },
        );
        apply(&mut p, &PlEdit::Split { id, at: 500.0 });
        assert_eq!(p.playlist.clips.len(), 2);
    }

    #[test]
    fn groups_of_clips_move_stay_in_shape_and_paste() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut a = new_pattern_clip(&p, 0, 96.0, 1).unwrap();
        let mut b = new_pattern_clip(&p, 0, 480.0, 3).unwrap();
        a.length = 96.0;
        b.length = 96.0;
        let ids = vec![a.id.clone(), b.id.clone()];
        p.playlist.clips.push(a);
        p.playlist.clips.push(b);
        apply(
            &mut p,
            &PlEdit::MoveMany {
                ids: ids.clone(),
                dstart: 96.0,
                dtrack: 1,
            },
        );
        let c = &p.playlist.clips;
        assert_eq!(
            (c[0].start, c[0].track, c[1].start, c[1].track),
            (192.0, 2, 576.0, 4)
        );
        // never before the start, never off the tracks
        apply(
            &mut p,
            &PlEdit::MoveMany {
                ids: ids.clone(),
                dstart: -9999.0,
                dtrack: -99,
            },
        );
        let c = &p.playlist.clips;
        assert_eq!(
            (c[0].start, c[0].track, c[1].start, c[1].track),
            (0.0, 0, 384.0, 2)
        );
        apply(
            &mut p,
            &PlEdit::MoveMany {
                ids: ids.clone(),
                dstart: 0.0,
                dtrack: 999,
            },
        );
        let last = track_count(&p) - 1;
        assert_eq!(p.playlist.clips[1].track, last);
        assert_eq!(p.playlist.clips[0].track, last - 2, "the shape is kept");
        let mut copy = p.playlist.clips[0].clone();
        copy.id = "new".into();
        apply(&mut p, &PlEdit::Paste { clips: vec![copy] });
        assert_eq!(p.playlist.clips.len(), 3);
        apply(&mut p, &PlEdit::DeleteMany { ids });
        assert_eq!(p.playlist.clips.len(), 1);
    }

    #[test]
    fn hits_find_the_clip_and_its_edge() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut c = new_pattern_clip(&p, 0, 0.0, 1).unwrap();
        c.length = 384.0;
        p.playlist.clips.push(c);
        assert_eq!(hit(&p, 10.0, 1, 20.0), Some((0, false)));
        assert_eq!(hit(&p, 380.0, 1, 20.0), Some((0, true)));
        assert_eq!(hit(&p, 10.0, 0, 20.0), None);
        assert_eq!(hit(&p, 384.0, 1, 20.0), None);
    }

    #[test]
    fn every_clip_of_the_fixture_has_a_name() {
        let p = project();
        for c in &p.playlist.clips {
            let (name, _) = describe(&p, c);
            assert!(!name.starts_with('('), "{name}");
        }
        assert!(new_pattern_clip(&p, 999, 0.0, 0).is_none());
    }

    #[test]
    fn the_playlist_draws() {
        let p = project();
        let ctx = egui::Context::default();
        let mut state = PlaylistState::default();
        let mut out = Vec::new();
        for _ in 0..2 {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                out = draw(ui, &p, 0, 0.0, &mut state, Some(100.0));
            });
        }
        assert!(out.is_empty());
    }

    #[test]
    fn a_double_click_places_the_chosen_pattern_on_the_bar() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut state = PlaylistState::default();
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let at = egui::pos2(HEAD + 400.0 * 0.12 + 5.0, RULER + 44.0 * 1.5);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frames = vec![
            vec![egui::Event::PointerMoved(at)],
            vec![press(true)],
            vec![press(false)],
            vec![press(true)],
            vec![press(false)],
            vec![],
        ];
        let mut all = Vec::new();
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                all.extend(draw(ui, &p, 0, 0.0, &mut state, None));
            });
        }
        let added: Vec<_> = all
            .iter()
            .filter_map(|e| match e {
                PlEdit::Add(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(added.len(), 1, "{all:?}");
        assert_eq!(added[0].track, 1);
        assert_eq!(added[0].start % ticks_per_bar(p.time_sig), 0.0);
    }
}
