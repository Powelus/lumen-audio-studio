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

pub fn draw(ui: &mut Ui, project: &Project, state: &mut MixerState) -> Vec<MixEdit> {
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
                        strip(ui, project, i, state, &mut edits);
                    }
                });
            });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(panel_w);
            detail(ui, project, state.selected, &mut edits);
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

fn strip(
    ui: &mut Ui,
    project: &Project,
    i: usize,
    state: &mut MixerState,
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
                let r = ui.add(
                    egui::Slider::new(&mut vol, 0.0..=1.25)
                        .vertical()
                        .show_value(false),
                );
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
                out = draw(ui, &p, &mut state);
            });
        }
        assert!(out.is_empty());
    }
}
