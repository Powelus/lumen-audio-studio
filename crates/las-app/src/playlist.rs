// SPDX-License-Identifier: GPL-3.0-or-later
//! The playlist: the song laid out in tracks of clips (patterns, audio and
//! automation). Like the piano roll, drawing only reports what the user asked for
//! ([`PlEdit`]) and [`apply`] changes the project.

use crate::rack::channel_colour;
use crate::widgets::ACCENT;
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, song_length, ticks_per_bar};
use las_project::create::uid;
use las_project::{Clip, ClipKind, Project, PPQ};

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
    /// Start playing from here.
    Seek(f64),
}

#[derive(Clone, Debug)]
enum Drag {
    Move { id: String, grab: f64 },
    Resize { id: String },
    Seek,
}

pub struct PlaylistState {
    /// Points per tick.
    pub zoom: f32,
    pub row: f32,
    /// Snap in ticks.
    pub snap: f64,
    drag: Option<Drag>,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            zoom: 0.12,
            row: 44.0,
            snap: f64::from(PPQ) * 4.0,
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
    let h = s.trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    ACCENT
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
        PlEdit::Begin | PlEdit::Seek(_) => {}
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
    let rows = track_count(project);
    let bar = ticks_per_bar(project.time_sig);
    let ticks = song_length(project) + bar * 4.0;
    let width = HEAD + ticks as f32 * state.zoom;
    let height = RULER + rows as f32 * state.row;
    egui::ScrollArea::both()
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
                let shade = if r % 2 == 0 { 30 } else { 26 };
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(left, y), egui::vec2(width - HEAD, state.row)),
                    0.0,
                    Color32::from_gray(shade),
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
                        Stroke::new(1.0, Color32::from_gray(55)),
                    );
                    painter.text(
                        egui::pos2(x + 3.0, o.y + 4.0),
                        egui::Align2::LEFT_TOP,
                        n.to_string(),
                        egui::FontId::proportional(10.5),
                        Color32::from_gray(170),
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
                    Stroke::new(1.0, Color32::from_black_alpha(150)),
                    egui::StrokeKind::Inside,
                );
                painter.with_clip_rect(r).text(
                    r.left_top() + egui::vec2(5.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    name,
                    egui::FontId::proportional(11.5),
                    Color32::from_gray(15),
                );
            }
            // where "play song" starts, and the playhead
            let mark = |t: f64, col: Color32, w: f32| {
                painter.line_segment(
                    [egui::pos2(x_of(t), o.y), egui::pos2(x_of(t), vb)],
                    Stroke::new(w, col),
                );
            };
            mark(start, ACCENT, 1.5);
            if let Some(t) = playhead {
                mark(t, Color32::WHITE, 1.5);
            }
            // the track names stay at the left of the view
            for r in 0..rows {
                let y = top + r as f32 * state.row;
                let cell = Rect::from_min_size(egui::pos2(vl, y), egui::vec2(HEAD, state.row));
                painter.rect_filled(cell, 0.0, Color32::from_gray(38));
                painter.rect_stroke(
                    cell,
                    0.0,
                    Stroke::new(1.0, Color32::from_gray(22)),
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
                    Color32::from_gray(200),
                );
            }
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(vl, vt), egui::pos2(vr, o.y + RULER)),
                0.0,
                Color32::from_gray(20),
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
                        Color32::from_gray(170),
                    );
                    painter.line_segment(
                        [egui::pos2(x, vt + RULER - 6.0), egui::pos2(x, vt + RULER)],
                        Stroke::new(1.0, Color32::from_gray(90)),
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
                        } else if let Some((i, at_edge)) = hit(project, t, tr, edge) {
                            let c = &project.playlist.clips[i];
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
                    }
                }
            }
            if resp.drag_stopped() {
                state.drag = None;
            }
            if resp.double_clicked_by(egui::PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    if p.x > left && !in_ruler(p) {
                        let (t, tr) = (tick_at(p.x), track_at(p.y));
                        if hit(project, t, tr, edge).is_none() {
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
    edits
}

fn mark_ruler(p: &egui::Painter, x: f32, top: f32) {
    let tri = vec![
        egui::pos2(x - 5.0, top),
        egui::pos2(x + 5.0, top),
        egui::pos2(x, top + 9.0),
    ];
    p.add(egui::Shape::convex_polygon(tri, ACCENT, Stroke::NONE));
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
