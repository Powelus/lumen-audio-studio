// SPDX-License-Identifier: GPL-3.0-or-later
//! The windows of the instruments the synth panels do not cover: Nebula (layers,
//! macros and its 230 factory presets) and LumaPad (a 4 by 4 pad drum machine). Every
//! edit goes to the channel's state through undo, like the other instrument windows.

use crate::app::App;
use crate::theme::pal;
use crate::widgets::knob;
use eframe::egui::{self, vec2, Color32, RichText, Sense, Ui};
use serde_json::{json, Value};

fn num(v: Option<&Value>, d: f64) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(d)
}

/// A preset of Nebula's bank: its whole state (layers, macros, master).
#[cfg(test)]
pub fn nebula_preset(name: &str) -> Option<Value> {
    let b = las_dsp::presets::bank("nebula")?;
    b.find(name).map(|i| b.presets[i].state.clone())
}

/// The pads' state with one pad's sample changed.
pub fn set_pad_sample(state: &mut Value, pad: usize, id: &str, name: &str) {
    if let Some(p) = state.get_mut("pads").and_then(|p| p.get_mut(pad)) {
        p["sampleId"] = json!(id);
        p["name"] = json!(name);
    }
}

impl App {
    fn channel_state(&self, channel: usize) -> Option<(String, Value)> {
        let c = self
            .project
            .loaded
            .as_ref()?
            .project
            .channels
            .get(channel)?;
        Some((c.name.clone(), c.state.clone()))
    }

    fn write_state(&mut self, channel: usize, key: &'static str, state: Value) {
        self.project.modify_coalesced(key, move |p| {
            if let Some(c) = p.channels.get_mut(channel) {
                c.state = state;
            }
        });
    }

    /// Nebula: preset browser, master and macros, and the layers.
    pub(crate) fn nebula_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some((name, mut st)) = self.channel_state(channel) else {
            return;
        };
        if !st.is_object() {
            st = json!({});
        }
        let p = pal();
        let mut changed = false;
        let mut preset: Option<Value> = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&name).size(16.0).strong());
            ui.menu_button("Presets", |ui| {
                let Some(b) = las_dsp::presets::bank("nebula") else {
                    return;
                };
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for cat in b.categories() {
                            ui.menu_button(cat, |ui| {
                                for pr in b.presets.iter().filter(|x| x.category == cat) {
                                    if ui.button(&pr.name).clicked() {
                                        preset = Some(pr.state.clone());
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
            });
        });
        if let Some(s) = preset {
            self.project.modify(true, |pr| {
                if let Some(c) = pr.channels.get_mut(channel) {
                    c.state = s;
                }
            });
            return;
        }
        ui.separator();
        ui.horizontal(|ui| {
            let mut master = num(st.get("master"), 0.8);
            if knob(ui, &mut master, 0.0..=1.0, 0.8, "Master", false, &|v| {
                format!("{:.0}%", v * 100.0)
            }) {
                st["master"] = json!(master);
                changed = true;
            }
            ui.add_space(12.0);
            for (k, label) in [
                ("cutoff", "Cutoff"),
                ("res", "Res"),
                ("attack", "Attack"),
                ("release", "Release"),
            ] {
                let mut v = num(
                    st.get("macros").and_then(|m| m.get(k)),
                    if k == "res" { 0.0 } else { 0.5 },
                );
                if knob(
                    ui,
                    &mut v,
                    0.0..=1.0,
                    if k == "res" { 0.0 } else { 0.5 },
                    label,
                    false,
                    &|v| format!("{:.0}%", v * 100.0),
                ) {
                    if !st.get("macros").is_some_and(Value::is_object) {
                        st["macros"] = json!({});
                    }
                    st["macros"][k] = json!(v);
                    changed = true;
                }
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("LAYERS").size(11.0).strong().color(p.muted));
        if !st.get("layers").is_some_and(Value::is_array) {
            st["layers"] = json!([{ "on": true, "name": "Layer 1", "level": 1.0 }]);
        }
        let mut remove = None;
        let n = st["layers"].as_array().map_or(0, Vec::len);
        // the layers as cards side by side, each with its sound's waveform
        ui.horizontal_wrapped(|ui| {
            for i in 0..n {
                crate::skin::group(ui, |ui| {
                    ui.set_width(196.0);
                    ui.vertical(|ui| {
                        let l = &mut st["layers"][i];
                        ui.horizontal(|ui| {
                            let mut on = l.get("on") != Some(&Value::Bool(false));
                            if ui.checkbox(&mut on, "").changed() {
                                l["on"] = json!(on);
                                changed = true;
                            }
                            let mut nm = l
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("Layer")
                                .to_string();
                            if ui
                                .add(egui::TextEdit::singleline(&mut nm).desired_width(120.0))
                                .changed()
                            {
                                l["name"] = json!(nm);
                                changed = true;
                            }
                            if n > 1
                                && ui
                                    .small_button("x")
                                    .on_hover_text("Remove the layer")
                                    .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                        let on = l.get("on") != Some(&Value::Bool(false));
                        let table = l
                            .pointer("/patch/osc/0/table")
                            .and_then(Value::as_str)
                            .unwrap_or("saw")
                            .to_string();
                        let pos = l
                            .pointer("/patch/osc/0/pos")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0);
                        let frames = crate::skin_tables::frames(&table);
                        let f = frames
                            .get((pos * (frames.len().max(1) - 1) as f64).round() as usize)
                            .cloned()
                            .unwrap_or_default();
                        crate::skin::wave_screen(ui, vec2(190.0, 54.0), &f, on);
                        ui.horizontal(|ui| {
                            for (k, range, d, label) in [
                                ("level", 0.0..=1.5, 1.0, "Level"),
                                ("pan", -1.0..=1.0, 0.0, "Pan"),
                                ("transpose", -24.0..=24.0, 0.0, "Semi"),
                            ] {
                                let mut v = num(l.get(k), d);
                                let show = move |v: f64| match k {
                                    "transpose" => format!("{:+.0}", v.round()),
                                    "pan" => format!("{v:+.2}"),
                                    _ => format!("{:.0}%", v * 100.0),
                                };
                                if knob(ui, &mut v, range, d, label, false, &show) {
                                    l[k] = json!(if k == "transpose" { v.round() } else { v });
                                    changed = true;
                                }
                            }
                        });
                    });
                });
            }
            if n < 8 && ui.button("+ Layer").clicked() {
                if let Some(a) = st["layers"].as_array_mut() {
                    a.push(json!({ "on": true, "name": format!("Layer {}", n + 1), "level": 0.8 }));
                    changed = true;
                }
            }
        });
        if let Some(i) = remove {
            if let Some(a) = st["layers"].as_array_mut() {
                a.remove(i);
                changed = true;
            }
        }
        ui.label(
            RichText::new(
                "Each layer is a full wavetable synth; pick a preset to load a designed sound.",
            )
            .weak()
            .size(11.0),
        );
        if changed {
            self.write_state(channel, "nebula", st);
        }
    }

    /// LumaPad: sixteen pads in a 4 by 4 grid; click to play, right click for a
    /// sample; the selected pad's level, pan, tuning and choke group.
    pub(crate) fn pads_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some((name, mut st)) = self.channel_state(channel) else {
            return;
        };
        if !st.get("pads").is_some_and(Value::is_array) {
            st = las_project::create::default_state("lumapad");
            self.write_state(channel, "pads", st.clone());
        }
        let p = pal();
        let mut changed = false;
        let mut play: Vec<(u8, bool)> = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&name).size(16.0).strong());
        });
        ui.separator();
        let sel = self.shell.pad.min(15);
        let n = st["pads"].as_array().map_or(0, Vec::len).min(16);
        // the grid: the lowest pad bottom left, as on drum machines
        for row in (0..4).rev() {
            ui.horizontal(|ui| {
                for col in 0..4 {
                    let i = row * 4 + col;
                    if i >= n {
                        continue;
                    }
                    let pad = &st["pads"][i];
                    let label = pad
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Empty")
                        .to_string();
                    let key = num(pad.get("key"), 36.0 + i as f64) as u8;
                    let has = pad.get("sampleId").and_then(Value::as_str).is_some();
                    let (r, resp) =
                        ui.allocate_exact_size(vec2(96.0, 64.0), Sense::click_and_drag());
                    let fill = if resp.is_pointer_button_down_on() {
                        p.accent
                    } else if i == sel {
                        p.accent.gamma_multiply(0.45)
                    } else if has {
                        p.panel2
                    } else {
                        p.panel
                    };
                    ui.painter().rect_filled(r, 6.0, fill);
                    ui.painter().rect_stroke(
                        r,
                        6.0,
                        egui::Stroke::new(1.0, p.border),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        &label,
                        egui::FontId::proportional(12.0),
                        if has { Color32::WHITE } else { p.muted },
                    );
                    ui.painter().text(
                        r.left_bottom() + vec2(5.0, -4.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{}", i + 1),
                        egui::FontId::monospace(10.0),
                        p.muted,
                    );
                    if resp.is_pointer_button_down_on() {
                        self.shell.pad = i;
                        if self.shell.pad_held != Some(key) {
                            if let Some(k) = self.shell.pad_held.take() {
                                play.push((k, false));
                            }
                            self.shell.pad_held = Some(key);
                            play.push((key, true));
                        }
                    }
                    resp.context_menu(|ui| {
                        ui.label(RichText::new("Sample").strong());
                        egui::ScrollArea::vertical()
                            .max_height(360.0)
                            .show(ui, |ui| {
                                for (id, nm) in factory_samples() {
                                    if ui.button(&nm).clicked() {
                                        set_pad_sample(&mut st, i, &id, &nm);
                                        changed = true;
                                        ui.close();
                                    }
                                }
                            });
                    });
                }
            });
        }
        // the pad held is let go when the button is
        if self.shell.pad_held.is_some() && !ui.input(|i| i.pointer.any_down()) {
            if let Some(k) = self.shell.pad_held.take() {
                play.push((k, false));
            }
        }
        for (key, on) in play {
            self.note(key, 0.9, on);
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("PAD {}", sel + 1))
                .size(11.0)
                .strong()
                .color(p.muted),
        );
        if sel < n {
            ui.horizontal(|ui| {
                let pad = &mut st["pads"][sel];
                for (k, range, d, label) in [
                    ("level", 0.0..=1.0, 0.85, "Level"),
                    ("pan", -1.0..=1.0, 0.0, "Pan"),
                    ("tune", -24.0..=24.0, 0.0, "Tune"),
                ] {
                    let mut v = num(pad.get(k), d);
                    let unit = if k == "tune" { " st" } else { "" };
                    if knob(ui, &mut v, range, d, label, false, &|v| {
                        format!("{v:.2}{unit}")
                    }) {
                        pad[k] = json!(v);
                        changed = true;
                    }
                }
                let mut cut = num(pad.get("cut"), 0.0) as i64;
                ui.vertical(|ui| {
                    ui.label("Choke group");
                    if ui
                        .add(egui::DragValue::new(&mut cut).range(0..=8))
                        .on_hover_text("Pads in the same group (not 0) cut each other off")
                        .changed()
                    {
                        pad["cut"] = json!(cut);
                        changed = true;
                    }
                });
            });
        }
        ui.label(
            RichText::new("Click a pad to play it, right click to choose its sample. Pads play from the piano roll on keys C2 (36) and up.")
                .weak()
                .size(11.0),
        );
        if changed {
            self.write_state(channel, "pads", st);
        }
    }
}

/// The factory samples, by id and name.
pub fn factory_samples() -> Vec<(String, String)> {
    las_engine::factory::FACTORY_SAMPLES
        .iter()
        .map(|s| (s.id.to_string(), s.name.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nebula_presets_load_and_a_pad_takes_a_new_sample() {
        let init = nebula_preset("Init").expect("the Init preset");
        assert!(init["layers"].as_array().is_some_and(|l| !l.is_empty()));
        let mut st = las_project::create::default_state("lumapad");
        assert_eq!(st["pads"].as_array().unwrap().len(), 16);
        set_pad_sample(&mut st, 3, "factory:crash", "Crash");
        assert_eq!(st["pads"][3]["sampleId"], "factory:crash");
        assert_eq!(st["pads"][3]["name"], "Crash");
        assert!(!factory_samples().is_empty());
    }
}
