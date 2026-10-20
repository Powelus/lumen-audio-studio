// SPDX-License-Identifier: GPL-3.0-or-later
//! The window of an effect in a mixer insert: its switch, presets, mix and parameters,
//! with the same curves and knobs as the instrument racks. Edits go to the project.

use crate::app::App;
use crate::fxui::{eq_curve, gate_steps, params_ui, shaper_curve, stepfx_lanes};
use crate::theme::pal;
use eframe::egui::{self, RichText, Ui};
use las_dsp::fx::registry::{descriptor, full_state};
use serde_json::Value;

/// What the user changed in one frame of the window.
#[derive(Default)]
struct Changes {
    state: Option<Value>,
    on: Option<bool>,
    mix: Option<f64>,
    remove: bool,
}

impl App {
    pub(crate) fn effect_window(&mut self, ui: &mut Ui, insert: usize, slot: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let t = l.project.mixer.get(insert);
        let Some(Some(s)) = t.and_then(|t| t.slots.get(slot)) else {
            ui.label("This slot is empty.");
            return;
        };
        let Some(desc) = descriptor(&s.plugin_id) else {
            ui.label(format!("{}: not available.", s.plugin_id));
            return;
        };
        let insert_name = t.map_or(String::new(), |t| t.name.clone());
        let mut state = full_state(&s.plugin_id, &s.state);
        let (mut on, mut mix) = (s.enabled, s.mix);
        let id = s.plugin_id.clone();
        let sr = self.rate();
        let mut ch = Changes::default();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, "").changed() {
                ch.on = Some(on);
            }
            ui.label(
                RichText::new(&desc.name)
                    .strong()
                    .size(16.0)
                    .color(pal().accent_text),
            );
            ui.label(RichText::new(format!("{} - {insert_name}", desc.category)).weak());
            ui.menu_button("Presets", |ui| {
                if desc.presets.is_empty() {
                    ui.label("No presets");
                }
                let mut shown = "";
                for p in &desc.presets {
                    if p.category != shown {
                        shown = &p.category;
                        ui.label(RichText::new(shown).strong());
                    }
                    if ui.button(&p.name).clicked() {
                        ch.state = Some(full_state(&id, &p.state));
                        ui.close();
                    }
                }
            });
            ui.label("Mix");
            if ui
                .add(egui::Slider::new(&mut mix, 0.0..=1.0).show_value(false))
                .changed()
            {
                ch.mix = Some(mix);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Remove").clicked() {
                    ch.remove = true;
                }
            });
        });
        ui.label(RichText::new(&desc.description).weak().small());
        ui.add_space(4.0);
        let mut changed = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            match id.as_str() {
                "las-eq" => eq_curve(ui, &state, sr),
                "las-gate" => changed |= gate_steps(ui, &mut state),
                "las-shaper" => changed |= shaper_curve(ui, &mut state),
                "las-stepfx" => changed |= stepfx_lanes(ui, &mut state),
                _ => {}
            }
            changed |= params_ui(ui, desc, &mut state, &format!("m{insert}-{slot}"));
        });
        if changed {
            ch.state = Some(state);
        }
        if ch.remove {
            self.project.modify(true, |p| {
                if let Some(s) = p.mixer.get_mut(insert).and_then(|t| t.slots.get_mut(slot)) {
                    *s = None;
                }
            });
            self.project.open_effect = None;
        } else if ch.state.is_some() || ch.on.is_some() || ch.mix.is_some() {
            self.project.modify_coalesced("effect", move |p| {
                if let Some(Some(s)) = p.mixer.get_mut(insert).and_then(|t| t.slots.get_mut(slot)) {
                    if let Some(st) = ch.state {
                        s.state = st;
                    }
                    if let Some(o) = ch.on {
                        s.enabled = o;
                    }
                    if let Some(m) = ch.mix {
                        s.mix = m;
                    }
                }
            });
        }
    }
}
