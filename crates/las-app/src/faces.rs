// SPDX-License-Identifier: GPL-3.0-or-later
//! The panels of Monolith, Zenith and TriOsc. They play on the Polaris engine but
//! each has its own layout: Monolith is a one page analogue strip between wooden
//! cheeks, Zenith is built around a big unison section and an XY pad, and TriOsc is
//! three compact oscillator rows with a small global strip.

use crate::app::{index, App, FxItem};
use crate::audio::Instrument;
use crate::widgets::{knob, knob_scale};
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke, Ui};
use las_dsp::fx::registry::full_state;
use serde_json::{json, Value};

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn secs(v: f64) -> String {
    if v < 1.0 {
        format!("{:.0} ms", v * 1000.0)
    } else {
        format!("{v:.2} s")
    }
}

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.1} kHz", v / 1000.0)
    } else {
        format!("{v:.0} Hz")
    }
}

fn skin() -> crate::skin::Skin {
    crate::skin::current().unwrap_or_else(|| crate::skin::skin_for("polaris"))
}

/// A small heading in the face's accent colour.
fn title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .strong()
            .size(11.5)
            .color(skin().accent),
    );
}

/// Classic oscillator shapes and the tables that play them.
const SHAPES: [(&str, &str); 5] = [
    ("saw", "Saw"),
    ("square", "Square"),
    ("triangle", "Tri"),
    ("sine", "Sine"),
    ("noise", "Noise"),
];

impl App {
    fn text_at(&self, path: &str) -> String {
        self.patches[index(self.inst)]
            .pointer(path)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn flag_at(&self, path: &str) -> bool {
        self.patches[index(self.inst)]
            .pointer(path)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// A row of buttons that set one text value of the patch.
    fn pick(&mut self, ui: &mut Ui, path: &str, options: &[(&str, &str)]) -> bool {
        let cur = self.text_at(path);
        let mut changed = false;
        for (value, label) in options {
            if ui.selectable_label(cur == *value, *label).clicked() && cur != *value {
                self.set(path, json!(value));
                changed = true;
            }
        }
        changed
    }

    /// Pick an oscillator's shape. A square gets the pulse width warp, the others none.
    fn shape_buttons(&mut self, ui: &mut Ui, o: usize, options: &[(&str, &str)]) -> bool {
        let base = format!("/osc/{o}");
        let cur = self.text_at(&format!("{base}/table"));
        let mut changed = false;
        for (value, label) in options {
            if ui.selectable_label(cur == *value, *label).clicked() && cur != *value {
                self.set(&format!("{base}/table"), json!(value));
                self.set(&format!("{base}/pos"), json!(0));
                let warp = if *value == "square" { "pwm" } else { "none" };
                self.set(&format!("{base}/warp"), json!(warp));
                changed = true;
            }
        }
        changed
    }

    /// The amount of the modulation slot from `src` to `dest` (0 when there is none).
    fn mod_amt(&self, src: &str, dest: &str) -> f64 {
        self.patches[index(self.inst)]["mod"]
            .as_array()
            .and_then(|l| {
                l.iter()
                    .find(|m| m["src"] == src && m["dest"] == dest)
                    .and_then(|m| m["amt"].as_f64())
            })
            .unwrap_or(0.0)
    }

    /// Set the slot from `src` to `dest`, adding it when needed and removing it at 0.
    fn set_mod(&mut self, src: &str, dest: &str, amt: f64) {
        let k = index(self.inst);
        if !self.patches[k]["mod"].is_array() {
            self.patches[k]["mod"] = json!([]);
        }
        let Some(list) = self.patches[k]["mod"].as_array_mut() else {
            return;
        };
        let at = list
            .iter()
            .position(|m| m["src"] == src && m["dest"] == dest);
        match (at, amt.abs() < 1e-4) {
            (Some(i), true) => {
                list.remove(i);
            }
            (Some(i), false) => list[i]["amt"] = json!(amt),
            (None, false) => list.push(json!({ "src": src, "dest": dest, "amt": amt })),
            (None, true) => {}
        }
    }

    /// A knob for one modulation slot.
    fn mod_knob(
        &mut self,
        ui: &mut Ui,
        src: &str,
        dest: &str,
        label: &str,
        range: std::ops::RangeInclusive<f64>,
        show: &dyn Fn(f64) -> String,
    ) -> bool {
        let mut v = self.mod_amt(src, dest);
        let changed = knob(ui, &mut v, range, 0.0, label, false, show);
        if changed {
            self.set_mod(src, dest, v);
        }
        changed
    }

    /// A value of an effect in the instrument's rack (`None` when it is not there).
    fn rack_value(&self, id: &str, key: &str) -> Option<f64> {
        self.fx[index(self.inst)]
            .iter()
            .find(|f| f.id == id && f.on)
            .and_then(|f| f.state[key].as_f64())
    }

    /// Set a value of an effect in the rack, adding the effect when it is missing.
    fn set_rack(&mut self, id: &str, key: &str, v: f64) {
        let inst = self.inst;
        let k = index(inst);
        match self.fx[k].iter().position(|f| f.id == id) {
            Some(slot) => {
                let on = self.fx[k][slot].on;
                self.fx[k][slot].state[key] = json!(v);
                if on {
                    self.push_fx(inst, slot);
                } else {
                    self.fx[k][slot].on = true;
                    self.rebuild_chain(inst);
                }
            }
            None => {
                self.fx[k].push(FxItem {
                    id: id.to_string(),
                    on: true,
                    mix: 1.0,
                    state: full_state(id, &json!({ key: v })),
                });
                self.rebuild_chain(inst);
            }
        }
    }

    /// A knob for one value of a rack effect; at zero it reads as off.
    fn rack_knob(
        &mut self,
        ui: &mut Ui,
        id: &str,
        key: &str,
        label: &str,
        range: std::ops::RangeInclusive<f64>,
        show: &dyn Fn(f64) -> String,
    ) {
        let lo = *range.start();
        // a missing effect shows its wet amount at zero and the rest at their defaults
        let missing = if matches!(key, "mix" | "wet") {
            lo
        } else {
            full_state(id, &json!({}))[key].as_f64().unwrap_or(lo)
        };
        let mut v = self.rack_value(id, key).unwrap_or(missing);
        if knob(ui, &mut v, range, lo, label, false, show) {
            self.set_rack(id, key, v);
        }
    }

    /// Four ADSR knobs with a drawing of the curve above them.
    fn adsr(&mut self, ui: &mut Ui, base: &str, width: f32) -> bool {
        let at = |s: &Self, n: &str| s.num(&format!("{base}/{n}"));
        let (a, d, s, r) = (at(self, "a"), at(self, "d"), at(self, "s"), at(self, "r"));
        adsr_curve(ui, vec2(width, 36.0), a, d, s, r);
        let mut c = false;
        ui.horizontal(|ui| {
            c |= self.knob(
                ui,
                &format!("{base}/a"),
                "Attack",
                0.001..=10.0,
                true,
                &secs,
            );
            c |= self.knob(ui, &format!("{base}/d"), "Decay", 0.001..=10.0, true, &secs);
            c |= self.knob(ui, &format!("{base}/s"), "Sustain", 0.0..=1.0, false, &pct);
            c |= self.knob(
                ui,
                &format!("{base}/r"),
                "Release",
                0.001..=10.0,
                true,
                &secs,
            );
        });
        c
    }

    /// Monolith: a virtual analogue laid out on one page like a classic hardware
    /// strip, controllers and oscillators and mixer on top, filter, contours, LFO and
    /// output below.
    pub(crate) fn monolith_panel(&mut self, ui: &mut Ui) {
        let cheek = 26.0;
        let mut c = false;
        let strip = ui.horizontal_top(|ui| {
            ui.add_space(cheek + 8.0);
            ui.vertical(|ui| {
                ui.add_space(6.0);
                knob_scale(0.82, || {
                    ui.horizontal_top(|ui| {
                        c |= self.mono_controllers(ui);
                        c |= self.mono_oscillators(ui);
                        c |= self.mono_mixer(ui);
                    });
                    ui.add_space(4.0);
                    ui.horizontal_top(|ui| {
                        c |= self.mono_filter(ui);
                        for (base, name) in
                            [("/env/1", "Filter contour"), ("/env/0", "Loudness contour")]
                        {
                            crate::skin::group(ui, |ui| {
                                ui.vertical(|ui| {
                                    title(ui, name);
                                    c |= self.adsr(ui, base, 176.0);
                                });
                            });
                        }
                        c |= self.mono_lfo(ui);
                    });
                });
                ui.add_space(6.0);
            });
            ui.add_space(cheek + 8.0);
        });
        let r = strip.response.rect;
        let p = ui.painter();
        wood(
            p,
            Rect::from_min_max(r.left_top(), pos2(r.left() + cheek, r.bottom())),
        );
        wood(
            p,
            Rect::from_min_max(pos2(r.right() - cheek, r.top()), r.right_bottom()),
        );
        if c {
            self.push_patch(Instrument::Polaris);
        }
    }

    fn mono_controllers(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Controllers");
                ui.horizontal(|ui| {
                    c |= self.knob(ui, "/voice/glide", "Glide", 0.0..=2.0, false, &|v| {
                        format!("{:.0} ms", v * 1000.0)
                    });
                    c |= self.knob(ui, "/voice/pbRange", "Bend", 0.0..=24.0, false, &|v| {
                        format!("{v:.0} st")
                    });
                });
                ui.horizontal(|ui| {
                    c |= self.knob(ui, "/tune", "Tune", -12.0..=12.0, false, &|v| {
                        format!("{v:+.2}")
                    });
                    c |= self.knob(ui, "/voice/poly", "Voices", 1.0..=16.0, false, &|v| {
                        format!("{:.0}", v.round())
                    });
                });
                c |= self.toggle(ui, "/voice/mono", "Mono");
                c |= self.toggle(ui, "/voice/legato", "Legato");
            });
        });
        c
    }

    fn mono_oscillators(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        let feet = [
            (-2.0, "32'"),
            (-1.0, "16'"),
            (0.0, "8'"),
            (1.0, "4'"),
            (2.0, "2'"),
        ];
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Oscillator bank");
                for o in 0..3 {
                    let base = format!("/osc/{o}");
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if o == 0 {
                                    ui.label(RichText::new("Osc 1").strong());
                                } else {
                                    c |= self.toggle(
                                        ui,
                                        &format!("{base}/on"),
                                        &format!("Osc {}", o + 1),
                                    );
                                }
                                c |= self.shape_buttons(ui, o, &SHAPES);
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Range").weak());
                                let oct = self.num(&format!("{base}/oct")).round();
                                for (v, name) in feet {
                                    if ui.selectable_label(oct == v, name).clicked() {
                                        self.set(&format!("{base}/oct"), json!(v));
                                        c = true;
                                    }
                                }
                            });
                        });
                        c |= self.knob(
                            ui,
                            &format!("{base}/fine"),
                            "Detune",
                            -50.0..=50.0,
                            false,
                            &|v| format!("{v:+.0} ct"),
                        );
                        c |= self.knob(
                            ui,
                            &format!("{base}/semi"),
                            "Interval",
                            -12.0..=12.0,
                            false,
                            &|v| format!("{:+.0} st", v.round()),
                        );
                        let square = self.text_at(&format!("{base}/table")) == "square";
                        ui.add_enabled_ui(square, |ui| {
                            // the pulse width warp narrows the pulse from 50% to 2%
                            c |= self.knob(
                                ui,
                                &format!("{base}/warpAmt"),
                                "Width",
                                0.0..=1.0,
                                false,
                                &|v| format!("{:.0}%", 50.0 * (1.0 - v * 0.96)),
                            );
                        });
                    });
                }
            });
        });
        c
    }

    fn mono_mixer(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Mixer");
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        for o in 0..3 {
                            c |= self.knob(
                                ui,
                                &format!("/osc/{o}/level"),
                                &format!("Osc {}", o + 1),
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                        }
                    });
                    ui.vertical(|ui| {
                        c |= self.toggle(ui, "/noise/on", "Noise");
                        c |= self.knob(ui, "/noise/level", "Noise", 0.0..=1.0, false, &pct);
                        ui.horizontal(|ui| {
                            c |= self.pick(
                                ui,
                                "/noise/color",
                                &[("white", "White"), ("pink", "Pink")],
                            );
                        });
                        ui.add_space(6.0);
                        c |= self.knob(ui, "/level", "Volume", 0.0..=1.0, false, &pct);
                    });
                });
            });
        });
        c
    }

    fn mono_filter(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Filter");
                ui.horizontal(|ui| {
                    c |= self.pick(
                        ui,
                        "/filters/0/type",
                        &[
                            ("ladder", "Ladder"),
                            ("lp24", "LP 24"),
                            ("lp12", "LP 12"),
                            ("bp12", "BP"),
                            ("hp12", "HP"),
                        ],
                    );
                });
                ui.horizontal_top(|ui| {
                    knob_scale(1.7, || {
                        c |=
                            self.knob(ui, "/filters/0/cutoff", "Cutoff", 20.0..=18000.0, true, &hz);
                    });
                    egui::Grid::new("mono-filter").show(ui, |ui| {
                        c |= self.knob(ui, "/filters/0/res", "Emphasis", 0.0..=1.0, false, &pct);
                        c |= self.knob(
                            ui,
                            "/filters/0/envAmt",
                            "Contour",
                            -8.0..=8.0,
                            false,
                            &|v| format!("{v:+.1} oct"),
                        );
                        ui.end_row();
                        c |= self.knob(
                            ui,
                            "/filters/0/keytrack",
                            "Key track",
                            0.0..=1.0,
                            false,
                            &pct,
                        );
                        c |= self.knob(ui, "/filters/0/drive", "Drive", 0.0..=1.0, false, &pct);
                        ui.end_row();
                    });
                });
            });
        });
        // the strip has one filter; it is always in the path
        if !self.flag_at("/filters/0/on") {
            self.set("/filters/0/on", json!(true));
            c = true;
        }
        c
    }

    fn mono_lfo(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Modulation");
                ui.horizontal(|ui| {
                    c |= self.pick(
                        ui,
                        "/lfo/0/shape",
                        &[
                            ("triangle", "Tri"),
                            ("square", "Sqr"),
                            ("saw", "Saw"),
                            ("sh", "S&H"),
                        ],
                    );
                });
                ui.horizontal(|ui| {
                    c |= self.knob(ui, "/lfo/0/rate", "Rate", 0.05..=30.0, true, &|v| {
                        format!("{v:.2} Hz")
                    });
                    c |= self.mod_knob(ui, "lfo1", "pitch", "Vibrato", 0.0..=0.05, &|v| {
                        format!("{:.0} ct", v * 2400.0)
                    });
                });
                ui.horizontal(|ui| {
                    c |= self.mod_knob(ui, "lfo1", "f0.cutoff", "Filter", 0.0..=0.5, &|v| {
                        format!("{:.1} oct", v * 8.0)
                    });
                    c |= self.mod_knob(ui, "mw", "pitch", "Wheel vib.", 0.0..=0.05, &|v| {
                        format!("{:.0} ct", v * 2400.0)
                    });
                });
            });
        });
        c
    }

    /// Zenith: supersaws and pads. A big unison section with the voices drawn
    /// fanned out, an XY pad on detune and brightness, the space effects, two
    /// layers and the envelopes.
    pub(crate) fn zenith_panel(&mut self, ui: &mut Ui) {
        let mut c = false;
        knob_scale(0.76, || {
            ui.horizontal_top(|ui| {
                c |= self.zen_unison(ui);
                c |= self.zen_xy(ui);
                self.zen_space(ui);
            });
            ui.add_space(2.0);
            ui.horizontal_top(|ui| {
                c |= self.zen_layer_a(ui);
                c |= self.zen_layer_b(ui);
            });
            ui.add_space(2.0);
            ui.horizontal_top(|ui| {
                for (base, name) in [("/env/0", "Amp envelope"), ("/env/1", "Filter envelope")] {
                    crate::skin::group(ui, |ui| {
                        ui.vertical(|ui| {
                            title(ui, name);
                            c |= self.adsr(ui, base, 196.0);
                        });
                    });
                }
                crate::skin::group(ui, |ui| {
                    ui.vertical(|ui| {
                        title(ui, "Voice");
                        ui.horizontal(|ui| {
                            c |= self.knob(ui, "/voice/poly", "Notes", 1.0..=16.0, false, &|v| {
                                format!("{:.0}", v.round())
                            });
                            c |= self.knob(ui, "/voice/glide", "Glide", 0.0..=2.0, false, &|v| {
                                format!("{:.0} ms", v * 1000.0)
                            });
                            c |= self.knob(ui, "/level", "Level", 0.0..=1.0, false, &pct);
                        });
                        ui.horizontal(|ui| {
                            c |= self.toggle(ui, "/voice/mono", "Mono");
                            c |= self.toggle(ui, "/voice/legato", "Legato");
                        });
                    });
                });
            });
        });
        if c {
            self.push_patch(Instrument::Polaris);
        }
    }

    /// Set a value on both oscillators of layer A (the stack and its octave).
    fn set_layer_a(&mut self, key: &str, v: f64) {
        for o in 0..2 {
            self.set(&format!("/osc/{o}/{key}"), json!(v));
        }
    }

    fn zen_unison(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Unison (layer A)");
                let voices = self.num("/osc/0/voices");
                let detune = self.num("/osc/0/detune");
                let spread = self.num("/osc/0/spread");
                let blend = self.num("/osc/0/blend");
                unison_fan(ui, vec2(330.0, 76.0), voices, detune, spread, blend);
                ui.horizontal(|ui| {
                    let rows: [(&str, &str, std::ops::RangeInclusive<f64>, f64); 4] = [
                        ("voices", "Voices", 1.0..=16.0, 1.0),
                        ("detune", "Detune", 0.0..=1.0, 0.2),
                        ("spread", "Width", 0.0..=1.0, 0.6),
                        ("blend", "Blend", 0.0..=1.0, 0.75),
                    ];
                    for (key, label, range, d) in rows {
                        let mut v = self.num(&format!("/osc/0/{key}"));
                        let show = |v: f64| {
                            if key == "voices" {
                                format!("{:.0}", v.round())
                            } else {
                                pct(v)
                            }
                        };
                        knob_scale(1.0, || {
                            if knob(ui, &mut v, range, d, label, false, &show) {
                                let v = if key == "voices" { v.round() } else { v };
                                self.set_layer_a(key, v);
                                c = true;
                            }
                        });
                    }
                });
            });
        });
        c
    }

    fn zen_xy(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Detune x Brightness");
                let (lo, hi) = (200f64, 18000f64);
                let cutoff = self.num("/filters/0/cutoff").clamp(lo, hi);
                let mut x = self.num("/osc/0/detune") as f32;
                let mut y = ((cutoff.ln() - lo.ln()) / (hi.ln() - lo.ln())) as f32;
                if xy_pad(ui, 132.0, &mut x, &mut y) {
                    self.set_layer_a("detune", f64::from(x));
                    let f = (lo.ln() + f64::from(y) * (hi.ln() - lo.ln())).exp();
                    self.set("/filters/0/cutoff", json!(f.round()));
                    self.set("/filters/0/on", json!(true));
                    c = true;
                }
                ui.label(
                    RichText::new(format!(
                        "Detune {}   Cutoff {}",
                        pct(f64::from(x)),
                        hz(cutoff)
                    ))
                    .small()
                    .color(skin().dim),
                );
            });
        });
        c
    }

    fn zen_space(&mut self, ui: &mut Ui) {
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Space");
                ui.horizontal(|ui| {
                    self.rack_knob(ui, "las-chorus", "mix", "Chorus", 0.0..=1.0, &pct);
                    self.rack_knob(ui, "las-chorus", "rate", "Ch. rate", 0.0..=5.0, &|v| {
                        format!("{v:.2} Hz")
                    });
                    self.rack_knob(ui, "las-delay", "wet", "Echo", 0.0..=1.0, &pct);
                });
                ui.horizontal(|ui| {
                    self.rack_knob(ui, "las-reverb", "wet", "Reverb", 0.0..=1.0, &pct);
                    self.rack_knob(ui, "las-reverb", "decay", "Size", 0.0..=10.0, &|v| {
                        format!("{v:.1} s")
                    });
                });
            });
            ui.label(
                RichText::new("These set the chorus, echo\nand reverb in the rack")
                    .small()
                    .weak(),
            );
        });
    }

    fn zen_layer_a(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Layer A: saw stack");
                ui.horizontal(|ui| {
                    let cur = self.text_at("/osc/0/table");
                    for (t, name) in [
                        ("saw", "Saw"),
                        ("analog", "Analog"),
                        ("square", "Square"),
                        ("reese", "Reese"),
                    ] {
                        if ui.selectable_label(cur == t, name).clicked() {
                            for o in 0..2 {
                                self.set(&format!("/osc/{o}/table"), json!(t));
                            }
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    c |= self.knob(ui, "/osc/0/level", "Stack", 0.0..=1.0, false, &pct);
                    c |= self.knob(ui, "/osc/0/oct", "Octave", -2.0..=2.0, false, &|v| {
                        format!("{:+.0}", v.round())
                    });
                    ui.vertical(|ui| {
                        c |= self.toggle(ui, "/osc/1/on", "Octave stack");
                        ui.horizontal(|ui| {
                            let o = self.num("/osc/1/oct").round();
                            for (v, n) in [(-1.0, "-1"), (1.0, "+1"), (2.0, "+2")] {
                                if ui.selectable_label(o == v, n).clicked() {
                                    self.set("/osc/1/oct", json!(v));
                                    c = true;
                                }
                            }
                        });
                    });
                    c |= self.knob(ui, "/osc/1/level", "Oct level", 0.0..=1.0, false, &pct);
                });
            });
        });
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                title(ui, "Tone");
                ui.horizontal(|ui| {
                    c |= self.pick(
                        ui,
                        "/filters/0/type",
                        &[
                            ("lp24", "LP 24"),
                            ("lp12", "LP 12"),
                            ("bp12", "BP"),
                            ("hp12", "HP"),
                        ],
                    );
                });
                ui.horizontal(|ui| {
                    c |= self.knob(ui, "/filters/0/res", "Resonance", 0.0..=1.0, false, &pct);
                    c |= self.knob(
                        ui,
                        "/filters/0/envAmt",
                        "Env amt",
                        -8.0..=8.0,
                        false,
                        &|v| format!("{v:+.1} oct"),
                    );
                    c |= self.knob(ui, "/filters/1/cutoff", "Low cut", 20.0..=2000.0, true, &hz);
                });
            });
        });
        c
    }

    fn zen_layer_b(&mut self, ui: &mut Ui) -> bool {
        let mut c = false;
        crate::skin::group(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    c |= self.toggle(ui, "/osc/2/on", "");
                    title(ui, "Layer B: pad and sub");
                });
                ui.horizontal(|ui| {
                    c |= self.shape_buttons(
                        ui,
                        2,
                        &[
                            ("choir", "Choir"),
                            ("harmonics", "Glass"),
                            ("organ", "Organ"),
                            ("pwm", "PWM"),
                            ("sine", "Sine"),
                        ],
                    );
                });
                ui.horizontal(|ui| {
                    c |= self.knob(ui, "/osc/2/level", "Pad", 0.0..=1.0, false, &pct);
                    c |= self.knob(ui, "/osc/2/pos", "Colour", 0.0..=1.0, false, &pct);
                    c |= self.knob(ui, "/osc/2/oct", "Octave", -2.0..=2.0, false, &|v| {
                        format!("{:+.0}", v.round())
                    });
                    c |= self.knob(ui, "/osc/2/detune", "Drift", 0.0..=1.0, false, &pct);
                    ui.separator();
                    ui.vertical(|ui| {
                        c |= self.toggle(ui, "/sub/on", "Sub");
                        ui.horizontal(|ui| {
                            c |= self.pick(
                                ui,
                                "/sub/wave",
                                &[("sine", "Sine"), ("triangle", "Tri")],
                            );
                        });
                    });
                    c |= self.knob(ui, "/sub/level", "Sub", 0.0..=1.0, false, &pct);
                    c |= self.knob(ui, "/sub/oct", "Sub oct", -2.0..=0.0, false, &|v| {
                        format!("{:+.0}", v.round())
                    });
                });
            });
        });
        c
    }

    /// TriOsc: three oscillator rows (shape, pan, coarse and fine pitch, volume,
    /// phase, stereo detune) and a small strip with filter, envelope and output.
    pub(crate) fn triosc_panel(&mut self, ui: &mut Ui) {
        let mut c = false;
        let shapes = [
            ("sine", "Sin"),
            ("triangle", "Tri"),
            ("square", "Sqr"),
            ("saw", "Saw"),
            ("noise", "Rnd"),
        ];
        knob_scale(0.72, || {
            for o in 0..3 {
                let base = format!("/osc/{o}");
                crate::skin::group(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            c |= self.toggle(ui, &format!("{base}/on"), &format!("OSC {}", o + 1));
                            ui.horizontal(|ui| {
                                c |= self.shape_buttons(ui, o, &shapes);
                            });
                        });
                        c |=
                            self.knob(ui, &format!("{base}/pan"), "Pan", -1.0..=1.0, false, &|v| {
                                format!("{v:+.2}")
                            });
                        // coarse is octave and semitones together
                        let mut coarse = self.num(&format!("{base}/oct")) * 12.0
                            + self.num(&format!("{base}/semi"));
                        if knob(ui, &mut coarse, -24.0..=24.0, 0.0, "Coarse", false, &|v| {
                            format!("{:+.0}", v.round())
                        }) {
                            let c2 = coarse.round();
                            let oct = (c2 / 12.0).trunc();
                            self.set(&format!("{base}/oct"), json!(oct));
                            self.set(&format!("{base}/semi"), json!(c2 - oct * 12.0));
                            c = true;
                        }
                        c |= self.knob(
                            ui,
                            &format!("{base}/fine"),
                            "Fine",
                            -100.0..=100.0,
                            false,
                            &|v| format!("{v:+.0}"),
                        );
                        c |= self.knob(ui, &format!("{base}/level"), "Vol", 0.0..=1.0, false, &pct);
                        // a fixed start phase needs the random phase off
                        let mut ph = self.num(&format!("{base}/phase"));
                        if knob(ui, &mut ph, 0.0..=1.0, 0.0, "Phase", false, &|v| {
                            format!("{:.0}°", v * 360.0)
                        }) {
                            self.set(&format!("{base}/phase"), json!(ph));
                            self.set(&format!("{base}/randPhase"), json!(0));
                            c = true;
                        }
                        // stereo detune: two voices, one per side
                        let stacked = self.num(&format!("{base}/voices")) >= 2.0;
                        let mut sd = if stacked {
                            self.num(&format!("{base}/detune"))
                        } else {
                            0.0
                        };
                        if knob(ui, &mut sd, 0.0..=1.0, 0.0, "St. det", false, &pct) {
                            let on = sd > 0.005;
                            self.set(&format!("{base}/voices"), json!(if on { 2 } else { 1 }));
                            self.set(&format!("{base}/detune"), json!(sd));
                            self.set(&format!("{base}/spread"), json!(1));
                            self.set(&format!("{base}/blend"), json!(1));
                            c = true;
                        }
                    });
                });
            }
            ui.add_space(2.0);
            crate::skin::group(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        c |= self.toggle(ui, "/filters/0/on", "Filter");
                        ui.horizontal(|ui| {
                            c |= self.pick(
                                ui,
                                "/filters/0/type",
                                &[("lp24", "LP"), ("hp12", "HP"), ("bp12", "BP")],
                            );
                        });
                    });
                    c |= self.knob(ui, "/filters/0/cutoff", "Cut", 20.0..=18000.0, true, &hz);
                    c |= self.knob(ui, "/filters/0/res", "Res", 0.0..=1.0, false, &pct);
                    ui.separator();
                    c |= self.knob(ui, "/env/0/a", "Att", 0.001..=10.0, true, &secs);
                    c |= self.knob(ui, "/env/0/d", "Dec", 0.001..=10.0, true, &secs);
                    c |= self.knob(ui, "/env/0/s", "Sus", 0.0..=1.0, false, &pct);
                    c |= self.knob(ui, "/env/0/r", "Rel", 0.001..=10.0, true, &secs);
                    ui.separator();
                    c |= self.knob(ui, "/voice/glide", "Porta", 0.0..=1.0, false, &|v| {
                        format!("{:.0} ms", v * 1000.0)
                    });
                    c |= self.knob(ui, "/level", "Master", 0.0..=1.0, false, &pct);
                    c |= self.toggle(ui, "/voice/mono", "Mono");
                });
            });
        });
        if c {
            self.push_patch(Instrument::Polaris);
        }
    }
}

/// A wooden side cheek: warm brown with darker grain lines and a bevel.
fn wood(p: &egui::Painter, r: Rect) {
    p.rect_filled(r, 3.0, Color32::from_rgb(112, 62, 30));
    let lines = 7;
    for i in 0..lines {
        let x0 = r.left() + r.width() * (i as f32 + 0.5) / lines as f32;
        let pts: Vec<Pos2> = (0..=40)
            .map(|j| {
                let t = j as f32 / 40.0;
                let y = r.top() + t * r.height();
                let wobble =
                    (t * 9.0 + i as f32 * 1.7).sin() * 1.6 + (t * 23.0 + i as f32).sin() * 0.6;
                pos2(x0 + wobble, y)
            })
            .collect();
        let shade = if i % 2 == 0 { 70 } else { 45 };
        p.add(egui::Shape::line(
            pts,
            Stroke::new(1.0, Color32::from_black_alpha(shade)),
        ));
    }
    p.line_segment(
        [
            r.left_top() + vec2(1.0, 2.0),
            r.left_bottom() + vec2(1.0, -2.0),
        ],
        Stroke::new(1.5, Color32::from_white_alpha(40)),
    );
    p.line_segment(
        [
            r.right_top() + vec2(-1.0, 2.0),
            r.right_bottom() + vec2(-1.0, -2.0),
        ],
        Stroke::new(1.5, Color32::from_black_alpha(110)),
    );
}

/// A small drawing of an ADSR envelope (times drawn on a square root scale).
fn adsr_curve(ui: &mut Ui, size: egui::Vec2, a: f64, d: f64, s: f64, r: f64) {
    let sk = skin();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, sk.screen);
    let (wa, wd, wr) = (a.max(0.0).sqrt(), d.max(0.0).sqrt(), r.max(0.0).sqrt());
    let hold = 0.6;
    let total = (wa + wd + hold + wr).max(1e-6);
    let inner = rect.shrink(4.0);
    let x = |t: f64| inner.left() + (t / total) as f32 * inner.width();
    let y = |v: f64| inner.bottom() - v.clamp(0.0, 1.0) as f32 * inner.height();
    let pts = vec![
        pos2(x(0.0), y(0.0)),
        pos2(x(wa), y(1.0)),
        pos2(x(wa + wd), y(s)),
        pos2(x(wa + wd + hold), y(s)),
        pos2(x(total), y(0.0)),
    ];
    for w in pts.windows(2) {
        let quad = vec![w[0], w[1], pos2(w[1].x, y(0.0)), pos2(w[0].x, y(0.0))];
        p.add(egui::Shape::convex_polygon(
            quad,
            sk.trace.gamma_multiply(0.12),
            Stroke::NONE,
        ));
    }
    p.add(egui::Shape::line(pts, Stroke::new(1.6, sk.trace)));
}

/// The unison voices fanned out: each voice a line from a common root, placed by
/// its detune (across) and pan (lean), the side voices dimmed by the blend.
fn unison_fan(ui: &mut Ui, size: egui::Vec2, voices: f64, detune: f64, spread: f64, blend: f64) {
    let sk = skin();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, sk.screen);
    for i in 1..8 {
        let x = rect.left() + rect.width() * i as f32 / 8.0;
        p.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, sk.trace.gamma_multiply(0.06)),
        );
    }
    let n = voices.round().clamp(1.0, 16.0) as usize;
    let root = pos2(rect.center().x, rect.bottom() - 8.0);
    for u in 0..n {
        let t = if n == 1 {
            0.0
        } else {
            u as f32 / (n - 1) as f32 * 2.0 - 1.0
        };
        let off = t * detune as f32;
        let lean = t * spread as f32;
        let tip = pos2(
            rect.center().x + off * rect.width() * 0.44,
            rect.top() + 14.0 + (1.0 - (1.0 - t.abs()) * 0.3) * 10.0,
        );
        let mid = pos2(
            root.x + lean * rect.width() * 0.2,
            root.y - (root.y - tip.y) * 0.5,
        );
        let side = if t.abs() < 1e-3 { 1.0 } else { blend as f32 };
        let col = sk
            .accent
            .lerp_to_gamma(sk.trace, (t + 1.0) * 0.5)
            .gamma_multiply(0.35 + 0.65 * side);
        let line = vec![root, mid, tip];
        p.add(egui::Shape::line(
            line.clone(),
            Stroke::new(5.0, col.gamma_multiply(0.2)),
        ));
        p.add(egui::Shape::line(line, Stroke::new(1.6, col)));
        p.circle_filled(tip, 3.0 + 2.0 * side, col);
    }
    p.circle_filled(root, 4.0, sk.text);
    p.text(
        rect.left_top() + vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        format!("{n} voices"),
        egui::FontId::proportional(11.0),
        sk.dim,
    );
}

/// A square pad that sets two values from 0 to 1 by dragging a dot. True on change.
fn xy_pad(ui: &mut Ui, side: f32, x: &mut f32, y: &mut f32) -> bool {
    let sk = skin();
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(at) = resp.interact_pointer_pos() {
            *x = ((at.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            *y = ((rect.bottom() - at.y) / rect.height()).clamp(0.0, 1.0);
            changed = true;
        }
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, sk.screen);
    for i in 1..8 {
        let f = i as f32 / 8.0;
        let col = Stroke::new(1.0, sk.accent.gamma_multiply(0.12));
        p.line_segment(
            [
                pos2(rect.left() + f * side, rect.top()),
                pos2(rect.left() + f * side, rect.bottom()),
            ],
            col,
        );
        p.line_segment(
            [
                pos2(rect.left(), rect.top() + f * side),
                pos2(rect.right(), rect.top() + f * side),
            ],
            col,
        );
    }
    let dot = pos2(rect.left() + *x * side, rect.bottom() - *y * side);
    let cross = Stroke::new(1.0, sk.trace.gamma_multiply(0.5));
    p.line_segment([pos2(dot.x, rect.top()), pos2(dot.x, rect.bottom())], cross);
    p.line_segment([pos2(rect.left(), dot.y), pos2(rect.right(), dot.y)], cross);
    p.circle_filled(dot, 14.0, sk.accent.gamma_multiply(0.2));
    p.circle_filled(dot, 7.0, sk.accent);
    p.circle_stroke(dot, 10.0, Stroke::new(1.5, sk.trace));
    p.text(
        rect.right_bottom() - vec2(6.0, 6.0),
        egui::Align2::RIGHT_BOTTOM,
        "detune",
        egui::FontId::proportional(10.0),
        sk.dim,
    );
    p.text(
        rect.left_top() + vec2(6.0, 6.0),
        egui::Align2::LEFT_TOP,
        "bright",
        egui::FontId::proportional(10.0),
        sk.dim,
    );
    resp.on_hover_text("Drag: across sets the detune, up makes it brighter");
    changed
}
