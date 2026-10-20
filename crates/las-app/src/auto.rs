// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation clips, as in FL Studio: right click a fader or knob, or pick an effect
//! parameter, to make a clip that moves it; draw the curve in a window; the clip sits in
//! the playlist. The engine already plays them (checked against the web version).

use crate::theme::pal;
use eframe::egui::{self, pos2, vec2, Rect, Sense, Stroke, Ui};
use las_project::create::uid;
use las_project::{AutomationClip, AutomationPoint, Clip, ClipKind, Project, PPQ};
use serde_json::{json, Value};

/// A request to automate a parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoRequest {
    /// The saved target (`{kind, track, param, ...}`).
    pub target: Value,
    /// What the clip is called.
    pub name: String,
    /// The parameter's position now, 0 to 1: the curve starts there.
    pub start: f64,
}

pub fn mixer_target(track: usize, param: &str) -> Value {
    json!({ "kind": "mixer", "track": track, "param": param })
}

pub fn channel_target(channel_id: &str, param: &str) -> Value {
    json!({ "kind": "channel", "channelId": channel_id, "param": param })
}

pub fn master_target(param: &str) -> Value {
    json!({ "kind": "master", "param": param })
}

pub fn effect_target(track: usize, slot_id: &str, param: &str) -> Value {
    json!({ "kind": "effect", "track": track, "slotId": slot_id, "param": param })
}

/// Make an automation clip for a request and put it in the playlist at `at` ticks, on the
/// first track with room. Returns the clip's id. Four bars long, flat at the start value.
pub fn create_clip(p: &mut Project, req: &AutoRequest, at: f64) -> String {
    let length = f64::from(PPQ) * 16.0;
    let id = uid("auto_");
    let colors = ["#ffb547", "#22d3ee", "#ff5c8a", "#3ddc97", "#c26bff"];
    p.automation.push(AutomationClip {
        id: id.clone(),
        name: req.name.clone(),
        color: colors[p.automation.len() % colors.len()].into(),
        target: req.target.clone(),
        points: vec![
            AutomationPoint {
                t: 0.0,
                v: req.start,
                ..Default::default()
            },
            AutomationPoint {
                t: length,
                v: req.start,
                ..Default::default()
            },
        ],
        length,
        ..Default::default()
    });
    let rows = crate::playlist::track_count(p);
    let free = (0..rows)
        .find(|t| {
            !p.playlist
                .clips
                .iter()
                .any(|c| c.track == *t && c.start < at + length && c.start + c.length > at)
        })
        .unwrap_or(0);
    p.playlist.clips.push(Clip {
        id: uid("clip_"),
        kind: ClipKind::Automation,
        ref_id: id.clone(),
        track: free,
        start: at,
        length,
        ..Default::default()
    });
    id
}

/// A change to an automation clip's curve.
#[derive(Clone, Debug, PartialEq)]
pub enum AutoEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    Add {
        clip: String,
        t: f64,
        v: f64,
    },
    Move {
        clip: String,
        index: usize,
        t: f64,
        v: f64,
    },
    Delete {
        clip: String,
        index: usize,
    },
}

/// Carry out one edit on a project.
pub fn apply(p: &mut Project, e: &AutoEdit) {
    let find = |p: &mut Project, id: &str| p.automation.iter().position(|c| c.id == id);
    match e {
        AutoEdit::Begin => {}
        AutoEdit::Add { clip, t, v } => {
            if let Some(i) = find(p, clip) {
                let c = &mut p.automation[i];
                let t = t.clamp(0.0, c.length);
                let at = c.points.partition_point(|x| x.t <= t);
                c.points.insert(
                    at,
                    AutomationPoint {
                        t,
                        v: v.clamp(0.0, 1.0),
                        ..Default::default()
                    },
                );
            }
        }
        AutoEdit::Move { clip, index, t, v } => {
            if let Some(i) = find(p, clip) {
                let c = &mut p.automation[i];
                let n = c.points.len();
                if *index < n {
                    // a point stays between its neighbours; the ends stay at the ends
                    let lo = if *index == 0 {
                        0.0
                    } else {
                        c.points[index - 1].t
                    };
                    let hi = if *index + 1 == n {
                        c.length
                    } else {
                        c.points[index + 1].t
                    };
                    let t = if *index == 0 {
                        0.0
                    } else if *index + 1 == n {
                        c.length
                    } else {
                        t.clamp(lo, hi)
                    };
                    c.points[*index].t = t;
                    c.points[*index].v = v.clamp(0.0, 1.0);
                }
            }
        }
        AutoEdit::Delete { clip, index } => {
            if let Some(i) = find(p, clip) {
                let c = &mut p.automation[i];
                // the first and last points hold the ends of the curve
                if *index > 0 && *index + 1 < c.points.len() {
                    c.points.remove(*index);
                }
            }
        }
    }
}

/// A short description of a target, for the editor's title.
pub fn describe_target(p: &Project, target: &Value) -> String {
    let s = |k: &str| target.get(k).and_then(Value::as_str).unwrap_or("");
    match s("kind") {
        "mixer" => {
            let t = target.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let name = p.mixer.get(t).map_or("?".to_string(), |m| m.name.clone());
            format!("{name} {}", s("param"))
        }
        "master" => format!("Master {}", s("param")),
        "channel" => {
            let name = p
                .channels
                .iter()
                .find(|c| c.id == s("channelId"))
                .map_or("?", |c| c.name.as_str());
            format!("{name} {}", s("param"))
        }
        "effect" => {
            let t = target.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let slot = p
                .mixer
                .get(t)
                .into_iter()
                .flat_map(|m| m.slots.iter().flatten())
                .find(|x| x.id == s("slotId"));
            let fx = slot.map_or("?", |x| x.plugin_id.as_str());
            format!("{fx} {}", s("param"))
        }
        other => other.to_string(),
    }
}

/// What the editor remembers between frames.
#[derive(Default)]
pub struct AutoState {
    drag: Option<usize>,
}

const H: f32 = 240.0;

/// Draw the curve editor of a clip. Returns the edits asked for this frame.
pub fn draw(ui: &mut Ui, clip: &AutomationClip, state: &mut AutoState) -> Vec<AutoEdit> {
    let mut edits = Vec::new();
    let p = pal();
    let size = vec2(ui.available_width().max(300.0), H);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, p.well_deep);
    let len = clip.length.max(1.0);
    let x_of = |t: f64| rect.left() + (t / len) as f32 * rect.width();
    let y_of = |v: f64| rect.bottom() - 6.0 - (v as f32).clamp(0.0, 1.0) * (rect.height() - 12.0);
    let t_at = |x: f32| f64::from((x - rect.left()) / rect.width()) * len;
    let v_at =
        |y: f32| f64::from((rect.bottom() - 6.0 - y) / (rect.height() - 12.0)).clamp(0.0, 1.0);
    // grid: bars, and quarters of the height
    let bar = f64::from(PPQ) * 4.0;
    let mut t = 0.0;
    while t <= len {
        painter.line_segment(
            [pos2(x_of(t), rect.top()), pos2(x_of(t), rect.bottom())],
            Stroke::new(1.0, p.grid_bar),
        );
        t += bar;
    }
    for q in 1..4 {
        let y = rect.top() + rect.height() * q as f32 / 4.0;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, p.grid),
        );
    }
    // the curve
    let pts: Vec<egui::Pos2> = clip
        .points
        .iter()
        .map(|x| pos2(x_of(x.t), y_of(x.v)))
        .collect();
    painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, p.accent)));
    for (i, q) in pts.iter().enumerate() {
        let hot = state.drag == Some(i);
        painter.circle_filled(
            *q,
            if hot { 6.0 } else { 4.5 },
            if hot { p.accent_text } else { p.accent },
        );
        painter.circle_stroke(*q, 5.0, Stroke::new(1.0, p.well_deep));
    }
    let near = |pos: egui::Pos2| pts.iter().position(|q| q.distance(pos) < 9.0);
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
            edits.push(AutoEdit::Begin);
            match near(o) {
                Some(i) => state.drag = Some(i),
                None => {
                    // a new point, which is then dragged
                    let at = clip.points.partition_point(|x| x.t <= t_at(o.x));
                    edits.push(AutoEdit::Add {
                        clip: clip.id.clone(),
                        t: t_at(o.x),
                        v: v_at(o.y),
                    });
                    state.drag = Some(at);
                }
            }
        }
    } else if resp.dragged_by(egui::PointerButton::Primary) {
        if let (Some(i), Some(pos)) = (state.drag, resp.interact_pointer_pos()) {
            edits.push(AutoEdit::Move {
                clip: clip.id.clone(),
                index: i,
                t: t_at(pos.x),
                v: v_at(pos.y),
            });
        }
    } else if resp.clicked_by(egui::PointerButton::Primary) {
        if let Some(pos) = resp.interact_pointer_pos() {
            if near(pos).is_none() {
                edits.push(AutoEdit::Begin);
                edits.push(AutoEdit::Add {
                    clip: clip.id.clone(),
                    t: t_at(pos.x),
                    v: v_at(pos.y),
                });
            }
        }
    }
    if resp.drag_stopped() {
        state.drag = None;
    }
    if resp.clicked_by(egui::PointerButton::Secondary) {
        if let Some(pos) = resp.interact_pointer_pos() {
            if let Some(i) = near(pos) {
                edits.push(AutoEdit::Begin);
                edits.push(AutoEdit::Delete {
                    clip: clip.id.clone(),
                    index: i,
                });
            }
        }
    }
    let _ = Rect::NOTHING;
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use las_project::create::default_project;

    fn req(target: Value, start: f64) -> AutoRequest {
        AutoRequest {
            target,
            name: "Kick volume".into(),
            start,
        }
    }

    #[test]
    fn a_clip_is_made_flat_and_placed_on_a_free_track() {
        let mut p = default_project();
        let id = create_clip(&mut p, &req(mixer_target(1, "volume"), 0.8), 0.0);
        let c = p.automation.iter().find(|c| c.id == id).unwrap();
        assert_eq!(c.points.len(), 2);
        assert!(c.points.iter().all(|x| x.v == 0.8));
        assert_eq!(c.points[1].t, c.length);
        let (kind, first_track) = {
            let placed = p.playlist.clips.iter().find(|x| x.ref_id == id).unwrap();
            (placed.kind, placed.track)
        };
        assert_eq!(kind, ClipKind::Automation);
        assert_ne!(first_track, 0, "track 0 holds the pattern at that time");
        // a second clip at the same place goes to another track
        let id2 = create_clip(&mut p, &req(mixer_target(2, "pan"), 0.5), 0.0);
        let t2 = p
            .playlist
            .clips
            .iter()
            .find(|x| x.ref_id == id2)
            .unwrap()
            .track;
        assert!(t2 != first_track && t2 != 0);
    }

    #[test]
    fn points_are_added_in_order_and_move_between_their_neighbours() {
        let mut p = default_project();
        let id = create_clip(&mut p, &req(mixer_target(1, "volume"), 0.5), 0.0);
        let len = p.automation[0].length;
        apply(
            &mut p,
            &AutoEdit::Add {
                clip: id.clone(),
                t: len / 2.0,
                v: 1.0,
            },
        );
        apply(
            &mut p,
            &AutoEdit::Add {
                clip: id.clone(),
                t: len / 4.0,
                v: 0.2,
            },
        );
        let ts: Vec<f64> = p.automation[0].points.iter().map(|x| x.t).collect();
        assert!(ts.windows(2).all(|w| w[0] <= w[1]), "{ts:?}");
        assert_eq!(ts.len(), 4);
        // the middle points cannot pass their neighbours; the ends keep their places
        apply(
            &mut p,
            &AutoEdit::Move {
                clip: id.clone(),
                index: 1,
                t: len * 10.0,
                v: 5.0,
            },
        );
        let a = &p.automation[0].points;
        assert!(a[1].t <= a[2].t && a[1].v == 1.0);
        apply(
            &mut p,
            &AutoEdit::Move {
                clip: id.clone(),
                index: 0,
                t: 99.0,
                v: 0.1,
            },
        );
        assert_eq!(p.automation[0].points[0].t, 0.0);
        assert_eq!(p.automation[0].points[0].v, 0.1);
        // the ends cannot be deleted, the middle can
        apply(
            &mut p,
            &AutoEdit::Delete {
                clip: id.clone(),
                index: 0,
            },
        );
        apply(
            &mut p,
            &AutoEdit::Delete {
                clip: id.clone(),
                index: 3,
            },
        );
        assert_eq!(p.automation[0].points.len(), 4);
        apply(&mut p, &AutoEdit::Delete { clip: id, index: 1 });
        assert_eq!(p.automation[0].points.len(), 3);
    }

    #[test]
    fn targets_are_described_by_name() {
        let p = default_project();
        assert_eq!(
            describe_target(&p, &mixer_target(1, "volume")),
            "Kick volume"
        );
        assert_eq!(
            describe_target(&p, &master_target("volume")),
            "Master volume"
        );
        let ch = &p.channels[2];
        assert_eq!(
            describe_target(&p, &channel_target(&ch.id, "pan")),
            "Hat pan"
        );
        let slot = p.mixer[0].slots[0].as_ref().unwrap();
        assert!(describe_target(&p, &effect_target(0, &slot.id, "gain")).starts_with("las-limiter"));
    }

    #[test]
    fn a_made_clip_moves_the_fader_in_the_render() {
        // volume automation: the kick insert's fader goes to zero halfway; the second
        // half of the render is quieter than the first
        let mut p = default_project();
        p.patterns[0].length = 384.0 * 4.0;
        for c in &mut p.playlist.clips {
            c.length = 384.0 * 4.0;
        }
        let kick = p.channels[0].id.clone();
        let notes: Vec<_> = (0..16)
            .map(|s| las_project::create::step_note(s * 4, 60, 0.9))
            .collect();
        p.patterns[0].notes.clear();
        p.patterns[0].notes.insert(kick, notes);
        let id = create_clip(&mut p, &req(mixer_target(1, "volume"), 0.8), 0.0);
        let a = p.automation.iter_mut().find(|c| c.id == id).unwrap();
        a.length = 384.0 * 4.0;
        a.points = vec![
            AutomationPoint {
                t: 0.0,
                v: 0.8,
                ..Default::default()
            },
            AutomationPoint {
                t: 384.0 * 2.0,
                v: 0.8,
                ..Default::default()
            },
            AutomationPoint {
                t: 384.0 * 2.0 + 1.0,
                v: 0.0,
                ..Default::default()
            },
            AutomationPoint {
                t: 384.0 * 4.0,
                v: 0.0,
                ..Default::default()
            },
        ];
        for c in &mut p.playlist.clips {
            if c.kind == ClipKind::Automation {
                c.length = 384.0 * 4.0;
            }
        }
        let r = las_engine::render_mix_with(
            &p,
            &Default::default(),
            &las_engine::RenderOptions {
                sample_rate: 24000.0,
                tail: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let half = r.left.len() / 2;
        let peak = |a: usize, b: usize| r.left[a..b].iter().fold(0f32, |m, s| m.max(s.abs()));
        let (first, second) = (peak(0, half - 2000), peak(half + 2000, r.left.len()));
        assert!(first > 0.1, "{first}");
        assert!(
            second < first * 0.05,
            "the fader went down: {first} then {second}"
        );
    }
}
