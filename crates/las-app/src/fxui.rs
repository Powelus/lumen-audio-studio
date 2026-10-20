// SPDX-License-Identifier: GPL-3.0-or-later
//! The effects rack: each instrument has a chain of effects, drawn from the
//! effect catalogue, so every effect's controls come from data.

use crate::app::{index, App, FxItem};
use crate::audio::Command;
use crate::widgets::knob_mapped;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke, Ui};
use las_dsp::fx::native::{eq_response_db, EqParams};
use las_dsp::fx::registry::{
    descriptor, descriptors, full_state, parse_params, shaper_presets, Descriptor, EffectUnit,
    FxSlot, ParamDef, ParamKind,
};
use serde_json::{json, Value};

const FALLBACK_RATE: f64 = 48_000.0;

/// What a click in the rack asks for, applied after drawing.
enum Edit {
    Add(&'static str),
    Remove(usize),
    Move(usize, usize),
    Preset(usize, Value),
}

fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}

fn get<'a>(state: &'a Value, path: &str) -> Option<&'a Value> {
    state.pointer(&pointer(path))
}

fn put(state: &mut Value, path: &str, v: Value) {
    if let Some(slot) = state.pointer_mut(&pointer(path)) {
        *slot = v;
    }
}

/// The text on a knob.
fn show(def: &ParamDef, v: f64) -> String {
    let unit = def.unit.as_deref().unwrap_or("");
    let max = def.max.unwrap_or(1.0);
    let (v, decimals) = if unit == "%" && max <= 2.0 {
        (v * 100.0, def.decimals.unwrap_or(0))
    } else {
        let auto = if v.abs() >= 1000.0 {
            0
        } else if v.abs() >= 100.0 {
            1
        } else {
            2
        };
        (v, def.decimals.unwrap_or(auto))
    };
    match unit {
        "" => format!("{v:.decimals$}"),
        ":1" => format!("{v:.decimals$}:1"),
        "%" | "Hz" | "dB" | "ms" | "s" | "oct" | "ct" => format!("{v:.decimals$} {unit}"),
        other => format!("{v:.decimals$} {other}"),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

/// Draw one parameter. Returns true when it changed.
fn param(ui: &mut Ui, def: &ParamDef, state: &mut Value, id: &str) -> bool {
    let Some(cur) = get(state, &def.id).cloned() else {
        return false;
    };
    match def.kind {
        ParamKind::Toggle => {
            let mut on = cur.as_bool().unwrap_or(false);
            if ui.checkbox(&mut on, &def.label).changed() {
                put(state, &def.id, Value::Bool(on));
                return true;
            }
        }
        ParamKind::Select => {
            let label = def
                .options
                .iter()
                .find(|o| same(&o.value, &cur))
                .map_or("?", |o| o.label.as_str());
            let mut pick = None;
            ui.vertical(|ui| {
                ui.label(RichText::new(&def.label).weak().small());
                egui::ComboBox::from_id_salt((id, &def.id))
                    .selected_text(label)
                    .width(86.0)
                    .show_ui(ui, |ui| {
                        for o in &def.options {
                            if ui
                                .selectable_label(same(&o.value, &cur), &o.label)
                                .clicked()
                            {
                                pick = Some(o.value.clone());
                            }
                        }
                    });
            });
            if let Some(v) = pick {
                put(state, &def.id, v);
                return true;
            }
        }
        ParamKind::Knob => {
            let mut v = cur.as_f64().unwrap_or(0.0);
            let default = def.default.as_f64().unwrap_or(0.0);
            if knob_mapped(
                ui,
                &mut v,
                &|x| def.to_normalized(x),
                &|t| def.from_normalized(t),
                default,
                &def.label,
                &|x| show(def, x),
            ) {
                put(state, &def.id, json!(v));
                return true;
            }
        }
    }
    false
}

/// The combined response of the equaliser's bands.
fn eq_curve(ui: &mut Ui, state: &Value, sr: f64) {
    let (rect, _) = ui.allocate_exact_size(vec2(560.0, 120.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, crate::theme::pal().well_deep);
    let Ok(p) = serde_json::from_value::<EqParams>(state.clone()) else {
        return;
    };
    let range = 18.0;
    let y_of = |db: f64| rect.center().y - (db / range) as f32 * rect.height() * 0.5;
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let c = if db == 0.0 {
            crate::theme::pal().grid_bar
        } else {
            crate::theme::pal().grid
        };
        painter.line_segment(
            [pos2(rect.left(), y_of(db)), pos2(rect.right(), y_of(db))],
            Stroke::new(1.0, c),
        );
    }
    for f in [100.0f64, 1000.0, 10000.0] {
        let x = rect.left() + (f / 20.0).ln() as f32 / (1000.0f64).ln() as f32 * rect.width();
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, crate::theme::pal().grid),
        );
    }
    let n = 200;
    let points: Vec<_> = (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let f = 20.0 * 1000f64.powf(t);
            let db = (eq_response_db(&p.bands, f, sr) + p.output).clamp(-range, range);
            pos2(rect.left() + t as f32 * rect.width(), y_of(db))
        })
        .collect();
    painter.add(egui::Shape::line(
        points,
        Stroke::new(2.0, crate::theme::accent()),
    ));
}

/// A row of values edited by dragging over bars. Returns true when changed.
fn lane(ui: &mut Ui, values: &mut [f64], lo: f64, hi: f64, count: usize, colour: Color32) -> bool {
    let size = vec2(count as f32 * 30.0, 64.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, crate::theme::pal().well_deep);
    let cell = rect.width() / count as f32;
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let i = (((pos.x - rect.left()) / cell) as usize).min(count - 1);
            let t = 1.0 - ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            if let Some(v) = values.get_mut(i) {
                *v = lo + (hi - lo) * f64::from(t);
                changed = true;
            }
        }
    }
    for (i, v) in values.iter().take(count).enumerate() {
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        let x0 = rect.left() + i as f32 * cell;
        let bar = Rect::from_min_max(
            pos2(x0 + 2.0, rect.bottom() - t * rect.height()),
            pos2(x0 + cell - 2.0, rect.bottom()),
        );
        painter.rect_filled(bar, 2.0, colour);
    }
    changed
}

fn number_list(v: &Value, n: usize, rest: f64) -> Vec<f64> {
    (0..n)
        .map(|i| v.get(i).and_then(Value::as_f64).unwrap_or(rest))
        .collect()
}

fn gate_steps(ui: &mut Ui, state: &mut Value) -> bool {
    let mut steps = number_list(&state["steps"], 16, 0.0);
    let mut changed = false;
    ui.horizontal(|ui| {
        for (i, s) in steps.iter_mut().enumerate() {
            let on = *s > 0.5;
            let label = RichText::new(format!("{}", i + 1)).small();
            if ui.selectable_label(on, label).clicked() {
                *s = if on { 0.0 } else { 1.0 };
                changed = true;
            }
        }
    });
    if changed {
        state["steps"] = json!(steps);
    }
    changed
}

fn shaper_curve(ui: &mut Ui, state: &mut Value) -> bool {
    let mut changed = false;
    let current = state["preset"].as_str().unwrap_or("").to_string();
    ui.horizontal(|ui| {
        ui.label("Shape");
        egui::ComboBox::from_id_salt("shaper-preset")
            .selected_text(if current.is_empty() {
                "Custom"
            } else {
                &current
            })
            .show_ui(ui, |ui| {
                for (name, pts) in shaper_presets() {
                    if ui.selectable_label(&current == name, name).clicked() {
                        state["preset"] = json!(name);
                        state["points"] = serde_json::to_value(pts).unwrap_or(Value::Null);
                        changed = true;
                    }
                }
            });
    });
    let (rect, _) = ui.allocate_exact_size(vec2(280.0, 90.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, crate::theme::pal().well_deep);
    if let Some(pts) = state["points"].as_array() {
        let line: Vec<_> = pts
            .iter()
            .filter_map(|p| Some((p.get("x")?.as_f64()?, p.get("y")?.as_f64()?)))
            .map(|(x, y)| {
                pos2(
                    rect.left() + x as f32 * rect.width(),
                    rect.bottom() - y as f32 * rect.height(),
                )
            })
            .collect();
        painter.add(egui::Shape::line(
            line,
            Stroke::new(2.0, crate::theme::accent()),
        ));
    }
    changed
}

fn stepfx_lanes(ui: &mut Ui, state: &mut Value) -> bool {
    let length = state["length"].as_f64().unwrap_or(16.0).clamp(1.0, 16.0) as usize;
    let mut changed = false;
    let lanes = [
        (
            "cutoff",
            "cutoffOn",
            "Cutoff",
            0.0,
            1.0,
            1.0,
            crate::theme::accent(),
        ),
        (
            "level",
            "levelOn",
            "Level",
            0.0,
            1.0,
            1.0,
            Color32::from_rgb(110, 190, 120),
        ),
        (
            "pan",
            "panOn",
            "Pan",
            -1.0,
            1.0,
            0.0,
            Color32::from_rgb(120, 150, 230),
        ),
    ];
    for (key, on_key, label, lo, hi, rest, colour) in lanes {
        let mut on = state[on_key].as_bool().unwrap_or(false);
        let mut values = number_list(&state[key], 16, rest);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, label).changed() {
                state[on_key] = json!(on);
                changed = true;
            }
            if lane(ui, &mut values, lo, hi, length, colour) {
                state[key] = json!(values);
                changed = true;
            }
        });
    }
    changed
}

fn chain_ok(id: &str) -> bool {
    EffectUnit::new(id, FALLBACK_RATE).is_some()
        && descriptor(id).is_some_and(|d| !d.params.is_empty())
}

impl App {
    fn rate(&self) -> f64 {
        self.audio
            .as_ref()
            .map_or(FALLBACK_RATE, |a| f64::from(a.sample_rate))
    }

    fn build_slot(&self, item: &FxItem) -> Option<FxSlot> {
        let sr = self.rate();
        let mut unit = EffectUnit::new(&item.id, sr)?;
        let params = parse_params(&item.id, &item.state, sr).ok()?;
        unit.apply(params);
        let mut slot = FxSlot::new(unit, sr);
        slot.enabled = item.on;
        slot.mix = item.mix;
        Some(slot)
    }

    /// Send the whole chain of an instrument to the audio thread.
    pub(crate) fn rebuild_chain(&mut self, inst: crate::audio::Instrument) {
        let slots: Vec<FxSlot> = self.fx[index(inst)]
            .iter()
            .filter_map(|item| self.build_slot(item))
            .collect();
        if let Some(a) = &mut self.audio {
            a.set_chain(inst, slots);
        }
    }

    /// Replace an instrument's chain with a preset's `{type, on, state}` items.
    pub(crate) fn load_fx_list(&mut self, inst: crate::audio::Instrument, list: &[Value]) {
        let items = list
            .iter()
            .filter_map(|v| {
                let id = v.get("type")?.as_str()?;
                if !chain_ok(id) {
                    return None;
                }
                Some(FxItem {
                    id: id.to_string(),
                    on: v.get("on").and_then(Value::as_bool).unwrap_or(true),
                    mix: 1.0,
                    state: full_state(id, v.get("state").unwrap_or(&Value::Null)),
                })
            })
            .collect();
        self.fx[index(inst)] = items;
        self.rebuild_chain(inst);
    }

    /// The effect at `slot` changed: send its new parameters.
    fn push_fx(&mut self, inst: crate::audio::Instrument, slot: usize) {
        let item = &self.fx[index(inst)][slot];
        match parse_params(&item.id, &item.state, self.rate()) {
            Ok(p) => {
                if let Some(a) = &mut self.audio {
                    a.set_fx_params(inst, slot, p);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn slot_ui(&mut self, ui: &mut Ui, slot: usize, edits: &mut Vec<Edit>) {
        let inst = self.inst;
        let k = index(inst);
        let count = self.fx[k].len();
        let id = self.fx[k][slot].id.clone();
        let Some(desc) = descriptor(&id) else {
            return;
        };
        let sr = self.rate();
        let mut state = self.fx[k][slot].state.clone();
        let mut on = self.fx[k][slot].on;
        let mut mix = self.fx[k][slot].mix;
        let (mut state_changed, mut mix_changed) = (false, false);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                mix_changed |= ui.checkbox(&mut on, "").changed();
                ui.label(
                    RichText::new(&desc.name)
                        .strong()
                        .size(16.0)
                        .color(crate::theme::accent()),
                );
                ui.label(RichText::new(&desc.category).weak());
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
                            edits.push(Edit::Preset(slot, p.state.clone()));
                            ui.close();
                        }
                    }
                });
                ui.label("Mix");
                mix_changed |= ui
                    .add(egui::Slider::new(&mut mix, 0.0..=1.0).show_value(false))
                    .changed();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        edits.push(Edit::Remove(slot));
                    }
                    if slot + 1 < count && ui.small_button("v").clicked() {
                        edits.push(Edit::Move(slot, slot + 1));
                    }
                    if slot > 0 && ui.small_button("^").clicked() {
                        edits.push(Edit::Move(slot, slot - 1));
                    }
                });
            });
            ui.label(RichText::new(&desc.description).weak().small());
            ui.add_space(4.0);
            match id.as_str() {
                "las-eq" => eq_curve(ui, &state, sr),
                "las-gate" => state_changed |= gate_steps(ui, &mut state),
                "las-shaper" => state_changed |= shaper_curve(ui, &mut state),
                "las-stepfx" => state_changed |= stepfx_lanes(ui, &mut state),
                _ => {}
            }
            state_changed |= params_ui(ui, desc, &mut state, &format!("{k}-{slot}"));
        });
        ui.add_space(6.0);
        if mix_changed {
            let item = &mut self.fx[k][slot];
            item.on = on;
            item.mix = mix;
            self.send(Command::FxMix(inst, slot, on, mix));
        }
        if state_changed {
            self.fx[k][slot].state = state;
            self.push_fx(inst, slot);
        }
    }

    /// The Effects view of the current instrument.
    pub(crate) fn effects_view(&mut self, ui: &mut Ui) {
        let mut edits = Vec::new();
        let k = index(self.inst);
        ui.horizontal(|ui| {
            ui.menu_button("Add effect", |ui| {
                let mut shown = "";
                let mut list: Vec<&'static Descriptor> =
                    descriptors().iter().filter(|d| chain_ok(&d.id)).collect();
                list.sort_by(|a, b| a.category.cmp(&b.category));
                for d in list {
                    if d.category != shown {
                        shown = &d.category;
                        ui.label(RichText::new(shown).strong().color(crate::theme::accent()));
                    }
                    if ui.button(&d.name).on_hover_text(&d.description).clicked() {
                        edits.push(Edit::Add(&d.id));
                        ui.close();
                    }
                }
            });
            ui.label("Tempo");
            if ui
                .add(
                    egui::DragValue::new(&mut self.bpm)
                        .range(40.0..=240.0)
                        .suffix(" bpm"),
                )
                .changed()
            {
                let bpm = self.bpm;
                self.send(Command::Tempo(bpm));
            }
            ui.label(RichText::new("Effects run in order, top to bottom").weak());
        });
        ui.add_space(6.0);
        if self.fx[k].is_empty() {
            ui.label(RichText::new("No effects. Add one from the menu.").weak());
        }
        for slot in 0..self.fx[k].len() {
            self.slot_ui(ui, slot, &mut edits);
        }
        let inst = self.inst;
        let mut rebuild = false;
        for e in edits {
            match e {
                Edit::Add(id) => {
                    if let Some(d) = descriptor(id) {
                        self.fx[k].push(FxItem {
                            id: id.to_string(),
                            on: true,
                            mix: 1.0,
                            state: d.default_state.clone(),
                        });
                        rebuild = true;
                    }
                }
                Edit::Remove(i) => {
                    if i < self.fx[k].len() {
                        self.fx[k].remove(i);
                        rebuild = true;
                    }
                }
                Edit::Move(a, b) => {
                    if a < self.fx[k].len() && b < self.fx[k].len() {
                        self.fx[k].swap(a, b);
                        rebuild = true;
                    }
                }
                Edit::Preset(i, state) => {
                    if let Some(item) = self.fx[k].get_mut(i) {
                        item.state = full_state(&item.id, &state);
                        rebuild = true;
                    }
                }
            }
        }
        if rebuild {
            self.rebuild_chain(inst);
        }
    }
}

/// The parameters, grouped as the catalogue says.
fn params_ui(ui: &mut Ui, desc: &Descriptor, state: &mut Value, id: &str) -> bool {
    let mut changed = false;
    let mut group: Option<&str> = None;
    let mut first = true;
    ui.horizontal_wrapped(|ui| {
        for def in &desc.params {
            let g = def.group.as_deref();
            if g != group || first {
                if !first {
                    ui.add_space(14.0);
                }
                group = g;
                first = false;
                if let Some(name) = g {
                    ui.label(RichText::new(name).strong().weak());
                }
            }
            changed |= param(ui, def, state, id);
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_exists_in_its_default_state() {
        for d in descriptors() {
            for p in &d.params {
                assert!(
                    get(&d.default_state, &p.id).is_some(),
                    "{} has no {} in its state",
                    d.id,
                    p.id
                );
            }
        }
    }

    #[test]
    fn dotted_paths_reach_into_lists() {
        let mut s = json!({ "bands": [{ "freq": 60 }, { "freq": 150 }] });
        assert_eq!(get(&s, "bands.1.freq"), Some(&json!(150)));
        put(&mut s, "bands.0.freq", json!(80));
        assert_eq!(s["bands"][0]["freq"], json!(80));
        // a missing path is left alone
        put(&mut s, "bands.9.freq", json!(1));
        assert_eq!(s["bands"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn knob_text_follows_the_unit() {
        let d = descriptor("las-comp").unwrap();
        let ratio = d.params.iter().find(|p| p.id == "ratio").unwrap();
        assert_eq!(show(ratio, 4.0), "4.00:1");
        let mix = d.params.iter().find(|p| p.id == "mix").unwrap();
        assert_eq!(show(mix, 0.5), "50 %");
        let eq = descriptor("las-eq").unwrap();
        let freq = eq.params.iter().find(|p| p.id == "bands.0.freq").unwrap();
        assert_eq!(show(freq, 1000.0), "1000 Hz");
    }

    #[test]
    fn selects_match_numbers_loosely() {
        assert!(same(&json!(0.25), &json!(0.25)));
        assert!(same(&json!(1), &json!(1.0)));
        assert!(!same(&json!("lp"), &json!("hp")));
    }

    #[test]
    fn the_add_menu_offers_only_working_effects() {
        let n = descriptors().iter().filter(|d| chain_ok(&d.id)).count();
        // 24 catalogue entries minus the scope and the meter
        assert_eq!(n, 22);
        assert!(!chain_ok("las-scope"));
        assert!(!chain_ok("nothing"));
    }
}
