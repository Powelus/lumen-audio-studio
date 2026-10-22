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
    automate: Option<crate::auto::AutoRequest>,
}

impl App {
    /// The name an effect window's face shows (leaked once per effect kind: there are
    /// few, and a face holds static text).
    pub(crate) fn effect_title(&self, insert: usize, slot: usize) -> &'static str {
        let s = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.mixer.get(insert))
            .and_then(|t| t.slots.get(slot))
            .and_then(Option::as_ref);
        let name = match s {
            Some(s) if s.plugin_id == las_host::HOSTED => crate::hosted::name_of(&s.state),
            Some(s) => descriptor(&s.plugin_id).map_or(s.plugin_id.clone(), |d| d.name.clone()),
            None => "Effect".into(),
        }
        .to_uppercase();
        static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
        let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = names.iter().find(|n| **n == name) {
            return n;
        }
        let leaked: &'static str = Box::leak(name.into_boxed_str());
        names.push(leaked);
        leaked
    }

    pub(crate) fn effect_window(&mut self, ui: &mut Ui, insert: usize, slot: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let t = l.project.mixer.get(insert);
        let Some(Some(s)) = t.and_then(|t| t.slots.get(slot)) else {
            ui.label("This slot is empty.");
            return;
        };
        if s.plugin_id == las_host::HOSTED {
            self.hosted_effect(ui, insert, slot);
            return;
        }
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
                if ui
                    .button("Default")
                    .on_hover_text("Back to the effect's own settings")
                    .clicked()
                {
                    ch.state = Some(full_state(&id, &desc.default_state));
                    ui.close();
                }
                if let Some(st) = user_presets(ui, &id, &state) {
                    ch.state = Some(st);
                    ui.close();
                }
                if !desc.presets.is_empty() {
                    ui.separator();
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
            ui.menu_button("Automate", |ui| {
                let slot_id = s.id.clone();
                let mut list: Vec<(String, String, f64)> =
                    vec![("mix".to_string(), "Mix".to_string(), s.mix.clamp(0.0, 1.0))];
                for d in &desc.params {
                    if d.kind != las_dsp::fx::registry::ParamKind::Knob {
                        continue;
                    }
                    let v = crate::inst::at(&state, &d.id).and_then(Value::as_f64);
                    list.push((
                        d.id.clone(),
                        d.label.clone(),
                        v.map_or(0.0, |v| d.to_normalized(v)),
                    ));
                }
                for (param, label, start) in list {
                    if ui.button(&label).clicked() {
                        ch.automate = Some(crate::auto::AutoRequest {
                            target: crate::auto::effect_target(insert, &slot_id, &param),
                            name: format!("{} {label}", desc.name),
                            start,
                        });
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
            ui.label(RichText::new(format!("{:.0}%", mix * 100.0)).weak().small());
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
        if let Some(req) = ch.automate.take() {
            self.project.create_automation(&req);
        }
        if ch.remove {
            self.project.modify(true, |p| {
                if let Some(s) = p.mixer.get_mut(insert).and_then(|t| t.slots.get_mut(slot)) {
                    *s = None;
                }
            });
            self.project.open_effect = None;
        } else if ch.state.is_some() || ch.on.is_some() || ch.mix.is_some() {
            if let (Some(m), Some(l)) = (ch.mix, &self.project.loaded) {
                let project = l.project.clone();
                self.mixer_live(
                    &project,
                    &[crate::mixer::MixEdit::SlotMix {
                        insert,
                        slot,
                        mix: m,
                    }],
                );
            }
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

/// The bank the user's presets of effect `id` are kept in.
pub fn bank_of(id: &str) -> String {
    format!("fx-{id}")
}

/// Save the effect settings `state` as the user's preset `name` of effect `id`,
/// replacing one of that name.
pub fn save_preset(id: &str, name: &str, state: &Value) -> Result<(), String> {
    let defaults = full_state(id, &Value::Null);
    let mut list = crate::presets::entries(&bank_of(id), &defaults);
    list.retain(|e| e.user && e.name != name);
    list.push(crate::presets::Entry {
        name: name.to_string(),
        category: "User".into(),
        patch: state.clone(),
        user: true,
        fx: Vec::new(),
    });
    crate::presets::save_user(&bank_of(id), &list)
}

/// The user's presets of effect `id`, as complete settings.
pub fn user_presets_of(id: &str) -> Vec<(String, Value)> {
    let defaults = full_state(id, &Value::Null);
    crate::presets::entries(&bank_of(id), &defaults)
        .into_iter()
        .filter(|e| e.user)
        .map(|e| (e.name, full_state(id, &e.patch)))
        .collect()
}

/// Delete the user's preset `name` of effect `id`.
pub fn delete_preset(id: &str, name: &str) -> Result<(), String> {
    let defaults = full_state(id, &Value::Null);
    let mut list = crate::presets::entries(&bank_of(id), &defaults);
    list.retain(|e| e.user && e.name != name);
    crate::presets::save_user(&bank_of(id), &list)
}

/// The user's part of an effect's preset menu: their presets (a click loads one, x
/// deletes it) and a box to save the current settings under a name. Returns settings
/// to load.
fn user_presets(ui: &mut Ui, id: &str, state: &Value) -> Option<Value> {
    let mut pick = None;
    let list = user_presets_of(id);
    if !list.is_empty() {
        ui.separator();
        ui.label(RichText::new("User").strong());
    }
    for (name, st) in list {
        ui.horizontal(|ui| {
            if ui.button(&name).clicked() {
                pick = Some(st);
            }
            if ui
                .small_button("x")
                .on_hover_text("Delete this preset")
                .clicked()
            {
                let _ = delete_preset(id, &name);
            }
        });
    }
    ui.separator();
    let key = egui::Id::unique("fx-preset-name").with(id);
    let mut name: String = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("Preset name")
                .desired_width(110.0),
        );
        let ok = !name.trim().is_empty();
        if ui
            .add_enabled(ok, egui::Button::new("Save preset"))
            .on_hover_text("Keep these settings as a preset of your own")
            .clicked()
        {
            if let Err(e) = save_preset(id, name.trim(), state) {
                ui.label(RichText::new(e).color(pal().hot));
            }
            name.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(key, name));
    pick
}
