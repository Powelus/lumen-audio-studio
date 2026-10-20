// SPDX-License-Identifier: GPL-3.0-or-later
//! The channel rack as a step sequencer, like the web version's: one row per channel
//! with its mute light, level and pan knobs, name, and a cell for every step of the
//! pattern. Like the other editors, drawing reports what was asked for.

use crate::rack::channel_colour;
use crate::roll::{new_note, Edit};
use crate::theme::pal;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, Ui};
use las_engine::timeline::pattern_length;
use las_project::{Note, Pattern, Project, TICKS_PER_STEP};

const STEP: f32 = 24.0;
const ROW: f32 = 33.0;
const NAME: f32 = 130.0;

/// A change to a channel's settings (level, pan, mute).
#[derive(Clone, Debug, PartialEq)]
pub enum ChanEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    Volume {
        channel: usize,
        value: f64,
    },
    Pan {
        channel: usize,
        value: f64,
    },
    Mute {
        channel: usize,
        on: bool,
    },
    /// Make an automation clip for a parameter of a channel.
    Automate(crate::auto::AutoRequest),
    /// Remove a channel, with its notes in every pattern.
    Delete(usize),
    /// A copy of a channel (with its notes) right after it.
    Clone(usize),
    /// Play the channel through another insert.
    Route {
        channel: usize,
        insert: usize,
    },
    Color {
        channel: usize,
        color: String,
    },
    /// The channel's own swing (0 to 1), or none to follow the project's.
    Swing {
        channel: usize,
        value: Option<f64>,
    },
    /// Ask for a new name for a channel.
    AskRename(usize),
}

/// Carry out a channel edit.
pub fn apply(project: &mut Project, e: &ChanEdit) {
    match e {
        ChanEdit::Begin => {}
        ChanEdit::Volume { channel, value } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.volume = value.clamp(0.0, 1.25);
            }
        }
        ChanEdit::Pan { channel, value } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.pan = value.clamp(-1.0, 1.0);
            }
        }
        ChanEdit::Mute { channel, on } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.mute = *on;
            }
        }
        ChanEdit::Automate(_) | ChanEdit::AskRename(_) => {}
        ChanEdit::Delete(i) => {
            if *i < project.channels.len() {
                let id = project.channels.remove(*i).id;
                for pat in &mut project.patterns {
                    pat.notes.remove(&id);
                }
            }
        }
        ChanEdit::Clone(i) => {
            if let Some(src) = project.channels.get(*i).cloned() {
                let mut c = src.clone();
                c.id = las_project::create::uid("ch_");
                c.name = format!("{} copy", src.name);
                for pat in &mut project.patterns {
                    if let Some(notes) = pat.notes.get(&src.id).cloned() {
                        let copy = notes
                            .into_iter()
                            .map(|mut n| {
                                n.id = las_project::create::uid("n");
                                n
                            })
                            .collect();
                        pat.notes.insert(c.id.clone(), copy);
                    }
                }
                project.channels.insert(*i + 1, c);
            }
        }
        ChanEdit::Route { channel, insert } => {
            if *insert < project.mixer.len() {
                if let Some(c) = project.channels.get_mut(*channel) {
                    c.mixer_track = *insert;
                }
            }
        }
        ChanEdit::Color { channel, color } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.color = color.clone();
            }
        }
        ChanEdit::Swing { channel, value } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.swing = value.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0));
            }
        }
    }
}

/// What the rack shows and does between frames.
#[derive(Default)]
pub struct StepState {
    /// While the mouse paints across cells: whether it adds (`true`) or removes.
    painting: Option<bool>,
}

/// The step note of a channel at a step, if there is one.
fn step_note(notes: &[Note], step: usize) -> Option<&Note> {
    let pos = step as f64 * f64::from(TICKS_PER_STEP);
    notes
        .iter()
        .find(|n| n.step == Some(true) && (n.pos - pos).abs() < 0.5)
}

/// The edit that toggles a step: remove the note there, or place one.
pub fn toggle(notes: &[Note], step: usize) -> Edit {
    match step_note(notes, step) {
        Some(n) => Edit::Delete { id: n.id.clone() },
        None => {
            let mut n = new_note(
                60,
                step as f64 * f64::from(TICKS_PER_STEP),
                f64::from(TICKS_PER_STEP),
            );
            n.step = Some(true);
            Edit::Add(n)
        }
    }
}

/// A small rotary knob: drag up and down, double-click to reset. Returns
/// (changed, gesture started).
pub fn mini_knob(
    ui: &mut Ui,
    v: &mut f64,
    range: (f64, f64),
    default: f64,
    tip: &str,
    colour: Color32,
) -> (bool, bool) {
    let (changed, started, _) = mini_knob_resp(ui, v, range, default, tip, colour);
    (changed, started)
}

/// As [`mini_knob`], also giving the knob's response (for a context menu).
pub fn mini_knob_resp(
    ui: &mut Ui,
    v: &mut f64,
    range: (f64, f64),
    default: f64,
    tip: &str,
    colour: Color32,
) -> (bool, bool, egui::Response) {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click_and_drag());
    let mut changed = false;
    if resp.dragged() {
        let d = -f64::from(resp.drag_delta().y) / 110.0 * (range.1 - range.0);
        let before = *v;
        *v = (*v + d).clamp(range.0, range.1);
        changed = *v != before;
    }
    let reset = resp.double_clicked();
    if reset {
        *v = default;
        changed = true;
    }
    let p = pal();
    let painter = ui.painter_at(r);
    let c = r.center();
    painter.circle_filled(c, 10.0, p.well_deep);
    painter.circle_stroke(c, 10.0, Stroke::new(1.0, p.border2));
    let t = ((*v - range.0) / (range.1 - range.0)).clamp(0.0, 1.0) as f32;
    let ang = (-135.0 + 270.0 * t).to_radians();
    let tip_pt = pos2(c.x + ang.sin() * 7.5, c.y - ang.cos() * 7.5);
    painter.line_segment([c, tip_pt], Stroke::new(2.0, colour));
    let resp = resp.on_hover_text(format!("{tip}: {:.0}%", *v * 100.0));
    (changed, resp.drag_started() || reset, resp)
}

/// What the rack asks for in one frame.
#[derive(Default)]
pub struct Out {
    /// Note edits, with the channel each belongs to.
    pub notes: Vec<(usize, Edit)>,
    pub chans: Vec<ChanEdit>,
    /// A channel whose name was right-clicked: open its piano roll.
    pub open_roll: Option<usize>,
    /// A channel whose name was double-clicked: open its instrument.
    pub open_synth: Option<usize>,
}

pub fn draw(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    selected: &mut usize,
    state: &mut StepState,
    playhead: Option<f64>,
) -> Out {
    let mut out = Out::default();
    let p = pal();
    let pat: Option<&Pattern> = project.patterns.get(pattern);
    let ticks = pattern_length(pat, project.time_sig);
    let steps = ((ticks / f64::from(TICKS_PER_STEP)).round() as usize).clamp(16, 256);
    let empty: Vec<Note> = Vec::new();
    egui::ScrollArea::horizontal()
        .auto_shrink([false, true])
        .id_salt("steps")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            for (i, c) in project.channels.iter().enumerate() {
                let notes = pat.and_then(|x| x.notes.get(&c.id)).unwrap_or(&empty);
                let col = channel_colour(c);
                ui.horizontal(|ui| {
                    // mute light
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, ROW), Sense::click());
                    let lit = !c.mute;
                    ui.painter()
                        .circle_filled(r.center(), 6.0, if lit { p.ok } else { p.led_off });
                    if resp.on_hover_text("Mute").clicked() {
                        out.chans.push(ChanEdit::Begin);
                        out.chans.push(ChanEdit::Mute {
                            channel: i,
                            on: !c.mute,
                        });
                    }
                    // volume and pan
                    let mut vol = c.volume;
                    let (ch, start, kr) =
                        mini_knob_resp(ui, &mut vol, (0.0, 1.25), 1.0, "Level", p.text2);
                    kr.context_menu(|ui| {
                        if ui.button("Create automation clip").clicked() {
                            out.chans.push(ChanEdit::Automate(crate::auto::AutoRequest {
                                target: crate::auto::channel_target(&c.id, "volume"),
                                name: format!("{} level", c.name),
                                start: c.volume / 1.25,
                            }));
                            ui.close();
                        }
                    });
                    if start {
                        out.chans.push(ChanEdit::Begin);
                    }
                    if ch {
                        out.chans.push(ChanEdit::Volume {
                            channel: i,
                            value: vol,
                        });
                    }
                    let mut pan = c.pan;
                    let (ch, start, kr) =
                        mini_knob_resp(ui, &mut pan, (-1.0, 1.0), 0.0, "Pan", p.accent);
                    kr.context_menu(|ui| {
                        if ui.button("Create automation clip").clicked() {
                            out.chans.push(ChanEdit::Automate(crate::auto::AutoRequest {
                                target: crate::auto::channel_target(&c.id, "pan"),
                                name: format!("{} pan", c.name),
                                start: (c.pan + 1.0) / 2.0,
                            }));
                            ui.close();
                        }
                    });
                    if start {
                        out.chans.push(ChanEdit::Begin);
                    }
                    if ch {
                        out.chans.push(ChanEdit::Pan {
                            channel: i,
                            value: pan,
                        });
                    }
                    // number and name
                    let (nr, nresp) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::hover());
                    ui.painter().rect_filled(nr, 4.0, p.well_deep);
                    ui.painter().text(
                        nr.center(),
                        Align2::CENTER_CENTER,
                        (i + 1).to_string(),
                        FontId::proportional(11.0),
                        p.muted,
                    );
                    let _ = nresp;
                    let (nr, resp) = ui.allocate_exact_size(vec2(NAME, 26.0), Sense::click());
                    let sel = *selected == i;
                    let bg = if sel { p.raised2 } else { p.raised };
                    ui.painter().rect_filled(nr, 6.0, bg);
                    if sel {
                        ui.painter().rect_stroke(
                            nr,
                            6.0,
                            Stroke::new(1.5, p.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    ui.painter().rect_filled(
                        Rect::from_min_size(nr.min + vec2(0.0, 4.0), vec2(4.0, nr.height() - 8.0)),
                        2.0,
                        col,
                    );
                    ui.painter().text(
                        nr.left_center() + vec2(14.0, 0.0),
                        Align2::LEFT_CENTER,
                        &c.name,
                        FontId::proportional(13.0),
                        p.text,
                    );
                    if resp.clicked() {
                        *selected = i;
                    }
                    if resp.double_clicked() {
                        out.open_synth = Some(i);
                    }
                    let resp = resp.on_hover_text("Double click: instrument. Right click: more.");
                    resp.context_menu(|ui| {
                        if ui.button("Piano roll").clicked() {
                            out.open_roll = Some(i);
                            ui.close();
                        }
                        if ui.button("Instrument").clicked() {
                            out.open_synth = Some(i);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Rename...").clicked() {
                            out.chans.push(ChanEdit::AskRename(i));
                            ui.close();
                        }
                        if ui.button("Clone").clicked() {
                            out.chans.push(ChanEdit::Begin);
                            out.chans.push(ChanEdit::Clone(i));
                            ui.close();
                        }
                        ui.menu_button("Colour", |ui| {
                            for hex in las_project::create::PALETTE {
                                let col = crate::rack::channel_colour_of(hex);
                                let (r, sw) =
                                    ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                                ui.painter().rect_filled(r, 4.0, col);
                                if sw.clicked() {
                                    out.chans.push(ChanEdit::Begin);
                                    out.chans.push(ChanEdit::Color {
                                        channel: i,
                                        color: hex.to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Swing", |ui| {
                            let now = c.swing;
                            if ui.selectable_label(now.is_none(), "Song's swing").clicked() {
                                out.chans.push(ChanEdit::Begin);
                                out.chans.push(ChanEdit::Swing {
                                    channel: i,
                                    value: None,
                                });
                                ui.close();
                            }
                            for pct in [0u32, 15, 25, 33, 50, 66, 75] {
                                let v = f64::from(pct) / 100.0;
                                let on = now.is_some_and(|x| (x - v).abs() < 1e-6);
                                if ui.selectable_label(on, format!("{pct}%")).clicked() {
                                    out.chans.push(ChanEdit::Begin);
                                    out.chans.push(ChanEdit::Swing {
                                        channel: i,
                                        value: Some(v),
                                    });
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text("Swing for this channel's steps only");
                        ui.menu_button("Send to insert", |ui| {
                            for (n, t) in project.mixer.iter().enumerate() {
                                let label = if n == 0 {
                                    "Master".to_string()
                                } else if t.name.is_empty() {
                                    format!("Insert {n}")
                                } else {
                                    t.name.clone()
                                };
                                if ui.selectable_label(c.mixer_track == n, label).clicked() {
                                    out.chans.push(ChanEdit::Begin);
                                    out.chans.push(ChanEdit::Route {
                                        channel: i,
                                        insert: n,
                                    });
                                    ui.close();
                                }
                            }
                        });
                        ui.separator();
                        if ui.button("Delete").clicked() {
                            out.chans.push(ChanEdit::Begin);
                            out.chans.push(ChanEdit::Delete(i));
                            ui.close();
                        }
                    });
                    // the steps
                    let width = steps as f32 * (STEP + 3.0) + (steps / 4) as f32 * 4.0;
                    let (area, resp) =
                        ui.allocate_exact_size(vec2(width, ROW - 4.0), Sense::click_and_drag());
                    let cell_at = |x: f32| -> Option<usize> {
                        let mut left = area.left();
                        for s in 0..steps {
                            if s > 0 && s % 4 == 0 {
                                left += 4.0;
                            }
                            if x >= left && x < left + STEP + 3.0 {
                                return Some(s);
                            }
                            left += STEP + 3.0;
                        }
                        None
                    };
                    let mut left = area.left();
                    let painter = ui.painter_at(area);
                    for s in 0..steps {
                        if s > 0 && s % 4 == 0 {
                            left += 4.0;
                        }
                        let on = step_note(notes, s).is_some();
                        let cell = Rect::from_min_size(
                            pos2(left, area.top() + 2.0),
                            vec2(STEP, area.height() - 4.0),
                        );
                        let fill = if on {
                            col
                        } else if (s / 4) % 2 == 1 {
                            p.step_off_alt
                        } else {
                            p.step_off
                        };
                        painter.rect_filled(cell, 4.0, fill);
                        if on {
                            painter.rect_stroke(
                                cell,
                                4.0,
                                Stroke::new(1.0, Color32::from_white_alpha(70)),
                                egui::StrokeKind::Inside,
                            );
                        }
                        left += STEP + 3.0;
                    }
                    if let Some(t) = playhead {
                        let step = t / f64::from(TICKS_PER_STEP);
                        let whole = step.floor() as usize;
                        let x = area.left()
                            + (step as f32) * (STEP + 3.0)
                            + (whole / 4) as f32 * 4.0
                            + ((step - step.floor()) as f32 * 0.0);
                        painter.line_segment(
                            [pos2(x, area.top()), pos2(x, area.bottom())],
                            Stroke::new(1.5, Color32::WHITE),
                        );
                    }
                    // click toggles, dragging paints
                    let primary = ui.input(|i| i.pointer.primary_down());
                    if resp.drag_started_by(egui::PointerButton::Primary)
                        || resp.clicked_by(egui::PointerButton::Primary)
                    {
                        if let Some(s) = resp.interact_pointer_pos().and_then(|q| cell_at(q.x)) {
                            let e = toggle(notes, s);
                            state.painting = Some(matches!(e, Edit::Add(_)));
                            out.notes.push((i, Edit::Begin));
                            out.notes.push((i, e));
                            *selected = i;
                        }
                    } else if resp.dragged_by(egui::PointerButton::Primary) && primary {
                        if let (Some(add), Some(s)) = (
                            state.painting,
                            resp.interact_pointer_pos().and_then(|q| cell_at(q.x)),
                        ) {
                            let has = step_note(notes, s).is_some();
                            if add != has {
                                out.notes.push((i, toggle(notes, s)));
                            }
                        }
                    }
                    if resp.drag_stopped() {
                        state.painting = None;
                    }
                });
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/mixer.las");
        las_project::decode(bytes).unwrap().project
    }

    #[test]
    fn toggling_a_step_adds_then_removes_a_step_note() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let e = toggle(&[], 5);
        let Edit::Add(n) = &e else { panic!("{e:?}") };
        assert_eq!(n.pos, 5.0 * f64::from(TICKS_PER_STEP));
        assert_eq!(n.step, Some(true));
        crate::roll::apply(&mut p, 0, &ch, &e);
        let notes = &p.patterns[0].notes[&ch];
        assert!(step_note(notes, 5).is_some() && step_note(notes, 4).is_none());
        let off = toggle(notes, 5);
        assert!(matches!(off, Edit::Delete { .. }));
        crate::roll::apply(&mut p, 0, &ch, &off);
        assert!(!p.patterns[0].notes.contains_key(&ch));
    }

    #[test]
    fn channels_are_cloned_routed_coloured_and_deleted() {
        let mut p = project();
        let n = p.channels.len();
        let first = p.channels[0].id.clone();
        let notes = p.patterns[0].notes.get(&first).map_or(0, Vec::len);
        apply(&mut p, &ChanEdit::Clone(0));
        assert_eq!(p.channels.len(), n + 1);
        let copy = p.channels[1].clone();
        assert_ne!(copy.id, first);
        assert!(copy.name.ends_with("copy"));
        let copied = p.patterns[0].notes.get(&copy.id).map_or(0, Vec::len);
        assert_eq!(copied, notes);
        if notes > 0 {
            assert_ne!(
                p.patterns[0].notes[&copy.id][0].id,
                p.patterns[0].notes[&first][0].id
            );
        }
        apply(
            &mut p,
            &ChanEdit::Route {
                channel: 1,
                insert: 3,
            },
        );
        assert_eq!(p.channels[1].mixer_track, 3);
        apply(
            &mut p,
            &ChanEdit::Route {
                channel: 1,
                insert: 999,
            },
        );
        assert_eq!(p.channels[1].mixer_track, 3);
        apply(
            &mut p,
            &ChanEdit::Color {
                channel: 1,
                color: "#112233".into(),
            },
        );
        assert_eq!(p.channels[1].color, "#112233");
        apply(&mut p, &ChanEdit::Delete(1));
        assert_eq!(p.channels.len(), n);
        assert!(p.patterns.iter().all(|x| !x.notes.contains_key(&copy.id)));
        apply(&mut p, &ChanEdit::Delete(99));
        assert_eq!(p.channels.len(), n);
    }

    #[test]
    fn channel_edits_are_held_to_their_ranges() {
        let mut p = project();
        apply(
            &mut p,
            &ChanEdit::Volume {
                channel: 0,
                value: 5.0,
            },
        );
        apply(
            &mut p,
            &ChanEdit::Pan {
                channel: 0,
                value: -5.0,
            },
        );
        apply(
            &mut p,
            &ChanEdit::Mute {
                channel: 0,
                on: true,
            },
        );
        assert_eq!((p.channels[0].volume, p.channels[0].pan), (1.25, -1.0));
        assert!(p.channels[0].mute);
        apply(
            &mut p,
            &ChanEdit::Volume {
                channel: 99,
                value: 0.5,
            },
        );
    }

    #[test]
    fn the_step_sequencer_draws_and_a_click_toggles_a_step() {
        let p = project();
        let ctx = egui::Context::default();
        let mut state = StepState::default();
        let mut sel = 0usize;
        let mut all = Vec::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 400.0));
        // find an empty step in the first row by clicking a late cell
        let at = pos2(18.0 + 22.0 * 2.0 + 20.0 + NAME + 60.0 + 14.0 * 27.0, 24.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![press(true)],
            vec![press(false)],
            vec![],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                let o = draw(ui, &p, 0, &mut sel, &mut state, Some(48.0));
                all.extend(o.notes);
            });
        }
        assert!(
            all.iter()
                .any(|(_, e)| matches!(e, Edit::Add(_) | Edit::Delete { .. })),
            "{all:?}"
        );
    }
}
