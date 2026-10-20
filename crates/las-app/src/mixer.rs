// SPDX-License-Identifier: GPL-3.0-or-later
//! The mixer: one strip for every insert with its fader, pan, mute and solo and the
//! effects in its slots. Like the other editors, drawing reports [`MixEdit`]s and
//! [`apply`] changes the project.

use crate::rack::channel_colour_of;
use eframe::egui::{self, RichText, Ui};
use las_project::Project;

/// A change the user asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum MixEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    /// Put an effect in the first empty slot.
    AddEffect {
        insert: usize,
        slot: usize,
        plugin: String,
    },
    Rename {
        insert: usize,
        name: String,
    },
    /// Where the insert plays to (the master, or another insert).
    Output {
        insert: usize,
        target: usize,
    },
    AddSend {
        insert: usize,
        target: usize,
    },
    SetSend {
        insert: usize,
        index: usize,
        amount: f64,
    },
    SendSidechain {
        insert: usize,
        index: usize,
        on: bool,
    },
    RemoveSend {
        insert: usize,
        index: usize,
    },
    Width {
        insert: usize,
        value: f64,
    },
    /// Empty a slot.
    RemoveEffect {
        insert: usize,
        slot: usize,
    },
    /// Open the window of an effect (not a change to the project).
    OpenEffect {
        insert: usize,
        slot: usize,
    },
    Volume {
        insert: usize,
        value: f64,
    },
    Pan {
        insert: usize,
        value: f64,
    },
    Mute {
        insert: usize,
        on: bool,
    },
    Solo {
        insert: usize,
        on: bool,
    },
    SlotOn {
        insert: usize,
        slot: usize,
        on: bool,
    },
}

/// Whether feeding `to` from `from` would make a loop: `to` is `from`, or `from` is
/// reached by following where `to` plays to and sends to.
pub fn would_cycle(project: &Project, from: usize, to: usize) -> bool {
    let mut seen = vec![false; project.mixer.len()];
    let mut stack = vec![to];
    while let Some(i) = stack.pop() {
        if i == from {
            return true;
        }
        if i >= project.mixer.len() || std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let t = &project.mixer[i];
        stack.extend(t.output);
        stack.extend(t.sends.iter().map(|s| s.target));
    }
    false
}

/// Carry out one edit on a project.
pub fn apply(project: &mut Project, edit: &MixEdit) {
    let strip = |p: &mut Project, i: usize| -> Option<usize> { (i < p.mixer.len()).then_some(i) };
    match edit {
        MixEdit::Begin => {}
        MixEdit::Volume { insert, value } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].volume = value.clamp(0.0, 1.25);
            }
        }
        MixEdit::Pan { insert, value } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].pan = value.clamp(-1.0, 1.0);
            }
        }
        MixEdit::Mute { insert, on } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].mute = *on;
            }
        }
        MixEdit::Solo { insert, on } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].solo = *on;
            }
        }
        MixEdit::OpenEffect { .. } => {}
        MixEdit::Output { insert, target } => {
            if *insert > 0
                && *insert < project.mixer.len()
                && *target < project.mixer.len()
                && !would_cycle(project, *insert, *target)
            {
                project.mixer[*insert].output = Some(*target);
            }
        }
        MixEdit::AddSend { insert, target } => {
            if *insert < project.mixer.len()
                && *target < project.mixer.len()
                && *target != 0
                && !would_cycle(project, *insert, *target)
                && !project.mixer[*insert]
                    .sends
                    .iter()
                    .any(|s| s.target == *target)
            {
                project.mixer[*insert].sends.push(las_project::MixSend {
                    target: *target,
                    amount: 0.5,
                    ..Default::default()
                });
            }
        }
        MixEdit::SetSend {
            insert,
            index,
            amount,
        } => {
            if let Some(s) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.sends.get_mut(*index))
            {
                s.amount = amount.clamp(0.0, 1.25);
            }
        }
        MixEdit::SendSidechain { insert, index, on } => {
            if let Some(s) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.sends.get_mut(*index))
            {
                s.sidechain_only = *on;
            }
        }
        MixEdit::RemoveSend { insert, index } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                if *index < t.sends.len() {
                    t.sends.remove(*index);
                }
            }
        }
        MixEdit::Rename { insert, name } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.name = name.clone();
            }
        }
        MixEdit::Width { insert, value } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.width = value.clamp(0.0, 2.0);
            }
        }
        MixEdit::AddEffect {
            insert,
            slot,
            plugin,
        } => {
            let Some(d) = las_dsp::fx::registry::descriptor(plugin) else {
                return;
            };
            if let Some(t) = project.mixer.get_mut(*insert) {
                if t.slots.len() < las_project::MIXER_SLOTS {
                    t.slots.resize(las_project::MIXER_SLOTS, None);
                }
                if let Some(free) = t.slots.get_mut(*slot).filter(|s| s.is_none()) {
                    *free = Some(las_project::EffectSlot {
                        id: las_project::create::uid("fx"),
                        plugin_id: plugin.clone(),
                        enabled: true,
                        mix: 1.0,
                        state: d.default_state.clone(),
                        ..Default::default()
                    });
                }
            }
        }
        MixEdit::RemoveEffect { insert, slot } => {
            if let Some(s) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.slots.get_mut(*slot))
            {
                *s = None;
            }
        }
        MixEdit::SlotOn { insert, slot, on } => {
            if let Some(Some(s)) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.slots.get_mut(*slot))
            {
                s.enabled = *on;
            }
        }
    }
}

/// What the mixer remembers between frames.
#[derive(Default)]
pub struct MixerState {
    /// The insert whose effects and settings the panel on the right shows.
    pub selected: usize,
}

pub fn draw(ui: &mut Ui, project: &Project, state: &mut MixerState, peaks: &[f32]) -> Vec<MixEdit> {
    let mut edits = Vec::new();
    state.selected = state.selected.min(project.mixer.len().saturating_sub(1));
    let panel_w = 190.0;
    let avail = ui.available_width();
    ui.horizontal_top(|ui| {
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .id_salt("mixer")
            .max_width((avail - panel_w - 12.0).max(200.0))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for i in 0..project.mixer.len() {
                        let peak = peaks.get(i).copied().unwrap_or(0.0);
                        strip(ui, project, i, state, peak, &mut edits);
                    }
                });
            });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(panel_w);
            detail(ui, project, state.selected, &mut edits);
            routing(ui, project, state.selected, &mut edits);
        });
    });
    edits
}

/// The panel of the selected insert: its name, its effect slots, its stereo width.
fn detail(ui: &mut Ui, project: &Project, i: usize, edits: &mut Vec<MixEdit>) {
    let p = crate::theme::pal();
    let t = &project.mixer[i];
    let mut name = t.name.clone();
    if ui
        .add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY))
        .changed()
    {
        edits.push(MixEdit::Begin);
        edits.push(MixEdit::Rename { insert: i, name });
    }
    ui.add_space(6.0);
    ui.label(RichText::new("EFFECTS").size(11.0).strong().color(p.muted));
    for n in 0..las_project::MIXER_SLOTS {
        match t.slots.get(n).and_then(Option::as_ref) {
            Some(s) => {
                ui.horizontal(|ui| {
                    let led = ui
                        .add(
                            egui::Button::new(if s.enabled { "\u{25cf}" } else { "\u{25cb}" })
                                .small()
                                .frame(false),
                        )
                        .on_hover_text("Switch on or off");
                    if led.clicked() {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::SlotOn {
                            insert: i,
                            slot: n,
                            on: !s.enabled,
                        });
                    }
                    let b = ui
                        .add(
                            egui::Button::new(short(&s.plugin_id)).min_size(egui::vec2(100.0, 0.0)),
                        )
                        .on_hover_text(format!("{}\nRight click: remove", s.plugin_id));
                    if b.clicked() {
                        edits.push(MixEdit::OpenEffect { insert: i, slot: n });
                    }
                    b.context_menu(|ui| {
                        if ui.button("Remove").clicked() {
                            edits.push(MixEdit::Begin);
                            edits.push(MixEdit::RemoveEffect { insert: i, slot: n });
                            ui.close();
                        }
                    });
                });
            }
            None => {
                let r = ui.menu_button(
                    RichText::new(format!("+  Slot {}", n + 1)).color(p.muted),
                    |ui| {
                        let mut shown = "";
                        let mut list: Vec<_> =
                            las_dsp::fx::registry::descriptors().iter().collect();
                        list.sort_by(|a, b| a.category.cmp(&b.category));
                        for d in list {
                            if d.category != shown {
                                shown = &d.category;
                                ui.label(RichText::new(shown).strong().color(p.accent_text));
                            }
                            if ui.button(&d.name).on_hover_text(&d.description).clicked() {
                                edits.push(MixEdit::Begin);
                                edits.push(MixEdit::AddEffect {
                                    insert: i,
                                    slot: n,
                                    plugin: d.id.clone(),
                                });
                                ui.close();
                            }
                        }
                    },
                );
                r.response.on_hover_text("Add an effect");
            }
        }
    }
    ui.add_space(8.0);
    ui.label(RichText::new("STEREO").size(11.0).strong().color(p.muted));
    ui.horizontal(|ui| {
        let mut w = t.width;
        let (ch, start) = crate::steps::mini_knob(ui, &mut w, (0.0, 2.0), 1.0, "Width", p.accent);
        if start {
            edits.push(MixEdit::Begin);
        }
        if ch {
            edits.push(MixEdit::Width {
                insert: i,
                value: w,
            });
        }
        ui.label(
            RichText::new(format!("Width {:.0}%", t.width * 100.0))
                .size(11.0)
                .color(p.muted),
        );
    });
}

/// The name an insert goes by in the routing menus.
fn insert_name(project: &Project, i: usize) -> String {
    let t = &project.mixer[i];
    if i == 0 {
        "Master".to_string()
    } else if t.name.is_empty() {
        format!("Insert {i}")
    } else {
        t.name.clone()
    }
}

/// Output and sends of the selected insert.
fn routing(ui: &mut Ui, project: &Project, i: usize, edits: &mut Vec<MixEdit>) {
    let p = crate::theme::pal();
    let t = &project.mixer[i];
    if i > 0 {
        ui.add_space(8.0);
        ui.label(RichText::new("OUTPUT").size(11.0).strong().color(p.muted));
        let current = t.output.unwrap_or(0);
        egui::ComboBox::from_id_salt("mixer-output")
            .selected_text(insert_name(project, current))
            .width(150.0)
            .show_ui(ui, |ui| {
                for to in 0..project.mixer.len() {
                    if to == i || would_cycle(project, i, to) {
                        continue;
                    }
                    if ui
                        .selectable_label(current == to, insert_name(project, to))
                        .clicked()
                    {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::Output {
                            insert: i,
                            target: to,
                        });
                    }
                }
            });
    }
    ui.add_space(8.0);
    ui.label(RichText::new("SENDS").size(11.0).strong().color(p.muted));
    for (n, s) in t.sends.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(insert_name(project, s.target)).size(11.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("x")
                    .on_hover_text("Remove the send")
                    .clicked()
                {
                    edits.push(MixEdit::Begin);
                    edits.push(MixEdit::RemoveSend {
                        insert: i,
                        index: n,
                    });
                }
                let mut sc = s.sidechain_only;
                if ui
                    .checkbox(&mut sc, "SC")
                    .on_hover_text("Only feeds the sidechain")
                    .changed()
                {
                    edits.push(MixEdit::Begin);
                    edits.push(MixEdit::SendSidechain {
                        insert: i,
                        index: n,
                        on: sc,
                    });
                }
            });
        });
        let mut amount = s.amount;
        let r = ui.add(egui::Slider::new(&mut amount, 0.0..=1.25).show_value(false));
        if r.changed() {
            if r.drag_started() || !r.dragged() {
                edits.push(MixEdit::Begin);
            }
            edits.push(MixEdit::SetSend {
                insert: i,
                index: n,
                amount,
            });
        }
    }
    ui.menu_button(RichText::new("+  Send to...").color(p.muted), |ui| {
        let mut any = false;
        for to in 1..project.mixer.len() {
            if to == i || t.sends.iter().any(|s| s.target == to) || would_cycle(project, i, to) {
                continue;
            }
            any = true;
            if ui.button(insert_name(project, to)).clicked() {
                edits.push(MixEdit::Begin);
                edits.push(MixEdit::AddSend {
                    insert: i,
                    target: to,
                });
                ui.close();
            }
        }
        if !any {
            ui.label("No insert to send to.");
        }
    });
}

fn strip(
    ui: &mut Ui,
    project: &Project,
    i: usize,
    state: &mut MixerState,
    peak: f32,
    edits: &mut Vec<MixEdit>,
) {
    let t = &project.mixer[i];
    let p = crate::theme::pal();
    let col = channel_colour_of(&t.color);
    let selected = state.selected == i;
    let frame = egui::Frame::new()
        .fill(p.panel)
        .stroke(egui::Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected { p.accent } else { p.border },
        ))
        .corner_radius(6.0)
        .inner_margin(4.0)
        .show(ui, |ui| {
            ui.set_width(58.0);
            ui.set_min_height(300.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if i == 0 {
                        "M".to_string()
                    } else {
                        i.to_string()
                    })
                    .size(10.0)
                    .color(p.muted),
                );
                // the name on a pill in the insert's colour
                let name = if t.name.is_empty() {
                    format!("Insert {i}")
                } else {
                    t.name.clone()
                };
                let (r, resp) =
                    ui.allocate_exact_size(egui::vec2(54.0, 17.0), egui::Sense::click());
                ui.painter().rect_filled(r, 4.0, col.gamma_multiply(0.55));
                ui.painter().with_clip_rect(r).text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::proportional(10.5),
                    p.text,
                );
                if resp.clicked() {
                    state.selected = i;
                }
                let mut pan = t.pan;
                let (ch, start) =
                    crate::steps::mini_knob(ui, &mut pan, (-1.0, 1.0), 0.0, "Pan", p.text2);
                if start {
                    edits.push(MixEdit::Begin);
                }
                if ch {
                    edits.push(MixEdit::Pan {
                        insert: i,
                        value: pan,
                    });
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let m = ui.selectable_label(t.mute, "M");
                    if m.clicked() {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::Mute {
                            insert: i,
                            on: !t.mute,
                        });
                    }
                    let s = ui.selectable_label(t.solo, "S");
                    if s.clicked() {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::Solo {
                            insert: i,
                            on: !t.solo,
                        });
                    }
                });
                let mut vol = t.volume;
                let r = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        let r = ui.add(
                            egui::Slider::new(&mut vol, 0.0..=1.25)
                                .vertical()
                                .show_value(false),
                        );
                        crate::widgets::meter(ui, peak, egui::vec2(6.0, 100.0));
                        r
                    })
                    .inner;
                if r.changed() {
                    if r.drag_started() || !r.dragged() {
                        edits.push(MixEdit::Begin);
                    }
                    edits.push(MixEdit::Volume {
                        insert: i,
                        value: vol,
                    });
                }
                r.on_hover_text(format!("{} ({:.1} dB)", pct(vol), db(vol)));
                let label = if t.volume <= 0.0 {
                    "-\u{221e}".to_string()
                } else {
                    format!("{:.1}", db(vol))
                };
                ui.label(RichText::new(label).weak().size(10.0));
            });
        });
    if frame.response.interact(egui::Sense::click()).clicked() {
        state.selected = i;
    }
}

fn db(v: f64) -> f64 {
    if v <= 0.0 {
        -99.0
    } else {
        20.0 * v.log10()
    }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// An effect's id without its `las-` prefix.
fn short(id: &str) -> String {
    id.strip_prefix("las-").unwrap_or(id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/mixer.las");
        las_project::decode(bytes).unwrap().project
    }

    #[test]
    fn faders_pan_mute_solo_and_slots_change_the_project() {
        let mut p = project();
        apply(
            &mut p,
            &MixEdit::Volume {
                insert: 1,
                value: 9.0,
            },
        );
        assert_eq!(p.mixer[1].volume, 1.25, "held to the top of the fader");
        apply(
            &mut p,
            &MixEdit::Pan {
                insert: 1,
                value: -3.0,
            },
        );
        assert_eq!(p.mixer[1].pan, -1.0);
        apply(
            &mut p,
            &MixEdit::Mute {
                insert: 1,
                on: true,
            },
        );
        apply(
            &mut p,
            &MixEdit::Solo {
                insert: 1,
                on: true,
            },
        );
        assert!(p.mixer[1].mute && p.mixer[1].solo);
        // an insert that does not exist is ignored
        apply(
            &mut p,
            &MixEdit::Volume {
                insert: 999,
                value: 0.5,
            },
        );
        apply(
            &mut p,
            &MixEdit::SlotOn {
                insert: 999,
                slot: 0,
                on: false,
            },
        );
        if let Some(n) = p
            .mixer
            .iter()
            .position(|t| t.slots.iter().any(Option::is_some))
        {
            let slot = p.mixer[n].slots.iter().position(Option::is_some).unwrap();
            apply(
                &mut p,
                &MixEdit::SlotOn {
                    insert: n,
                    slot,
                    on: false,
                },
            );
            assert!(!p.mixer[n].slots[slot].as_ref().unwrap().enabled);
        }
    }

    #[test]
    fn routing_never_makes_a_loop() {
        let mut p = project();
        apply(
            &mut p,
            &MixEdit::Output {
                insert: 1,
                target: 2,
            },
        );
        assert_eq!(p.mixer[1].output, Some(2));
        // sending 2 back to 1 would loop, and so would 2 to itself
        let sends = p.mixer[2].sends.len();
        apply(
            &mut p,
            &MixEdit::Output {
                insert: 2,
                target: 1,
            },
        );
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 2,
                target: 1,
            },
        );
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 2,
                target: 2,
            },
        );
        assert_eq!(p.mixer[2].output, Some(0));
        assert_eq!(p.mixer[2].sends.len(), sends);
        assert!(would_cycle(&p, 2, 1) && !would_cycle(&p, 1, 3));
        // the master has no output to change, and cannot be a send target
        apply(
            &mut p,
            &MixEdit::Output {
                insert: 0,
                target: 1,
            },
        );
        assert_eq!(p.mixer[0].output, None);
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 1,
                target: 0,
            },
        );
        assert!(p.mixer[1].sends.iter().all(|s| s.target != 0));
    }

    #[test]
    fn sends_are_added_once_set_flagged_and_removed() {
        let mut p = project();
        let before = p.mixer[1].sends.len();
        let fresh = (3..p.mixer.len())
            .find(|t| p.mixer[1].sends.iter().all(|s| s.target != *t))
            .unwrap();
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 1,
                target: fresh,
            },
        );
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 1,
                target: fresh,
            },
        );
        assert_eq!(p.mixer[1].sends.len(), before + 1);
        let i = p.mixer[1]
            .sends
            .iter()
            .position(|s| s.target == fresh)
            .unwrap();
        apply(
            &mut p,
            &MixEdit::SetSend {
                insert: 1,
                index: i,
                amount: 9.0,
            },
        );
        apply(
            &mut p,
            &MixEdit::SendSidechain {
                insert: 1,
                index: i,
                on: true,
            },
        );
        assert_eq!(p.mixer[1].sends[i].amount, 1.25);
        assert!(p.mixer[1].sends[i].sidechain_only);
        apply(
            &mut p,
            &MixEdit::RemoveSend {
                insert: 1,
                index: i,
            },
        );
        assert_eq!(p.mixer[1].sends.len(), before);
        apply(
            &mut p,
            &MixEdit::RemoveSend {
                insert: 1,
                index: 99,
            },
        );
    }

    #[test]
    fn effects_are_added_to_the_first_free_slot_and_removed() {
        let mut p = project();
        let n = p.mixer[2].slots.iter().filter(|s| s.is_some()).count();
        let free = p.mixer[2].slots.iter().position(|s| s.is_none()).unwrap();
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 2,
                slot: free,
                plugin: "las-hall".into(),
            },
        );
        let slots = &p.mixer[2].slots;
        assert_eq!(slots.iter().filter(|s| s.is_some()).count(), n + 1);
        let at = slots
            .iter()
            .rposition(|s| s.as_ref().is_some_and(|s| s.plugin_id == "las-hall"));
        let at = at.expect("the effect is in a slot");
        assert!(slots[at].as_ref().unwrap().enabled);
        apply(
            &mut p,
            &MixEdit::RemoveEffect {
                insert: 2,
                slot: at,
            },
        );
        assert!(p.mixer[2].slots[at].is_none());
        // unknown effects and inserts are ignored
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 2,
                slot: 0,
                plugin: "nothing".into(),
            },
        );
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 999,
                slot: 0,
                plugin: "las-hall".into(),
            },
        );
        assert_eq!(p.mixer[2].slots.iter().filter(|s| s.is_some()).count(), n);
    }

    #[test]
    fn decibels_read_sensibly() {
        assert!((db(1.0)).abs() < 1e-9);
        assert!((db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(db(0.0), -99.0);
    }

    #[test]
    fn the_mixer_draws() {
        let p = project();
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let mut state = MixerState::default();
        for _ in 0..2 {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                out = draw(ui, &p, &mut state, &[0.5, 0.2]);
            });
        }
        assert!(out.is_empty());
    }
}
