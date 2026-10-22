// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation clips, as in FL Studio: right click a fader or knob, or pick an effect
//! parameter, to make a clip that moves it; draw the curve in a window; the clip sits in
//! the playlist. The engine already plays them (checked against the web version).

use crate::theme::pal;
use eframe::egui::{self, pos2, vec2, Rect, Sense, Stroke, Ui};
use las_project::create::uid;
use las_project::{AutomationClip, AutomationPoint, Clip, ClipKind, CurveKind, Project, PPQ};
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

/// A parameter of a channel's hosted plugin (`p.<id>`).
pub fn plugin_target(channel_id: &str, param: &str) -> Value {
    json!({ "kind": "plugin", "channelId": channel_id, "param": param })
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
    /// The shape of the curve from a point to the next.
    Curve {
        clip: String,
        index: usize,
        curve: CurveKind,
    },
    /// How far the curve from a point to the next bends (-1 to 1).
    Tension {
        clip: String,
        index: usize,
        tension: f64,
    },
    /// Fill a stretch of the clip (clip ticks) with an LFO shape, replacing the points
    /// there, as FL's LFO tool.
    Lfo {
        clip: String,
        from: f64,
        to: f64,
        lfo: Lfo,
    },
}

/// The shapes of the LFO tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Square,
    Saw,
}

impl LfoShape {
    pub const ALL: [LfoShape; 4] = [Self::Sine, Self::Triangle, Self::Square, Self::Saw];

    pub fn name(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Triangle => "Triangle",
            Self::Square => "Square",
            Self::Saw => "Saw",
        }
    }
}

/// The settings of the LFO tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lfo {
    pub shape: LfoShape,
    /// The length of one cycle, ticks.
    pub period: f64,
    /// From the low to the high value, 0 to 1.
    pub depth: f64,
    /// The middle value, 0 to 1.
    pub centre: f64,
}

impl Default for Lfo {
    fn default() -> Self {
        Self {
            shape: LfoShape::Sine,
            period: f64::from(PPQ),
            depth: 0.6,
            centre: 0.5,
        }
    }
}

/// The points an LFO shape makes from clip tick `from` to `to`, curves set so that the
/// clip plays the shape exactly (a sine is made of smooth halves).
pub fn lfo_points(from: f64, to: f64, lfo: &Lfo) -> Vec<AutomationPoint> {
    let lo = (lfo.centre - lfo.depth / 2.0).clamp(0.0, 1.0);
    let hi = (lfo.centre + lfo.depth / 2.0).clamp(0.0, 1.0);
    let period = lfo.period.max(2.0);
    let pt = |t: f64, v: f64, curve: CurveKind| AutomationPoint {
        t,
        v,
        curve: (curve != CurveKind::Single).then_some(curve),
        ..Default::default()
    };
    let mut out = Vec::new();
    if to <= from {
        return out;
    }
    let half = period / 2.0;
    let mut t = from;
    let mut k = 0u64;
    // never more than a few thousand points, whatever the settings
    while t < to + period && out.len() < 4000 {
        match lfo.shape {
            LfoShape::Sine => {
                // halves of a cosine from the bottom to the top and back
                out.push(pt(t, if k % 2 == 0 { lo } else { hi }, CurveKind::Smooth));
                t += half;
            }
            LfoShape::Triangle => {
                out.push(pt(t, if k % 2 == 0 { lo } else { hi }, CurveKind::Single));
                t += half;
            }
            LfoShape::Square => {
                out.push(pt(t, if k % 2 == 0 { hi } else { lo }, CurveKind::Hold));
                t += half;
            }
            LfoShape::Saw => {
                // a ramp up, then a drop at the end of each cycle
                out.push(pt(t, lo, CurveKind::Single));
                let top = t + period - 1.0;
                if top > t {
                    out.push(pt(top, hi, CurveKind::Hold));
                }
                t += period;
            }
        }
        k += 1;
    }
    // the shape is made past the end, then cut there, at the value it has there
    let probe = AutomationClip {
        points: out.clone(),
        length: to,
        ..Default::default()
    };
    let end = las_engine::timeline::automation_value_at(&probe, to);
    out.retain(|p| p.t < to - 1e-6);
    out.push(pt(to, end, CurveKind::Single));
    out
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
        AutoEdit::Curve { clip, index, curve } => {
            if let Some(q) = find(p, clip).and_then(|i| p.automation[i].points.get_mut(*index)) {
                q.curve = (*curve != CurveKind::Single).then_some(*curve);
                if *curve == CurveKind::Hold || *curve == CurveKind::Smooth {
                    q.tension = None;
                }
            }
        }
        AutoEdit::Tension {
            clip,
            index,
            tension,
        } => {
            if let Some(q) = find(p, clip).and_then(|i| p.automation[i].points.get_mut(*index)) {
                let t = tension.clamp(-1.0, 1.0);
                q.tension = (t.abs() > 1e-3).then_some(t);
            }
        }
        AutoEdit::Lfo {
            clip,
            from,
            to,
            lfo,
        } => {
            if let Some(i) = find(p, clip) {
                let c = &mut p.automation[i];
                let (from, to) = (from.clamp(0.0, c.length), to.clamp(0.0, c.length));
                if to - from < 2.0 {
                    return;
                }
                let fresh = lfo_points(from, to, lfo);
                c.points.retain(|q| q.t < from || q.t > to);
                let at = c.points.partition_point(|q| q.t < from);
                c.points.splice(at..at, fresh);
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
        "channel" | "plugin" => {
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
            let fx = slot.map_or("?".to_string(), |x| {
                if x.plugin_id == las_host::HOSTED {
                    crate::hosted::name_of(&x.state)
                } else {
                    x.plugin_id.clone()
                }
            });
            format!("{fx} {}", s("param"))
        }
        other => other.to_string(),
    }
}

/// What the editor remembers between frames.
#[derive(Default)]
pub struct AutoState {
    drag: Option<usize>,
    /// The segment (by its first point) whose tension is being dragged, and its tension
    /// when the drag began.
    bend: Option<(usize, f64)>,
    /// The point whose menu is open.
    menu: Option<usize>,
    /// The LFO tool is on: a drag picks the stretch it fills.
    pub lfo_on: bool,
    pub lfo: Lfo,
    /// The stretch the LFO tool fills (clip ticks); the whole clip when none.
    pub lfo_range: Option<(f64, f64)>,
    range_from: Option<f64>,
}

const H: f32 = 240.0;

/// The settings row of the LFO tool. Returns the edits asked for.
pub fn lfo_row(ui: &mut Ui, clip: &AutomationClip, state: &mut AutoState) -> Vec<AutoEdit> {
    let mut edits = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.toggle_value(&mut state.lfo_on, "LFO tool")
            .on_hover_text(
                "Fill the clip, or a stretch of it you drag across, with a repeating shape",
            );
        if !state.lfo_on {
            return;
        }
        let l = &mut state.lfo;
        egui::ComboBox::from_id_salt("lfo-shape")
            .selected_text(l.shape.name())
            .show_ui(ui, |ui| {
                for s in LfoShape::ALL {
                    ui.selectable_value(&mut l.shape, s, s.name());
                }
            });
        ui.label("Speed");
        let beat = f64::from(PPQ);
        let speeds = [
            (beat / 4.0, "1/16"),
            (beat / 2.0, "1/8"),
            (beat, "1 beat"),
            (beat * 2.0, "2 beats"),
            (beat * 4.0, "1 bar"),
            (beat * 8.0, "2 bars"),
            (beat * 16.0, "4 bars"),
        ];
        let shown = speeds
            .iter()
            .find(|s| (s.0 - l.period).abs() < 1e-6)
            .map_or("?", |s| s.1);
        egui::ComboBox::from_id_salt("lfo-speed")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (v, n) in speeds {
                    ui.selectable_value(&mut l.period, v, n);
                }
            })
            .response
            .on_hover_text("How long one cycle lasts");
        ui.label("Depth");
        ui.add(egui::Slider::new(&mut l.depth, 0.0..=1.0).show_value(false));
        ui.label("Centre");
        ui.add(egui::Slider::new(&mut l.centre, 0.0..=1.0).show_value(false));
        let (from, to) = state.lfo_range.unwrap_or((0.0, clip.length));
        let what = if state.lfo_range.is_some() {
            "Fill the stretch"
        } else {
            "Fill the clip"
        };
        if ui
            .button(what)
            .on_hover_text("Replace the points there with the shape")
            .clicked()
        {
            edits.push(AutoEdit::Begin);
            edits.push(AutoEdit::Lfo {
                clip: clip.id.clone(),
                from,
                to,
                lfo: state.lfo,
            });
        }
        if state.lfo_range.is_some() && ui.button("Whole clip").clicked() {
            state.lfo_range = None;
        }
    });
    edits
}

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
    let t_at = |x: f32| (f64::from((x - rect.left()) / rect.width()) * len).clamp(0.0, len);
    let v_at =
        |y: f32| f64::from((rect.bottom() - 6.0 - y) / (rect.height() - 12.0)).clamp(0.0, 1.0);
    // grid: beats and bars, and quarters of the height
    let beat = f64::from(PPQ);
    let mut t = 0.0;
    let mut n = 0;
    while t <= len {
        let bar = n % 4 == 0;
        painter.line_segment(
            [pos2(x_of(t), rect.top()), pos2(x_of(t), rect.bottom())],
            Stroke::new(1.0, if bar { p.grid_bar } else { p.grid }),
        );
        t += beat;
        n += 1;
    }
    for q in 1..4 {
        let y = rect.top() + rect.height() * q as f32 / 4.0;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, p.grid),
        );
    }
    // the stretch the LFO tool fills
    if state.lfo_on {
        let (a, b) = state.lfo_range.unwrap_or((0.0, len));
        painter.rect_filled(
            Rect::from_x_y_ranges(x_of(a)..=x_of(b), rect.y_range()),
            0.0,
            p.accent.gamma_multiply(0.12),
        );
    }
    // the curve, as it plays, filled below like FL's
    let line: Vec<egui::Pos2> = las_engine::timeline::automation_curve(clip)
        .into_iter()
        .map(|(t, v)| pos2(x_of(t), y_of(v)))
        .collect();
    if line.len() > 1 {
        for w in line.windows(2) {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    w[0],
                    w[1],
                    pos2(w[1].x, rect.bottom()),
                    pos2(w[0].x, rect.bottom()),
                ],
                p.accent.gamma_multiply(0.14),
                Stroke::NONE,
            ));
        }
    }
    painter.add(egui::Shape::line(line, Stroke::new(2.0, p.accent)));
    let pts: Vec<egui::Pos2> = clip
        .points
        .iter()
        .map(|x| pos2(x_of(x.t), y_of(x.v)))
        .collect();
    // the tension handles: small rings halfway along the bendable segments
    let handles: Vec<(usize, egui::Pos2)> = clip
        .points
        .windows(2)
        .enumerate()
        .filter(|(_, w)| {
            let k = w[0].curve.unwrap_or_default();
            !matches!(k, CurveKind::Hold | CurveKind::Smooth) && x_of(w[1].t) - x_of(w[0].t) > 18.0
        })
        .map(|(i, w)| {
            let mid = (w[0].t + w[1].t) / 2.0;
            let v = las_engine::timeline::automation_value_at(clip, mid);
            (i, pos2(x_of(mid), y_of(v)))
        })
        .collect();
    for (i, h) in &handles {
        let hot = state.bend.is_some_and(|b| b.0 == *i);
        painter.circle_stroke(
            *h,
            if hot { 5.0 } else { 3.5 },
            Stroke::new(1.5, p.accent_text),
        );
    }
    for (i, q) in pts.iter().enumerate() {
        let hot = state.drag == Some(i) || state.menu == Some(i);
        painter.circle_filled(
            *q,
            if hot { 6.0 } else { 4.5 },
            if hot { p.accent_text } else { p.accent },
        );
        painter.circle_stroke(*q, 5.0, Stroke::new(1.0, p.well_deep));
    }
    let near = |pos: egui::Pos2| pts.iter().position(|q| q.distance(pos) < 9.0);
    let near_handle = |pos: egui::Pos2| {
        handles
            .iter()
            .find(|(_, h)| h.distance(pos) < 8.0)
            .map(|(i, _)| *i)
    };
    if let Some(h) = resp.hover_pos() {
        if near(h).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if near_handle(h).is_some() || state.bend.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
            if state.lfo_on && near(o).is_none() && near_handle(o).is_none() {
                state.range_from = Some(t_at(o.x));
            } else if let Some(i) = near(o) {
                edits.push(AutoEdit::Begin);
                state.drag = Some(i);
            } else if let Some(i) = near_handle(o) {
                edits.push(AutoEdit::Begin);
                state.bend = Some((i, clip.points[i].tension.unwrap_or(0.0)));
            } else {
                // a new point, which is then dragged
                edits.push(AutoEdit::Begin);
                let at = clip.points.partition_point(|x| x.t <= t_at(o.x));
                edits.push(AutoEdit::Add {
                    clip: clip.id.clone(),
                    t: t_at(o.x),
                    v: v_at(o.y),
                });
                state.drag = Some(at);
            }
        }
    } else if resp.dragged_by(egui::PointerButton::Primary) {
        if let Some(pos) = resp.interact_pointer_pos() {
            if let Some(a) = state.range_from {
                let b = t_at(pos.x);
                state.lfo_range = Some((a.min(b), a.max(b))).filter(|r| r.1 - r.0 > 1.0);
            } else if let Some(i) = state.drag {
                edits.push(AutoEdit::Move {
                    clip: clip.id.clone(),
                    index: i,
                    t: t_at(pos.x),
                    v: v_at(pos.y),
                });
            } else if let (Some((i, t0)), Some(o)) =
                (state.bend, ui.input(|i| i.pointer.press_origin()))
            {
                // up bends the curve toward its start value for a rise, as FL's
                let rising = clip
                    .points
                    .get(i + 1)
                    .is_some_and(|b| b.v >= clip.points[i].v);
                let d = f64::from(pos.y - o.y) / 120.0;
                edits.push(AutoEdit::Tension {
                    clip: clip.id.clone(),
                    index: i,
                    tension: t0 + if rising { d } else { -d },
                });
            }
        }
    } else if resp.clicked_by(egui::PointerButton::Primary) && !state.lfo_on {
        if let Some(pos) = resp.interact_pointer_pos() {
            if near(pos).is_none() && near_handle(pos).is_none() {
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
        state.bend = None;
        state.range_from = None;
    }
    // a double click on a tension handle straightens the curve
    if resp.double_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(near_handle) {
            edits.push(AutoEdit::Begin);
            edits.push(AutoEdit::Tension {
                clip: clip.id.clone(),
                index: i,
                tension: 0.0,
            });
        }
    }
    if resp.secondary_clicked() {
        state.menu = resp.interact_pointer_pos().and_then(near);
    }
    // a right click on a point: its curve type, or delete it (FL's point menu)
    resp.context_menu(|ui| {
        let Some(i) = state.menu.filter(|i| *i < clip.points.len()) else {
            ui.label(egui::RichText::new("Right click a point for its curve").weak());
            return;
        };
        let cur = clip.points[i].curve.unwrap_or_default();
        if i + 1 < clip.points.len() {
            ui.label(egui::RichText::new("Curve to the next point").strong());
            for k in CurveKind::ALL {
                if ui.radio(cur == k, k.name()).clicked() {
                    edits.push(AutoEdit::Begin);
                    edits.push(AutoEdit::Curve {
                        clip: clip.id.clone(),
                        index: i,
                        curve: k,
                    });
                    ui.close();
                }
            }
            if clip.points[i].tension.is_some() && ui.button("Straighten (no tension)").clicked() {
                edits.push(AutoEdit::Begin);
                edits.push(AutoEdit::Tension {
                    clip: clip.id.clone(),
                    index: i,
                    tension: 0.0,
                });
                ui.close();
            }
            ui.separator();
        }
        let ends = i == 0 || i + 1 == clip.points.len();
        if ui
            .add_enabled(!ends, egui::Button::new("Delete point"))
            .on_disabled_hover_text("The first and last points hold the ends of the clip")
            .clicked()
        {
            edits.push(AutoEdit::Begin);
            edits.push(AutoEdit::Delete {
                clip: clip.id.clone(),
                index: i,
            });
            ui.close();
        }
    });
    if !resp.context_menu_opened() {
        state.menu = None;
    }
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
    fn a_point_takes_a_curve_and_a_tension() {
        let mut p = default_project();
        let id = create_clip(&mut p, &req(mixer_target(1, "volume"), 0.5), 0.0);
        let curve = |p: &Project| {
            (
                p.automation[0].points[0].curve,
                p.automation[0].points[0].tension,
            )
        };
        apply(
            &mut p,
            &AutoEdit::Tension {
                clip: id.clone(),
                index: 0,
                tension: 3.0,
            },
        );
        assert_eq!(curve(&p), (None, Some(1.0)), "held to -1 to 1");
        apply(
            &mut p,
            &AutoEdit::Curve {
                clip: id.clone(),
                index: 0,
                curve: CurveKind::Stairs,
            },
        );
        assert_eq!(curve(&p), (Some(CurveKind::Stairs), Some(1.0)));
        apply(
            &mut p,
            &AutoEdit::Curve {
                clip: id.clone(),
                index: 0,
                curve: CurveKind::Hold,
            },
        );
        assert_eq!(
            curve(&p),
            (Some(CurveKind::Hold), None),
            "a hold has no tension"
        );
        apply(
            &mut p,
            &AutoEdit::Curve {
                clip: id,
                index: 0,
                curve: CurveKind::Single,
            },
        );
        assert_eq!(curve(&p), (None, None), "single is the default, not saved");
    }

    #[test]
    fn the_lfo_tool_fills_a_stretch_with_its_shape() {
        let beat = f64::from(PPQ);
        for shape in LfoShape::ALL {
            let lfo = Lfo {
                shape,
                period: beat,
                depth: 0.6,
                centre: 0.5,
            };
            let pts = lfo_points(0.0, beat * 4.0, &lfo);
            let c = AutomationClip {
                points: pts.clone(),
                length: beat * 4.0,
                ..Default::default()
            };
            assert!(pts.windows(2).all(|w| w[0].t <= w[1].t), "{shape:?}");
            assert_eq!(pts.last().unwrap().t, beat * 4.0);
            // it swings between 0.2 and 0.8, once a beat
            let at = |t: f64| las_engine::timeline::automation_value_at(&c, t);
            let vals: Vec<f64> = (0..400)
                .map(|i| at(f64::from(i) * beat * 4.0 / 400.0))
                .collect();
            let (lo, hi) = vals
                .iter()
                .fold((1.0f64, 0.0f64), |(a, b), v| (a.min(*v), b.max(*v)));
            assert!(
                (lo - 0.2).abs() < 0.02 && (hi - 0.8).abs() < 0.02,
                "{shape:?}: {lo} {hi}"
            );
            for k in 0..3 {
                let t = f64::from(k) * beat + beat * 0.3;
                assert!((at(t) - at(t + beat)).abs() < 0.02, "{shape:?} repeats");
            }
        }
        // a sine is a sine
        let lfo = Lfo::default();
        let c = AutomationClip {
            points: lfo_points(0.0, beat * 2.0, &lfo),
            length: beat * 2.0,
            ..Default::default()
        };
        for i in 0..50 {
            let t = f64::from(i) * beat * 2.0 / 50.0;
            let want = 0.5 - 0.3 * (std::f64::consts::TAU * t / beat).cos();
            let got = las_engine::timeline::automation_value_at(&c, t);
            assert!((got - want).abs() < 1e-6, "at {t}: {got} vs {want}");
        }
        // applied to a clip it replaces only the points in the stretch
        let mut p = default_project();
        let id = create_clip(&mut p, &req(mixer_target(1, "volume"), 0.9), 0.0);
        let len = p.automation[0].length;
        apply(
            &mut p,
            &AutoEdit::Lfo {
                clip: id,
                from: len / 4.0,
                to: len / 2.0,
                lfo,
            },
        );
        let a = &p.automation[0];
        assert_eq!((a.points[0].t, a.points[0].v), (0.0, 0.9));
        assert_eq!(a.points.last().unwrap().t, len);
        assert!(a.points.windows(2).all(|w| w[0].t <= w[1].t));
        assert!(a.points.len() > 8);
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
