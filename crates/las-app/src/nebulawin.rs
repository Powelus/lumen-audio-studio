// SPDX-License-Identifier: GPL-3.0-or-later
//! Nebula's window, laid out like a ROM preset player: a browser of categories and
//! presets, a large display with the sound loaded, and three pages below it. The
//! main page has the macro knobs and the built-in effects (items of the channel's
//! effects rack), the layers page each layer's synth, and the arp page the
//! arpeggiator and a trance gate. Every edit goes to the channel's state with undo.

use crate::app::App;
use crate::widgets::{big_knob, knob};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, RichText, Sense, Stroke, Ui};
use las_dsp::presets::Bank;
use serde_json::{json, Value};

fn num(v: Option<&Value>, d: f64) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(d)
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// The width of the display and pages beside the browser.
const RIGHT: f32 = 760.0;

/// The browser's entry that lists the favourites.
pub const FAVOURITES: &str = "Favourites";

/// The macros: state key, label, default.
pub const MACROS: [(&str, &str, f64); 7] = [
    ("cutoff", "Cutoff", 0.5),
    ("res", "Resonance", 0.0),
    ("attack", "Attack", 0.5),
    ("release", "Release", 0.5),
    ("drive", "Drive", 0.0),
    ("width", "Width", 0.5),
    ("detune", "Detune", 0.5),
];

/// A built-in effect: its effect id, name, and up to three knobs (state key,
/// label, range, logarithmic).
struct Effect {
    id: &'static str,
    name: &'static str,
    knobs: [(&'static str, &'static str, f64, f64, bool); 3],
}

const EFFECTS: [Effect; 4] = [
    Effect {
        id: "las-filter",
        name: "FILTER",
        knobs: [
            ("cutoff", "Cutoff", 40.0, 18000.0, true),
            ("res", "Res", 0.5, 12.0, true),
            ("lfoDepth", "LFO", 0.0, 4.0, false),
        ],
    },
    Effect {
        id: "las-chorus",
        name: "CHORUS",
        knobs: [
            ("mix", "Mix", 0.0, 1.0, false),
            ("rate", "Rate", 0.05, 5.0, true),
            ("depth", "Depth", 0.0, 10.0, false),
        ],
    },
    Effect {
        id: "las-delay",
        name: "DELAY",
        knobs: [
            ("wet", "Mix", 0.0, 1.0, false),
            ("time", "Time", 0.0625, 1.0, false),
            ("feedback", "Feedback", 0.0, 0.95, false),
        ],
    },
    Effect {
        id: "las-reverb",
        name: "REVERB",
        knobs: [
            ("wet", "Mix", 0.0, 1.0, false),
            ("decay", "Decay", 0.2, 10.0, true),
            ("size", "Size", 0.0, 1.0, false),
        ],
    },
];

/// Where the rack holds the first effect of this type.
pub fn fx_index(state: &Value, id: &str) -> Option<usize> {
    state
        .get("fx")
        .and_then(Value::as_array)?
        .iter()
        .position(|f| f.get("type").and_then(Value::as_str) == Some(id))
}

/// Switch a built-in effect on or off; switching on one the rack lacks adds it.
pub fn set_fx_on(state: &mut Value, id: &str, on: bool) {
    match fx_index(state, id) {
        Some(i) => state["fx"][i]["on"] = json!(on),
        None if on => {
            if !state.get("fx").is_some_and(Value::is_array) {
                state["fx"] = json!([]);
            }
            let st = las_dsp::fx::registry::full_state(id, &json!({}));
            if let Some(a) = state["fx"].as_array_mut() {
                a.push(json!({ "type": id, "on": true, "state": st }));
            }
        }
        None => {}
    }
}

/// The names of the favourite presets.
pub fn favourites(state: &Value) -> Vec<String> {
    state
        .get("favs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Star or unstar a preset.
pub fn toggle_favourite(state: &mut Value, name: &str) {
    let mut favs = favourites(state);
    match favs.iter().position(|f| f == name) {
        Some(i) => {
            favs.remove(i);
        }
        None => favs.push(name.to_string()),
    }
    state["favs"] = json!(favs);
}

/// The presets the browser lists: those of a category (all for an empty one, the
/// starred ones for [`FAVOURITES`]) whose names hold the search text.
pub fn browse(bank: &Bank, category: &str, find: &str, favs: &[String]) -> Vec<usize> {
    let find = find.trim().to_lowercase();
    (0..bank.presets.len())
        .filter(|&i| {
            let p = &bank.presets[i];
            let in_cat = match category {
                "" => true,
                FAVOURITES => favs.contains(&p.name),
                c => p.category == c,
            };
            in_cat && (find.is_empty() || p.name.to_lowercase().contains(&find))
        })
        .collect()
}

/// The state a preset loads: its sound, with the channel's favourites kept.
pub fn preset_state(bank: &Bank, index: usize, current: &Value) -> Value {
    let p = &bank.presets[index];
    let mut s = p.state.clone();
    if !s.is_object() {
        s = json!({});
    }
    s["preset"] = json!(p.name);
    s["favs"] = json!(favourites(current));
    s
}

/// A preset at random from `list`, other than `not` when there is a choice.
fn random_pick(list: &[usize], not: Option<usize>) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(7, |d| d.as_nanos() as usize);
    let mut i = list[(seed / 1000) % list.len()];
    if Some(i) == not && list.len() > 1 {
        i = list[(seed / 1000 + 1) % list.len()];
    }
    Some(i)
}

/// The arpeggiator's step lengths, in beats.
const DIVS: [(f64, &str); 6] = [
    (1.0, "1/4"),
    (0.5, "1/8"),
    (1.0 / 3.0, "1/8 T"),
    (0.25, "1/16"),
    (1.0 / 6.0, "1/16 T"),
    (0.125, "1/32"),
];

impl App {
    /// Nebula: the browser and display on top, then the main, layers or arp page.
    pub(crate) fn nebula_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some((_, mut st)) = self.channel_state(channel) else {
            return;
        };
        if !st.is_object() {
            st = json!({});
        }
        let Some(bank) = las_dsp::presets::bank("nebula") else {
            return;
        };
        let mut changed = false;
        let mut load: Option<usize> = None;
        let mut cat = std::mem::take(&mut self.shell.nebula_cat);
        let mut find = std::mem::take(&mut self.shell.nebula_find);
        let mut page = self.shell.nebula_page;
        let favs = favourites(&st);
        let current = st
            .get("preset")
            .and_then(Value::as_str)
            .and_then(|n| bank.find(n));
        let list = browse(bank, &cat, &find, &favs);
        ui.horizontal_top(|ui| {
            // the browser: categories, then the presets in the one chosen
            crate::skin::group(ui, |ui| {
                ui.set_height(452.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(96.0);
                        ui.label(RichText::new("CATEGORY").size(10.5).strong());
                        egui::ScrollArea::vertical()
                            .id_salt("nebula-cats")
                            .max_height(424.0)
                            .show(ui, |ui| {
                                let mut cats = vec![""];
                                cats.extend(bank.categories());
                                cats.push(FAVOURITES);
                                for c in cats {
                                    let label = match c {
                                        "" => "All",
                                        FAVOURITES => "\u{2605} Favourites",
                                        c => c,
                                    };
                                    if ui.selectable_label(cat == c, label).clicked() {
                                        cat = c.to_string();
                                    }
                                }
                            });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut find)
                                .hint_text("Search presets")
                                .desired_width(196.0),
                        );
                        egui::ScrollArea::vertical()
                            .id_salt("nebula-list")
                            .max_height(408.0)
                            .show(ui, |ui| {
                                for &i in &list {
                                    let p = &bank.presets[i];
                                    let star = if favs.contains(&p.name) {
                                        " \u{2605}"
                                    } else {
                                        ""
                                    };
                                    let r = ui.selectable_label(
                                        current == Some(i),
                                        format!("{}{star}", p.name),
                                    );
                                    if r.clicked() {
                                        load = Some(i);
                                    }
                                }
                                if list.is_empty() {
                                    ui.label(RichText::new("No presets").weak());
                                }
                            });
                    });
                });
            });
            ui.vertical(|ui| {
                ui.set_width(RIGHT);
                lcd(ui, bank, current, &st, list.len());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let pos = current.and_then(|c| list.iter().position(|&i| i == c));
                    let step = |d: isize| -> Option<usize> {
                        if list.is_empty() {
                            return None;
                        }
                        let n = list.len() as isize;
                        let at = pos.map_or(if d > 0 { -1 } else { 0 }, |p| p as isize);
                        Some(list[(at + d).rem_euclid(n) as usize])
                    };
                    if ui
                        .button("\u{25C0}")
                        .on_hover_text("Previous preset")
                        .clicked()
                    {
                        load = step(-1);
                    }
                    if ui.button("\u{25B6}").on_hover_text("Next preset").clicked() {
                        load = step(1);
                    }
                    if ui
                        .button("Random")
                        .on_hover_text("A preset at random from the list")
                        .clicked()
                    {
                        load = random_pick(&list, current);
                    }
                    if let Some(c) = current {
                        let name = bank.presets[c].name.clone();
                        let on = favs.contains(&name);
                        let star = if on {
                            "\u{2605} Favourite"
                        } else {
                            "\u{2606} Favourite"
                        };
                        if ui.selectable_label(on, star).clicked() {
                            toggle_favourite(&mut st, &name);
                            changed = true;
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (i, label) in ["MAIN", "LAYERS", "ARP / GATE"].iter().enumerate() {
                        if ui
                            .selectable_label(page == i, RichText::new(*label).strong())
                            .clicked()
                        {
                            page = i;
                        }
                    }
                });
                ui.add_space(2.0);
                match page {
                    1 => changed |= layers_page(ui, &mut st),
                    2 => changed |= arp_page(ui, &mut st),
                    _ => changed |= main_page(ui, &mut st),
                }
            });
        });
        self.shell.nebula_cat = cat;
        self.shell.nebula_find = find;
        self.shell.nebula_page = page;
        if let Some(i) = load {
            let s = preset_state(bank, i, &st);
            self.project.modify(true, |pr| {
                if let Some(c) = pr.channels.get_mut(channel) {
                    c.state = s;
                }
            });
            return;
        }
        if changed {
            self.write_state(channel, "nebula", st);
        }
    }
}

/// The display: the preset's name large, its category, the layers and what is on.
fn lcd(ui: &mut Ui, bank: &Bank, current: Option<usize>, st: &Value, listed: usize) {
    let s = crate::skin::current().unwrap_or_else(|| crate::skin::skin_for("nebula"));
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 112.0), Sense::hover());
    let p = ui.painter_at(r.expand(4.0));
    // a glass screen with a soft glow along its edge
    p.rect_filled(r.expand(3.0), 10.0, s.accent.gamma_multiply(0.12));
    let mut mesh = egui::Mesh::default();
    let top = s.screen.lerp_to_gamma(s.trace, 0.10);
    mesh.colored_vertex(r.left_top(), top);
    mesh.colored_vertex(r.right_top(), top);
    mesh.colored_vertex(r.right_bottom(), s.screen);
    mesh.colored_vertex(r.left_bottom(), s.screen);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
    // scan lines, as on an LCD
    let mut y = r.top() + 3.0;
    while y < r.bottom() {
        p.line_segment(
            [pos2(r.left() + 2.0, y), pos2(r.right() - 2.0, y)],
            Stroke::new(1.0, Color32::from_black_alpha(26)),
        );
        y += 3.0;
    }
    p.rect_stroke(
        r,
        8.0,
        Stroke::new(1.0, s.trace.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    let glow = |at: egui::Pos2, align: Align2, text: &str, size: f32, col: Color32| {
        p.text(
            at,
            align,
            text,
            FontId::proportional(size),
            col.gamma_multiply(0.25),
        );
        p.text(
            at + vec2(0.0, -0.5),
            align,
            text,
            FontId::proportional(size),
            col,
        );
    };
    let (name, cat) = match current {
        Some(i) => (
            bank.presets[i].name.as_str(),
            bank.presets[i].category.as_str(),
        ),
        None => ("Custom Sound", "User"),
    };
    let layers = st.get("layers").and_then(Value::as_array).map_or(0, |a| {
        a.iter()
            .filter(|l| l.get("on") != Some(&Value::Bool(false)))
            .count()
    });
    glow(
        r.left_top() + vec2(14.0, 16.0),
        Align2::LEFT_CENTER,
        &cat.to_uppercase(),
        12.0,
        s.dim,
    );
    let index = current.map_or_else(String::new, |i| {
        format!("{:03} / {}", i + 1, bank.presets.len())
    });
    glow(
        r.right_top() + vec2(-14.0, 16.0),
        Align2::RIGHT_CENTER,
        &index,
        12.0,
        s.dim,
    );
    glow(
        r.center() + vec2(0.0, -2.0),
        Align2::CENTER_CENTER,
        name,
        28.0,
        s.trace,
    );
    let mut tags = vec![format!(
        "{layers} LAYER{}",
        if layers == 1 { "" } else { "S" }
    )];
    if st.pointer("/arp/on") == Some(&Value::Bool(true)) {
        tags.push("ARP".into());
    }
    for e in &EFFECTS {
        if fx_index(st, e.id).is_some_and(|i| st["fx"][i].get("on") != Some(&Value::Bool(false))) {
            tags.push(e.name.into());
        }
    }
    if fx_index(st, "las-gate").is_some_and(|i| st["fx"][i].get("on") != Some(&Value::Bool(false)))
    {
        tags.push("GATE".into());
    }
    glow(
        r.left_bottom() + vec2(14.0, -16.0),
        Align2::LEFT_CENTER,
        &tags.join("  \u{00B7}  "),
        11.5,
        s.accent,
    );
    glow(
        r.right_bottom() + vec2(-14.0, -16.0),
        Align2::RIGHT_CENTER,
        &format!("{listed} IN LIST"),
        11.0,
        s.dim,
    );
}

/// The macro knobs, master and the built-in effects.
fn main_page(ui: &mut Ui, st: &mut Value) -> bool {
    let mut changed = false;
    crate::skin::group(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("MACROS").size(10.5).strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (k, label, d) in MACROS {
                let mut v = num(st.get("macros").and_then(|m| m.get(k)), d);
                if big_knob(ui, &mut v, 0.0..=1.0, d, label, &pct) {
                    if !st.get("macros").is_some_and(Value::is_object) {
                        st["macros"] = json!({});
                    }
                    st["macros"][k] = json!(v);
                    changed = true;
                }
            }
            ui.separator();
            let mut master = num(st.get("master"), 0.8);
            if big_knob(ui, &mut master, 0.0..=1.2, 0.8, "Master", &pct) {
                st["master"] = json!(master);
                changed = true;
            }
        });
    });
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        for e in &EFFECTS {
            crate::skin::group(ui, |ui| {
                ui.vertical(|ui| {
                    let at = fx_index(st, e.id);
                    let mut on =
                        at.is_some_and(|i| st["fx"][i].get("on") != Some(&Value::Bool(false)));
                    if ui
                        .checkbox(&mut on, RichText::new(e.name).strong())
                        .changed()
                    {
                        set_fx_on(st, e.id, on);
                        changed = true;
                    }
                    let full = las_dsp::fx::registry::full_state(
                        e.id,
                        at.and_then(|i| st["fx"][i].get("state"))
                            .unwrap_or(&Value::Null),
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (k, label, lo, hi, log) in e.knobs {
                            let mut v = num(full.get(k), lo);
                            let def = descriptor_default(e.id, k).unwrap_or(v);
                            let show = move |v: f64| match k {
                                "cutoff" => format!("{v:.0} Hz"),
                                "decay" => format!("{v:.1} s"),
                                "rate" => format!("{v:.2} Hz"),
                                "depth" => format!("{v:.1} ms"),
                                "time" => format!("{}/16", (v * 16.0).round()),
                                "res" => format!("{v:.1}"),
                                "lfoDepth" => format!("{v:.1} oct"),
                                _ => pct(v),
                            };
                            if knob(ui, &mut v, lo..=hi, def, label, log, &show) {
                                let v = if k == "time" {
                                    ((v * 16.0).round() / 16.0).max(0.0625)
                                } else {
                                    v
                                };
                                if at.is_none() {
                                    set_fx_on(st, e.id, true);
                                }
                                if let Some(i) = fx_index(st, e.id) {
                                    if !st["fx"][i].get("state").is_some_and(Value::is_object) {
                                        st["fx"][i]["state"] = json!({});
                                    }
                                    st["fx"][i]["state"][k] = json!(v);
                                }
                                changed = true;
                            }
                        }
                    });
                });
            });
        }
    });
    changed
}

/// An effect parameter's default value.
fn descriptor_default(id: &str, key: &str) -> Option<f64> {
    las_dsp::fx::registry::full_state(id, &json!({}))
        .get(key)
        .and_then(Value::as_f64)
}

/// The layers as cards: on, name, level, pan, transpose and the wave they start from.
fn layers_page(ui: &mut Ui, st: &mut Value) -> bool {
    let mut changed = false;
    if !st.get("layers").is_some_and(Value::is_array) {
        st["layers"] = json!([{ "on": true, "name": "Layer 1", "level": 1.0 }]);
    }
    let mut remove = None;
    let n = st["layers"].as_array().map_or(0, Vec::len);
    ui.horizontal_wrapped(|ui| {
        for i in 0..n {
            crate::skin::group(ui, |ui| {
                ui.set_width(196.0);
                ui.vertical(|ui| {
                    let l = &mut st["layers"][i];
                    ui.horizontal(|ui| {
                        let mut on = l.get("on") != Some(&Value::Bool(false));
                        if ui
                            .checkbox(&mut on, "")
                            .on_hover_text("Layer on or off")
                            .changed()
                        {
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
                    ui.label(RichText::new(format!("Patch: {table}")).size(10.5).weak());
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
                                _ => pct(v),
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
        RichText::new("Each layer is a full wavetable synth; load a preset for a designed sound.")
            .weak()
            .size(11.0),
    );
    changed
}

/// The arpeggiator's settings and a sixteen step trance gate.
fn arp_page(ui: &mut Ui, st: &mut Value) -> bool {
    let mut changed = false;
    if !st.get("arp").is_some_and(Value::is_object) {
        st["arp"] = json!({ "on": false });
    }
    crate::skin::group(ui, |ui| {
        ui.horizontal(|ui| {
            let a = &mut st["arp"];
            let mut on = a.get("on") == Some(&Value::Bool(true));
            if ui
                .checkbox(&mut on, RichText::new("ARPEGGIATOR").strong())
                .changed()
            {
                a["on"] = json!(on);
                changed = true;
            }
            ui.add_space(8.0);
            let mode = a
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("up")
                .to_string();
            egui::ComboBox::from_id_salt("nebula-arp-mode")
                .selected_text(mode.clone())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for m in ["up", "down", "updown", "random"] {
                        if ui.selectable_label(mode == m, m).clicked() {
                            a["mode"] = json!(m);
                            changed = true;
                        }
                    }
                });
            let div = num(a.get("div"), 0.25);
            let name = DIVS
                .iter()
                .find(|d| (d.0 - div).abs() < 1e-6)
                .map_or("1/16", |d| d.1);
            egui::ComboBox::from_id_salt("nebula-arp-div")
                .selected_text(name)
                .width(70.0)
                .show_ui(ui, |ui| {
                    for (v, label) in DIVS {
                        if ui.selectable_label((v - div).abs() < 1e-6, label).clicked() {
                            a["div"] = json!(v);
                            changed = true;
                        }
                    }
                });
            let mut oct = num(a.get("octaves"), 1.0);
            if knob(ui, &mut oct, 1.0..=4.0, 1.0, "Octaves", false, &|v| {
                format!("{:.0}", v.round())
            }) {
                a["octaves"] = json!(oct.round());
                changed = true;
            }
            let mut gate = num(a.get("gate"), 0.7);
            if knob(ui, &mut gate, 0.05..=1.0, 0.7, "Gate", false, &pct) {
                a["gate"] = json!(gate);
                changed = true;
            }
        });
        ui.label(
            RichText::new(
                "Hold a chord: the arpeggiator plays it in steps, in time with the song.",
            )
            .weak()
            .size(11.0),
        );
    });
    ui.add_space(4.0);
    crate::skin::group(ui, |ui| {
        let at = fx_index(st, "las-gate");
        let mut on = at.is_some_and(|i| st["fx"][i].get("on") != Some(&Value::Bool(false)));
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut on, RichText::new("TRANCE GATE").strong())
                .changed()
            {
                set_fx_on(st, "las-gate", on);
                changed = true;
            }
        });
        let Some(i) = fx_index(st, "las-gate") else {
            ui.label(
                RichText::new("Switch it on to chop the sound in sixteenth steps.")
                    .weak()
                    .size(11.0),
            );
            return;
        };
        if !st["fx"][i].get("state").is_some_and(Value::is_object) {
            st["fx"][i]["state"] = json!({});
        }
        let g = &mut st["fx"][i]["state"];
        let mut steps: Vec<f64> = g
            .get("steps")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
            .unwrap_or_default();
        if steps.len() != 16 {
            steps = (0..16)
                .map(|k| if k % 2 == 0 { 1.0 } else { 0.0 })
                .collect();
        }
        let s = crate::skin::current().unwrap_or_else(|| crate::skin::skin_for("nebula"));
        let mut flip = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (k, v) in steps.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(26.0, 34.0), Sense::click());
                let lit = *v > 0.5;
                let fill = if lit { s.accent } else { s.track };
                ui.painter().rect_filled(r, 4.0, fill);
                if k % 4 == 0 {
                    ui.painter().rect_stroke(
                        r,
                        4.0,
                        Stroke::new(1.0, s.edge),
                        egui::StrokeKind::Inside,
                    );
                }
                if resp.clicked() {
                    flip = Some(k);
                }
            }
        });
        if let Some(k) = flip {
            steps[k] = if steps[k] > 0.5 { 0.0 } else { 1.0 };
            g["steps"] = json!(steps);
            changed = true;
        }
        ui.horizontal(|ui| {
            for (k, label, lo, hi, d) in [
                ("gate", "Length", 0.05, 1.0, 0.6),
                ("depth", "Depth", 0.0, 1.0, 1.0),
            ] {
                let mut v = num(g.get(k), d);
                if knob(ui, &mut v, lo..=hi, d, label, false, &pct) {
                    g[k] = json!(v);
                    changed = true;
                }
            }
            let div = num(g.get("div"), 0.25);
            egui::ComboBox::from_id_salt("nebula-gate-div")
                .selected_text(
                    DIVS.iter()
                        .find(|d| (d.0 - div).abs() < 1e-6)
                        .map_or("1/16", |d| d.1),
                )
                .width(70.0)
                .show_ui(ui, |ui| {
                    for (v, label) in DIVS {
                        if ui.selectable_label((v - div).abs() < 1e-6, label).clicked() {
                            g["div"] = json!(v);
                            changed = true;
                        }
                    }
                });
        });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_filters_and_a_preset_keeps_the_favourites() {
        let b = las_dsp::presets::bank("nebula").unwrap();
        let all = browse(b, "", "", &[]);
        assert_eq!(all.len(), b.presets.len());
        let bass = browse(b, "Bass", "", &[]);
        assert!(!bass.is_empty() && bass.iter().all(|&i| b.presets[i].category == "Bass"));
        let found = browse(b, "", "SUPERSAW", &[]);
        assert!(found
            .iter()
            .all(|&i| b.presets[i].name.to_lowercase().contains("supersaw")));
        assert!(!found.is_empty());
        let mut st = json!({});
        toggle_favourite(&mut st, &b.presets[3].name);
        let favs = favourites(&st);
        assert_eq!(browse(b, FAVOURITES, "", &favs), vec![3]);
        let loaded = preset_state(b, 5, &st);
        assert_eq!(loaded["preset"], json!(b.presets[5].name));
        assert_eq!(favourites(&loaded), favs);
        toggle_favourite(&mut st, &b.presets[3].name);
        assert!(favourites(&st).is_empty());
    }

    #[test]
    fn an_effect_switched_on_joins_the_rack() {
        let mut st = json!({ "fx": [{ "type": "las-delay", "on": true, "state": {} }] });
        set_fx_on(&mut st, "las-reverb", true);
        assert_eq!(fx_index(&st, "las-reverb"), Some(1));
        set_fx_on(&mut st, "las-delay", false);
        assert_eq!(st["fx"][0]["on"], false);
        // switching off one that is not there adds nothing
        set_fx_on(&mut st, "las-chorus", false);
        assert_eq!(st["fx"].as_array().unwrap().len(), 2);
        assert!(random_pick(&[4, 9], Some(4)).is_some());
        assert_eq!(random_pick(&[], None), None);
    }
}
