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
        plugin: String,
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
        MixEdit::AddEffect { insert, plugin } => {
            let Some(d) = las_dsp::fx::registry::descriptor(plugin) else {
                return;
            };
            if let Some(t) = project.mixer.get_mut(*insert) {
                if t.slots.len() < las_project::MIXER_SLOTS {
                    t.slots.resize(las_project::MIXER_SLOTS, None);
                }
                if let Some(free) = t.slots.iter_mut().find(|s| s.is_none()) {
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

/// The insert that is the master (the one that plays to nothing).
fn is_master(project: &Project, i: usize) -> bool {
    i == 0 || project.mixer[i].output.is_none()
}

/// Whether an insert has anything to show: the master, or an insert that is named,
/// has effects, or has been moved from its defaults.
fn in_use(project: &Project, i: usize) -> bool {
    let t = &project.mixer[i];
    is_master(project, i)
        || project.channels.iter().any(|c| c.mixer_track == i)
        || t.slots.iter().any(Option::is_some)
        || (t.volume - 1.0).abs() > 1e-9
        || t.pan != 0.0
        || t.mute
        || t.solo
}

pub fn draw(ui: &mut Ui, project: &Project) -> Vec<MixEdit> {
    let mut edits = Vec::new();
    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .id_salt("mixer")
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for i in 0..project.mixer.len() {
                    if !in_use(project, i) {
                        continue;
                    }
                    strip(ui, project, i, &mut edits);
                }
            });
        });
    edits
}

fn strip(ui: &mut Ui, project: &Project, i: usize, edits: &mut Vec<MixEdit>) {
    let t = &project.mixer[i];
    let col = channel_colour_of(&t.color);
    egui::Frame::new()
        .fill(crate::theme::pal().panel)
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(76.0);
            ui.vertical_centered(|ui| {
                let name = if t.name.is_empty() {
                    format!("Insert {i}")
                } else {
                    t.name.clone()
                };
                let bar = ui.available_rect_before_wrap();
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        bar.min - egui::vec2(0.0, 3.0),
                        egui::vec2(bar.width(), 3.0),
                    ),
                    1.0,
                    col,
                );
                ui.label(RichText::new(name).strong().size(12.0));
                // pan
                let mut pan = t.pan;
                let r = ui.add(
                    egui::Slider::new(&mut pan, -1.0..=1.0)
                        .show_value(false)
                        .text(""),
                );
                if r.changed() {
                    if r.drag_started() || !r.dragged() {
                        edits.push(MixEdit::Begin);
                    }
                    edits.push(MixEdit::Pan {
                        insert: i,
                        value: pan,
                    });
                }
                r.on_hover_text(format!("Pan {:+.0}", pan * 100.0));
                // fader
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
                ui.label(
                    RichText::new(format!("{:.1} dB", db(vol)))
                        .weak()
                        .size(10.5),
                );
                ui.horizontal(|ui| {
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
                ui.separator();
                for (n, slot) in t.slots.iter().enumerate() {
                    let Some(s) = slot else { continue };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
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
                        let label = RichText::new(short(&s.plugin_id)).size(11.5);
                        let label = if s.enabled { label } else { label.weak() };
                        let b = ui
                            .add(egui::Button::new(label).small())
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
                if t.slots.iter().filter(|s| s.is_some()).count() < las_project::MIXER_SLOTS {
                    ui.menu_button("+", |ui| {
                        let mut shown = "";
                        let mut list: Vec<_> =
                            las_dsp::fx::registry::descriptors().iter().collect();
                        list.sort_by(|a, b| a.category.cmp(&b.category));
                        for d in list {
                            if d.category != shown {
                                shown = &d.category;
                                ui.label(
                                    RichText::new(shown).strong().color(crate::theme::accent()),
                                );
                            }
                            if ui.button(&d.name).on_hover_text(&d.description).clicked() {
                                edits.push(MixEdit::Begin);
                                edits.push(MixEdit::AddEffect {
                                    insert: i,
                                    plugin: d.id.clone(),
                                });
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Add an effect");
                }
                if is_master(project, i) {
                    ui.label(
                        RichText::new("master")
                            .color(crate::theme::accent())
                            .size(10.5),
                    );
                }
            });
        });
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
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 2,
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
                plugin: "nothing".into(),
            },
        );
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 999,
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
        for _ in 0..2 {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                out = draw(ui, &p);
            });
        }
        assert!(out.is_empty());
    }
}
