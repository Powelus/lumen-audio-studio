// SPDX-License-Identifier: GPL-3.0-or-later
//! The main window: the instruments (Facet, Mist), a keyboard and the meters.

use crate::audio::{Audio, Command, Instrument, Source};
use crate::widgets::{knob, meter, note_name, piano, ACCENT};
use eframe::egui::{self, Color32, RichText, Ui};
use las_dsp::fm::{default_fm_json, ALGORITHMS};
use las_dsp::mist::default_mist_json;
use las_dsp::patch::{default_engine_json, merge_patch};
use serde_json::{json, Value};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

/// Factory sounds, as differences from the defaults.
const FACET_PRESETS: [(&str, &str); 3] = [
    ("Init", "{}"),
    ("Bell", include_str!("../../../examples/facet-bell.json")),
    (
        "Electric Piano",
        include_str!("../../../examples/facet-electric-piano.json"),
    ),
];

const MIST_PRESETS: [(&str, &str); 2] = [
    ("Init", "{}"),
    ("Pad", include_str!("../../../examples/mist-pad.json")),
];

fn presets(i: Instrument) -> &'static [(&'static str, &'static str)] {
    match i {
        Instrument::Facet => &FACET_PRESETS,
        Instrument::Mist => &MIST_PRESETS,
    }
}

fn index(i: Instrument) -> usize {
    match i {
        Instrument::Facet => 0,
        Instrument::Mist => 1,
    }
}

/// Computer keys that play notes, as semitones above the keyboard's base note.
const KEY_MAP: [(egui::Key, u8); 17] = [
    (egui::Key::A, 0),
    (egui::Key::W, 1),
    (egui::Key::S, 2),
    (egui::Key::E, 3),
    (egui::Key::D, 4),
    (egui::Key::F, 5),
    (egui::Key::T, 6),
    (egui::Key::G, 7),
    (egui::Key::Y, 8),
    (egui::Key::H, 9),
    (egui::Key::U, 10),
    (egui::Key::J, 11),
    (egui::Key::K, 12),
    (egui::Key::O, 13),
    (egui::Key::L, 14),
    (egui::Key::P, 15),
    (egui::Key::Semicolon, 16),
];

pub struct App {
    audio: Option<Audio>,
    _midi: Option<midir::MidiInputConnection<()>>,
    status: String,
    midi_name: Option<String>,
    inst: Instrument,
    /// Defaults and the current patch of each instrument, Facet then Mist.
    defaults: [Value; 2],
    patches: [Value; 2],
    presets: [usize; 2],
    /// What Mist reads from: the built-in texture or a dropped file.
    source_name: String,
    volume: f64,
    /// Octave of the computer keyboard (the base note is `12 * (octave + 1)`).
    octave: i32,
    /// Keys held by the computer keyboard, with the note each started.
    typed: Vec<(egui::Key, u8)>,
    /// Key held with the mouse on the piano.
    clicked: Option<u8>,
    error: Option<String>,
}

fn full_defaults() -> [Value; 2] {
    [
        merge_patch(&default_engine_json(), &default_fm_json()),
        merge_patch(&default_engine_json(), &default_mist_json()),
    ]
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style(&cc.egui_ctx);
        let (audio, midi, status, midi_name) = match Audio::start() {
            Ok((audio, tx)) => {
                let status = format!("{} at {} Hz", audio.device, audio.sample_rate);
                let m = crate::midi::connect(tx);
                let name = m.as_ref().map(|(_, n)| n.clone());
                (Some(audio), m.map(|(c, _)| c), status, name)
            }
            Err(e) => (None, None, e, None),
        };
        let mut app = Self {
            audio,
            _midi: midi,
            status,
            midi_name,
            inst: Instrument::Facet,
            defaults: full_defaults(),
            patches: full_defaults(),
            presets: [0, 0],
            source_name: "built-in texture".into(),
            volume: 0.8,
            octave: 4,
            typed: Vec::new(),
            clicked: None,
            error: None,
        };
        app.load_preset(Instrument::Mist, 1);
        app.load_preset(Instrument::Facet, 1);
        // `lumen --mist` starts on Mist
        if std::env::args().any(|a| a == "--mist") {
            app.switch(Instrument::Mist);
        }
        app
    }

    fn load_preset(&mut self, inst: Instrument, i: usize) {
        let k = index(inst);
        let case: Value = serde_json::from_str(presets(inst)[i].1).unwrap_or_else(|_| json!({}));
        let over = case.get("patch").cloned().unwrap_or(case);
        self.patches[k] = merge_patch(&self.defaults[k], &over);
        self.presets[k] = i;
        self.push_patch(inst);
    }

    fn push_patch(&mut self, inst: Instrument) {
        if let Some(a) = &mut self.audio {
            self.error = a.set_patch(inst, &self.patches[index(inst)]).err();
        }
    }

    fn switch(&mut self, inst: Instrument) {
        if inst != self.inst {
            self.inst = inst;
            self.send(Command::Instrument(inst));
            // held keys belong to the old instrument
            self.typed.clear();
            self.clicked = None;
        }
    }

    /// A WAV file dropped on the window becomes Mist's source.
    fn load_wav(&mut self, path: &std::path::Path) -> Result<(), String> {
        let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 * scale))
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?
            }
        };
        let frames = samples.len() / channels;
        if frames < 4 {
            return Err("the file is too short".into());
        }
        let ch =
            |c: usize| -> Arc<[f32]> { (0..frames).map(|i| samples[i * channels + c]).collect() };
        let left = ch(0);
        let right = if channels > 1 { ch(1) } else { left.clone() };
        self.send(Command::Source(Source {
            left,
            right,
            rate: f64::from(spec.sample_rate),
        }));
        self.source_name = format!(
            "{} ({:.1} s)",
            path.file_name()
                .map_or_else(|| "sample".into(), |n| n.to_string_lossy()),
            frames as f64 / f64::from(spec.sample_rate)
        );
        Ok(())
    }

    fn send(&mut self, c: Command) {
        if let Some(a) = &mut self.audio {
            a.send(c);
        }
    }

    fn num(&self, path: &str) -> f64 {
        self.patches[index(self.inst)]
            .pointer(path)
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    }

    fn set(&mut self, path: &str, v: Value) {
        if let Some(t) = self.patches[index(self.inst)].pointer_mut(path) {
            *t = v;
        }
    }

    /// A knob bound to one number of the patch. Returns true on change.
    fn knob(
        &mut self,
        ui: &mut Ui,
        path: &str,
        label: &str,
        range: RangeInclusive<f64>,
        log: bool,
        show: &dyn Fn(f64) -> String,
    ) -> bool {
        let mut v = self.num(path);
        let d = self.defaults[index(self.inst)]
            .pointer(path)
            .and_then(Value::as_f64)
            .unwrap_or(v);
        let changed = knob(ui, &mut v, range, d, label, log, show);
        if changed {
            self.set(path, json!(v));
        }
        changed
    }

    fn toggle(&mut self, ui: &mut Ui, path: &str, label: &str) -> bool {
        let mut on = self.patches[index(self.inst)]
            .pointer(path)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let changed = ui.checkbox(&mut on, label).changed();
        if changed {
            self.set(path, json!(on));
        }
        changed
    }

    fn choice(&mut self, ui: &mut Ui, path: &str, id: &str, options: &[(&str, &str)]) -> bool {
        let cur = self.patches[index(self.inst)]
            .pointer(path)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let shown = options
            .iter()
            .find(|o| o.0 == cur)
            .map_or(cur.as_str(), |o| o.1);
        let mut changed = false;
        egui::ComboBox::from_id_salt(id)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (value, label) in options {
                    if ui.selectable_label(cur == *value, *label).clicked() {
                        self.set(path, json!(value));
                        changed = true;
                    }
                }
            });
        changed
    }

    fn envelope(&mut self, ui: &mut Ui, base: &str) -> bool {
        let secs = |v: f64| {
            if v < 1.0 {
                format!("{:.0} ms", v * 1000.0)
            } else {
                format!("{v:.2} s")
            }
        };
        let mut c = false;
        c |= self.knob(
            ui,
            &format!("{base}/a"),
            "Attack",
            0.001..=10.0,
            true,
            &secs,
        );
        c |= self.knob(ui, &format!("{base}/d"), "Decay", 0.001..=10.0, true, &secs);
        c |= self.knob(
            ui,
            &format!("{base}/s"),
            "Sustain",
            0.0..=1.0,
            false,
            &|v| format!("{:.0}%", v * 100.0),
        );
        c |= self.knob(
            ui,
            &format!("{base}/r"),
            "Release",
            0.001..=10.0,
            true,
            &secs,
        );
        c
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Lumen Audio Studio").color(ACCENT));
            ui.separator();
            for (inst, name) in [(Instrument::Facet, "Facet"), (Instrument::Mist, "Mist")] {
                if ui.selectable_label(self.inst == inst, name).clicked() {
                    self.switch(inst);
                }
            }
            ui.separator();
            ui.label("Preset");
            let inst = self.inst;
            let k = index(inst);
            let before = self.presets[k];
            let mut pick = before;
            egui::ComboBox::from_id_salt(("preset", k))
                .selected_text(presets(inst)[before].0)
                .show_ui(ui, |ui| {
                    for (i, (name, _)) in presets(inst).iter().enumerate() {
                        ui.selectable_value(&mut pick, i, *name);
                    }
                });
            if pick != before {
                self.load_preset(inst, pick);
            }
            ui.separator();
            let mut vol = self.volume;
            if knob(ui, &mut vol, 0.0..=1.0, 0.8, "Volume", false, &|v| {
                format!("{:.0}%", v * 100.0)
            }) {
                self.volume = vol;
                self.send(Command::Volume(vol as f32));
            }
            if let Some(a) = &self.audio {
                let (l, r) = a.meters.peaks();
                meter(ui, l, egui::vec2(8.0, 60.0));
                meter(ui, r, egui::vec2(8.0, 60.0));
                ui.separator();
                let load = a.meters.load();
                let colour = if load > 0.7 {
                    Color32::from_rgb(235, 70, 60)
                } else {
                    Color32::from_gray(200)
                };
                ui.vertical(|ui| {
                    ui.label(RichText::new(format!("DSP {:.1}%", load * 100.0)).color(colour));
                    ui.label(format!("Voices {}", a.meters.voices()));
                    ui.label(format!("Dropouts {}", a.meters.dropouts()));
                });
            }
        });
    }

    /// The parts both instruments share: amp envelope, filter and voices.
    fn common(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        ui.group(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Amp envelope").strong());
                ui.horizontal(|ui| changed |= self.envelope(ui, "/env/0"));
            });
        });
        ui.group(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    changed |= self.toggle(ui, "/filter/on", "Filter");
                    changed |= self.choice(
                        ui,
                        "/filter/type",
                        "ftype",
                        &[("lp", "Low pass"), ("bp", "Band pass"), ("hp", "High pass")],
                    );
                });
                ui.horizontal(|ui| {
                    changed |=
                        self.knob(ui, "/filter/cutoff", "Cutoff", 20.0..=18000.0, true, &|v| {
                            if v >= 1000.0 {
                                format!("{:.1} kHz", v / 1000.0)
                            } else {
                                format!("{v:.0} Hz")
                            }
                        });
                    changed |= self.knob(ui, "/filter/res", "Resonance", 0.0..=1.0, false, &|v| {
                        format!("{:.0}%", v * 100.0)
                    });
                    changed |=
                        self.knob(ui, "/filter/envAmt", "Env amt", -1.0..=1.0, false, &|v| {
                            format!("{:+.0}%", v * 100.0)
                        });
                });
            });
        });
        ui.group(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    changed |= self.toggle(ui, "/voice/mono", "Mono");
                    changed |= self.toggle(ui, "/voice/legato", "Legato");
                });
                ui.horizontal(|ui| {
                    changed |= self.knob(ui, "/voice/poly", "Voices", 1.0..=32.0, false, &|v| {
                        format!("{v:.0}")
                    });
                    changed |= self.knob(ui, "/voice/glide", "Glide", 0.0..=2.0, false, &|v| {
                        format!("{:.0} ms", v * 1000.0)
                    });
                    changed |= self.knob(ui, "/level", "Level", 0.0..=1.0, false, &|v| {
                        format!("{:.0}%", v * 100.0)
                    });
                });
            });
        });
        changed
    }

    fn synth(&mut self, ui: &mut Ui) {
        let mut changed = false;
        ui.horizontal_top(|ui| {
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Algorithm").strong());
                    let mut algo = self.num("/algo").round() as usize;
                    let before = algo;
                    egui::ComboBox::from_id_salt("algo")
                        .selected_text(ALGORITHMS[algo.min(7)].name)
                        .show_ui(ui, |ui| {
                            for (i, a) in ALGORITHMS.iter().enumerate() {
                                ui.selectable_value(&mut algo, i, a.name);
                            }
                        });
                    if algo != before {
                        self.set("/algo", json!(algo));
                        changed = true;
                    }
                    changed |= self.knob(ui, "/feedback", "Feedback", 0.0..=1.0, false, &|v| {
                        format!("{:.0}%", v * 100.0)
                    });
                });
            });
            changed |= self.common(ui);
        });
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            for i in 0..4 {
                changed |= self.operator(ui, i);
            }
        });
        if changed {
            self.push_patch(Instrument::Facet);
        }
    }

    fn mist_panel(&mut self, ui: &mut Ui) {
        let mut changed = false;
        let pct = |v: f64| format!("{:.0}%", v * 100.0);
        ui.horizontal_top(|ui| {
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Source").strong());
                    ui.label(RichText::new(&self.source_name).weak());
                    ui.label(RichText::new("Drop a .wav file on the window").weak());
                    ui.horizontal(|ui| {
                        changed |= self.toggle(ui, "/grain/keytrack", "Follow keys");
                    });
                    changed |=
                        self.knob(ui, "/grain/rootKey", "Root key", 0.0..=127.0, false, &|v| {
                            note_name(v.round().clamp(0.0, 127.0) as u8)
                        });
                });
            });
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Where").strong());
                    ui.horizontal(|ui| {
                        changed |=
                            self.knob(ui, "/grain/position", "Position", 0.0..=1.0, false, &pct);
                        changed |= self.knob(ui, "/grain/scan", "Scan", -1.0..=1.0, false, &|v| {
                            format!("{v:+.2}/s")
                        });
                        changed |= self.knob(ui, "/grain/spray", "Spray", 0.0..=0.5, false, &|v| {
                            format!("{:.0} ms", v * 1000.0)
                        });
                    });
                });
            });
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Grains").strong());
                    ui.horizontal(|ui| {
                        changed |= self.knob(ui, "/grain/size", "Size", 5.0..=500.0, true, &|v| {
                            format!("{v:.0} ms")
                        });
                        changed |=
                            self.knob(ui, "/grain/density", "Density", 1.0..=100.0, true, &|v| {
                                format!("{v:.0}/s")
                            });
                        changed |= self.knob(ui, "/grain/shape", "Shape", 0.0..=1.0, false, &pct);
                        changed |=
                            self.knob(ui, "/grain/reverse", "Reverse", 0.0..=1.0, false, &pct);
                    });
                });
            });
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Pitch and stereo").strong());
                    ui.horizontal(|ui| {
                        changed |=
                            self.knob(ui, "/grain/pitch", "Transpose", -24.0..=24.0, false, &|v| {
                                format!("{v:+.1} st")
                            });
                        changed |=
                            self.knob(ui, "/grain/pitchRand", "Random", 0.0..=12.0, false, &|v| {
                                format!("{v:.1} st")
                            });
                        changed |= self.knob(ui, "/grain/spread", "Spread", 0.0..=1.0, false, &pct);
                    });
                });
            });
        });
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            changed |= self.common(ui);
        });
        if changed {
            self.push_patch(Instrument::Mist);
        }
    }

    fn operator(&mut self, ui: &mut Ui, i: usize) -> bool {
        let base = format!("/ops/{i}");
        let mut changed = false;
        ui.group(|ui| {
            ui.set_width(250.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    changed |=
                        self.toggle(ui, &format!("{base}/on"), &format!("Operator {}", i + 1));
                    changed |= self.choice(
                        ui,
                        &format!("{base}/wave"),
                        &format!("wave{i}"),
                        &[
                            ("sine", "Sine"),
                            ("half", "Half sine"),
                            ("abs", "Abs sine"),
                            ("square", "Soft square"),
                        ],
                    );
                });
                ui.horizontal(|ui| {
                    let fixed = self.patches[index(self.inst)]
                        .pointer(&format!("{base}/fixed"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if fixed {
                        changed |= self.knob(
                            ui,
                            &format!("{base}/freq"),
                            "Freq",
                            20.0..=8000.0,
                            true,
                            &|v| format!("{v:.0} Hz"),
                        );
                    } else {
                        changed |= self.knob(
                            ui,
                            &format!("{base}/ratio"),
                            "Ratio",
                            0.25..=16.0,
                            true,
                            &|v| format!("{v:.2}"),
                        );
                    }
                    changed |= self.knob(
                        ui,
                        &format!("{base}/detune"),
                        "Detune",
                        -100.0..=100.0,
                        false,
                        &|v| format!("{v:+.0} ct"),
                    );
                    changed |= self.knob(
                        ui,
                        &format!("{base}/level"),
                        "Level",
                        0.0..=1.0,
                        false,
                        &|v| format!("{:.0}%", v * 100.0),
                    );
                    changed |= self.knob(
                        ui,
                        &format!("{base}/velSens"),
                        "Velocity",
                        0.0..=1.0,
                        false,
                        &|v| format!("{:.0}%", v * 100.0),
                    );
                });
                changed |= self.toggle(ui, &format!("{base}/fixed"), "Fixed frequency");
                ui.horizontal(|ui| changed |= self.envelope(ui, &format!("{base}/env")));
            });
        });
        changed
    }

    /// Computer keyboard: play notes, change octave. Mouse: the on-screen piano.
    fn keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            if let egui::Event::Key {
                key,
                pressed,
                repeat: false,
                modifiers,
                ..
            } = e
            {
                if modifiers.command || modifiers.alt {
                    continue;
                }
                if let Some((_, semis)) = KEY_MAP.iter().find(|(k, _)| *k == key) {
                    if pressed && !self.typed.iter().any(|(k, _)| *k == key) {
                        let note = (12 * (self.octave + 1) + *semis as i32).clamp(0, 127) as u8;
                        self.typed.push((key, note));
                        self.send(Command::NoteOn {
                            key: note,
                            vel: 0.8,
                        });
                    } else if !pressed {
                        if let Some(p) = self.typed.iter().position(|(k, _)| *k == key) {
                            let (_, note) = self.typed.remove(p);
                            self.send(Command::NoteOff { key: note });
                        }
                    }
                } else if pressed && key == egui::Key::Z {
                    self.octave = (self.octave - 1).max(0);
                } else if pressed && key == egui::Key::X {
                    self.octave = (self.octave + 1).min(8);
                }
            }
        }
    }

    fn keyboard(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "Computer keys A to ; play {}. Z and X change octave.",
                note_name((12 * (self.octave + 1)).clamp(0, 127) as u8)
            ));
            if let Some(m) = &self.midi_name {
                ui.label(format!("MIDI: {m}"));
            } else {
                ui.label("No MIDI keyboard found");
            }
        });
        let mut held: Vec<u8> = self.typed.iter().map(|(_, n)| *n).collect();
        held.extend(self.clicked);
        let pressed = piano(ui, 36, 96, &held);
        if pressed != self.clicked {
            if let Some(old) = self.clicked {
                self.send(Command::NoteOff { key: old });
            }
            if let Some(new) = pressed {
                self.send(Command::NoteOn { key: new, vel: 0.8 });
            }
            self.clicked = pressed;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.keys(ctx);
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for path in dropped {
            match self.load_wav(&path) {
                Ok(()) => {
                    self.error = None;
                    self.switch(Instrument::Mist);
                }
                Err(e) => self.error = Some(format!("{}: {e}", path.display())),
            }
        }
        if let Some(a) = &mut self.audio {
            a.collect_garbage();
        }
        // meters move on their own
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("keys").show(ui, |ui| {
            ui.add_space(4.0);
            self.keyboard(ui);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.audio.is_none() {
                    ui.label(
                        RichText::new(format!("No sound: {}", self.status))
                            .color(Color32::from_rgb(235, 70, 60)),
                    );
                } else {
                    ui.label(RichText::new(&self.status).weak());
                }
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(Color32::from_rgb(235, 70, 60)));
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::both().show(ui, |ui| match self.inst {
                Instrument::Facet => {
                    ui.label(RichText::new("Facet").size(20.0).strong());
                    ui.label(RichText::new("Four-operator FM synth").weak());
                    ui.add_space(6.0);
                    self.synth(ui);
                }
                Instrument::Mist => {
                    ui.label(RichText::new("Mist").size(20.0).strong());
                    ui.label(RichText::new("Granular synth").weak());
                    ui.add_space(6.0);
                    self.mist_panel(ui);
                }
            });
        });
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(24, 26, 31);
    v.window_fill = Color32::from_rgb(30, 33, 39);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke.color = Color32::BLACK;
    v.hyperlink_color = ACCENT;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}
