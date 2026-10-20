// SPDX-License-Identifier: GPL-3.0-or-later
//! The main window: the instruments (Facet, Mist), a keyboard and the meters.

use crate::audio::{Audio, Command, Instrument, Source};
use crate::presets::{self, Entry};
use crate::widgets::{knob, note_name, piano};
use eframe::egui::{self, RichText, Ui};
use las_dsp::fm::{default_fm_json, ALGORITHMS};
use las_dsp::mist::default_mist_json;
use las_dsp::patch::{default_engine_json, merge_patch};
use las_dsp::synth::{default_synth_json, mod_destinations, MOD_SOURCES};
use las_dsp::wavetable::table_ids;
use serde_json::{json, Value};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

/// The preset bank of each instrument.
pub(crate) fn bank_id(i: Instrument) -> &'static str {
    match i {
        Instrument::Facet => "facet",
        Instrument::Mist => "mist",
        Instrument::Polaris => "polaris",
    }
}

pub(crate) fn index(i: Instrument) -> usize {
    match i {
        Instrument::Facet => 0,
        Instrument::Mist => 1,
        Instrument::Polaris => 2,
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

/// One effect in an instrument's rack.
#[derive(Clone)]
pub struct FxItem {
    pub id: String,
    pub on: bool,
    pub mix: f64,
    /// The complete state (the defaults with what was saved on top).
    pub state: Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sound,
    Effects,
}

pub struct App {
    pub(crate) audio: Option<Audio>,
    pub(crate) _midi: Option<midir::MidiInputConnection<()>>,
    pub(crate) status: String,
    pub(crate) midi_name: Option<String>,
    pub(crate) inst: Instrument,
    /// Defaults and the current patch of each instrument: Facet, Mist, Polaris.
    pub(crate) defaults: [Value; 3],
    pub(crate) patches: [Value; 3],
    /// The preset lists (Init, factory, user) and the one each instrument is on.
    pub(crate) entries: [Vec<Entry>; 3],
    pub(crate) presets: [usize; 3],
    /// What is typed in the preset search box, and the name for a new preset.
    pub(crate) search: String,
    pub(crate) new_name: String,
    /// Which page of the Polaris panel is showing.
    pub(crate) tab: Tab,
    /// What Mist reads from: the built-in texture or a dropped file.
    pub(crate) source_name: String,
    pub(crate) volume: f64,
    /// Octave of the computer keyboard (the base note is `12 * (octave + 1)`).
    pub(crate) octave: i32,
    /// Keys held by the computer keyboard, with the note each started.
    pub(crate) typed: Vec<(egui::Key, u8)>,
    /// Key held with the mouse on the piano.
    pub(crate) clicked: Option<u8>,
    pub(crate) error: Option<String>,
    /// The effects of each instrument (Facet, Mist, Polaris).
    pub(crate) fx: [Vec<FxItem>; 3],
    /// Sound (the instrument) or Effects.
    pub(crate) view: View,
    /// Tempo for the effects that follow the beat.
    pub(crate) bpm: f64,
    /// The open project and its player.
    pub(crate) project: crate::project::ProjectState,
    pub(crate) shell: crate::shell::Shell,
    /// The channel whose instrument the synth window edits, if any.
    pub(crate) bound: Option<crate::inst::Bound>,
    /// The synth panels changed something that belongs in the bound channel.
    pub(crate) synth_dirty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Oscillators,
    Filters,
    Modulators,
    Matrix,
    Global,
}

fn full_defaults() -> [Value; 3] {
    [
        merge_patch(&default_engine_json(), &default_fm_json()),
        merge_patch(&default_engine_json(), &default_mist_json()),
        default_synth_json(),
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
            inst: Instrument::Polaris,
            defaults: full_defaults(),
            patches: full_defaults(),
            entries: [
                presets::entries(bank_id(Instrument::Facet), &full_defaults()[0]),
                presets::entries(bank_id(Instrument::Mist), &full_defaults()[1]),
                presets::entries(bank_id(Instrument::Polaris), &full_defaults()[2]),
            ],
            presets: [0, 0, 0],
            search: String::new(),
            new_name: String::new(),
            tab: Tab::Oscillators,
            fx: [Vec::new(), Vec::new(), Vec::new()],
            view: View::Sound,
            bpm: 120.0,
            source_name: "built-in texture".into(),
            volume: 0.8,
            octave: 4,
            typed: Vec::new(),
            clicked: None,
            error: None,
            project: Default::default(),
            shell: crate::shell::Shell::new(),
            bound: None,
            synth_dirty: false,
        };
        for inst in [Instrument::Polaris, Instrument::Mist, Instrument::Facet] {
            app.load_preset(inst, 1);
        }
        // `lumen --mist` or `--facet` start on that instrument (Polaris is the default)
        if std::env::args().any(|a| a == "--mist") {
            app.shell.open.synth = true;
            app.switch(Instrument::Mist);
        }
        if std::env::args().any(|a| a == "--facet") {
            app.shell.open.synth = true;
            app.switch(Instrument::Facet);
        }
        // `lumen song.las` opens a project
        if let Some(path) = std::env::args().skip(1).find(|a| a.ends_with(".las")) {
            app.open_project(&path);
        }
        // `--channel 1` opens that channel's instrument window
        if let Some(n) = std::env::args()
            .skip_while(|a| a != "--channel")
            .nth(1)
            .and_then(|n| n.parse::<usize>().ok())
        {
            app.open_channel(n);
        }
        // `--effect 1,0` opens the window of the effect in slot 0 of insert 1
        if let Some(v) = std::env::args().skip_while(|a| a != "--effect").nth(1) {
            let mut it = v.split(',').filter_map(|n| n.parse::<usize>().ok());
            if let (Some(i), Some(s)) = (it.next(), it.next()) {
                app.project.open_effect = Some((i, s));
            }
        }
        // `--open roll,mixer,synth,export` opens windows at the start
        if let Some(list) = std::env::args().skip_while(|a| a != "--open").nth(1) {
            for w in list.split(',') {
                let o = &mut app.shell.open;
                match w {
                    "rack" => o.rack = true,
                    "playlist" => o.playlist = true,
                    "roll" => o.roll = true,
                    "mixer" => o.mixer = true,
                    "synth" => o.synth = true,
                    "export" => o.export = true,
                    _ => {}
                }
            }
        }
        // `--close playlist` closes one that opens by default
        if let Some(list) = std::env::args().skip_while(|a| a != "--close").nth(1) {
            for w in list.split(',') {
                let o = &mut app.shell.open;
                match w {
                    "rack" => o.rack = false,
                    "playlist" => o.playlist = false,
                    "browser" => app.shell.browser = false,
                    _ => {}
                }
            }
        }
        // `--effects` opens the effects rack
        if std::env::args().any(|a| a == "--effects") {
            app.view = View::Effects;
            app.shell.open.synth = true;
        }
        app
    }

    fn load_preset(&mut self, inst: Instrument, i: usize) {
        let k = index(inst);
        let Some(e) = self.entries[k].get(i) else {
            return;
        };
        self.patches[k] = e.patch.clone();
        let fx = e.fx.clone();
        self.presets[k] = i;
        self.push_patch(inst);
        self.load_fx_list(inst, &fx);
    }

    /// Save the current patch as a user preset (replacing one of the same name).
    fn save_preset(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let k = index(self.inst);
        let patch = self.patches[k].clone();
        let fx: Vec<Value> = self.fx[k]
            .iter()
            .map(|f| json!({ "type": f.id, "on": f.on, "state": f.state }))
            .collect();
        let list = &mut self.entries[k];
        let at = match list.iter().position(|e| e.user && e.name == name) {
            Some(i) => {
                list[i].patch = patch;
                list[i].fx = fx;
                i
            }
            None => {
                list.push(Entry {
                    name: name.to_string(),
                    category: "User".into(),
                    patch,
                    user: true,
                    fx,
                });
                list.len() - 1
            }
        };
        self.presets[k] = at;
        self.error = presets::save_user(bank_id(self.inst), &self.entries[k]).err();
    }

    fn delete_preset(&mut self, i: usize) {
        let k = index(self.inst);
        if self.entries[k].get(i).is_some_and(|e| e.user) {
            self.entries[k].remove(i);
            if self.presets[k] >= i {
                self.presets[k] = self.presets[k].saturating_sub(1);
            }
            self.error = presets::save_user(bank_id(self.inst), &self.entries[k]).err();
        }
    }

    pub(crate) fn push_patch(&mut self, inst: Instrument) {
        self.synth_dirty = true;
        if let Some(a) = &mut self.audio {
            self.error = a.set_patch(inst, &self.patches[index(inst)]).err();
        }
    }

    pub(crate) fn switch(&mut self, inst: Instrument) {
        if inst != self.inst {
            self.inst = inst;
            self.send(Command::Instrument(inst));
            // held keys belong to the old instrument
            self.typed.clear();
            self.clicked = None;
        }
    }

    /// A WAV file dropped on the window becomes Mist's source.
    pub(crate) fn load_wav(&mut self, path: &std::path::Path) -> Result<(), String> {
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

    pub(crate) fn send(&mut self, c: Command) {
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

    /// Previous and next buttons and a searchable list of the presets.
    pub(crate) fn preset_picker(&mut self, ui: &mut Ui) {
        let inst = self.inst;
        let k = index(inst);
        let count = self.entries[k].len();
        let current = self.presets[k].min(count - 1);
        if ui.button("<").clicked() {
            self.load_preset(inst, (current + count - 1) % count);
        }
        let title = self.entries[k][current].name.clone();
        let mut pick = None;
        let mut delete = None;
        let mut save = None;
        ui.menu_button(format!("{title}  v"), |ui| {
            ui.set_min_width(300.0);
            ui.horizontal(|ui| {
                ui.label("Search");
                ui.text_edit_singleline(&mut self.search);
            });
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    let mut shown_category = "";
                    for (i, e) in self.entries[k].iter().enumerate() {
                        if !presets::matches(e, &self.search) {
                            continue;
                        }
                        if e.category != shown_category {
                            shown_category = &e.category;
                            ui.label(
                                RichText::new(shown_category)
                                    .strong()
                                    .color(crate::theme::accent()),
                            );
                        }
                        ui.horizontal(|ui| {
                            if ui.selectable_label(i == current, &e.name).clicked() {
                                pick = Some(i);
                                ui.close();
                            }
                            if e.user && ui.small_button("Delete").clicked() {
                                delete = Some(i);
                            }
                        });
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.new_name);
                if ui.button("Save as").clicked() {
                    save = Some(self.new_name.clone());
                }
            });
        });
        if ui.button(">").clicked() {
            self.load_preset(inst, (current + 1) % count);
        }
        if let Some(i) = pick {
            self.load_preset(inst, i);
        }
        if let Some(i) = delete {
            self.delete_preset(i);
        }
        if let Some(name) = save {
            self.save_preset(&name);
        }
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

    pub(crate) fn synth(&mut self, ui: &mut Ui) {
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

    pub(crate) fn mist_panel(&mut self, ui: &mut Ui) {
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

    pub(crate) fn polaris_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Tab::Oscillators, "Oscillators"),
                (Tab::Filters, "Filters"),
                (Tab::Modulators, "Envelopes and LFOs"),
                (Tab::Matrix, "Modulation"),
                (Tab::Global, "Voice and global"),
            ] {
                ui.selectable_value(&mut self.tab, tab, name);
            }
        });
        ui.add_space(6.0);
        let mut changed = false;
        match self.tab {
            Tab::Oscillators => changed |= self.oscillators(ui),
            Tab::Filters => changed |= self.filters(ui),
            Tab::Modulators => changed |= self.modulators(ui),
            Tab::Matrix => changed |= self.matrix(ui),
            Tab::Global => changed |= self.globals(ui),
        }
        if changed {
            self.push_patch(Instrument::Polaris);
        }
    }

    fn oscillators(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let pct = |v: f64| format!("{:.0}%", v * 100.0);
        let tables: Vec<(&str, &str)> = table_ids().map(|t| (t, t)).collect();
        let warps = [
            ("none", "No warp"),
            ("sync", "Sync"),
            ("bend+", "Bend +"),
            ("bend-", "Bend -"),
            ("pwm", "PWM"),
            ("mirror", "Mirror"),
            ("fm", "FM"),
            ("ring", "Ring"),
            ("quantize", "Quantize"),
            ("asym", "Asym"),
        ];
        let routes = [
            ("A", "Filter A"),
            ("B", "Filter B"),
            ("AB", "A + B"),
            ("direct", "Direct"),
        ];
        egui::Grid::new("oscs").num_columns(2).show(ui, |ui| {
            for o in 0..4 {
                let base = format!("/osc/{o}");
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            changed |= self.toggle(
                                ui,
                                &format!("{base}/on"),
                                &format!("Osc {}", ["A", "B", "C", "D"][o]),
                            );
                            changed |= self.choice(
                                ui,
                                &format!("{base}/table"),
                                &format!("tbl{o}"),
                                &tables,
                            );
                            changed |= self.choice(
                                ui,
                                &format!("{base}/warp"),
                                &format!("wrp{o}"),
                                &warps,
                            );
                            changed |= self.choice(
                                ui,
                                &format!("{base}/route"),
                                &format!("rte{o}"),
                                &routes,
                            );
                        });
                        ui.horizontal(|ui| {
                            changed |= self.knob(
                                ui,
                                &format!("{base}/pos"),
                                "WT pos",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/warpAmt"),
                                "Warp",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/level"),
                                "Level",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/pan"),
                                "Pan",
                                -1.0..=1.0,
                                false,
                                &|v| format!("{v:+.2}"),
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/oct"),
                                "Octave",
                                -3.0..=3.0,
                                false,
                                &|v| format!("{:+.0}", v.round()),
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/semi"),
                                "Semi",
                                -12.0..=12.0,
                                false,
                                &|v| format!("{:+.0}", v.round()),
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/fine"),
                                "Fine",
                                -100.0..=100.0,
                                false,
                                &|v| format!("{v:+.0} ct"),
                            );
                        });
                        ui.horizontal(|ui| {
                            changed |= self.knob(
                                ui,
                                &format!("{base}/voices"),
                                "Unison",
                                1.0..=16.0,
                                false,
                                &|v| format!("{:.0}", v.round()),
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/detune"),
                                "Detune",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/blend"),
                                "Blend",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/spread"),
                                "Spread",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/phase"),
                                "Phase",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/randPhase"),
                                "Rand ph.",
                                0.0..=1.0,
                                false,
                                &pct,
                            );
                        });
                    });
                });
                if o % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.horizontal_top(|ui| {
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        changed |= self.toggle(ui, "/sub/on", "Sub");
                        changed |= self.choice(
                            ui,
                            "/sub/wave",
                            "subwave",
                            &[
                                ("sine", "Sine"),
                                ("triangle", "Triangle"),
                                ("square", "Square"),
                                ("saw", "Saw"),
                            ],
                        );
                        changed |= self.choice(ui, "/sub/route", "subroute", &routes);
                    });
                    ui.horizontal(|ui| {
                        changed |= self.knob(ui, "/sub/level", "Level", 0.0..=1.0, false, &pct);
                        changed |= self.knob(ui, "/sub/oct", "Octave", -3.0..=0.0, false, &|v| {
                            format!("{:+.0}", v.round())
                        });
                    });
                });
            });
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        changed |= self.toggle(ui, "/noise/on", "Noise");
                        changed |= self.choice(
                            ui,
                            "/noise/color",
                            "ncolor",
                            &[("white", "White"), ("pink", "Pink")],
                        );
                        changed |= self.choice(ui, "/noise/route", "nroute", &routes);
                    });
                    changed |= self.knob(ui, "/noise/level", "Level", 0.0..=1.0, false, &pct);
                });
            });
        });
        changed
    }

    fn filters(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let pct = |v: f64| format!("{:.0}%", v * 100.0);
        let types = [
            ("lp12", "LP 12"),
            ("lp24", "LP 24"),
            ("ladder", "Ladder"),
            ("hp12", "HP 12"),
            ("hp24", "HP 24"),
            ("bp12", "BP 12"),
            ("bp24", "BP 24"),
            ("notch", "Notch"),
            ("peak", "Peak"),
            ("formant", "Formant"),
            ("comb", "Comb"),
        ];
        ui.horizontal(|ui| {
            ui.label("Routing");
            changed |= self.choice(
                ui,
                "/filterRouting",
                "frouting",
                &[("serial", "Serial"), ("parallel", "Parallel (A and B)")],
            );
        });
        for f in 0..2 {
            let base = format!("/filters/{f}");
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        changed |=
                            self.toggle(ui, &format!("{base}/on"), &format!("Filter {}", f + 1));
                        changed |=
                            self.choice(ui, &format!("{base}/type"), &format!("ftype{f}"), &types);
                    });
                    ui.horizontal(|ui| {
                        changed |= self.knob(
                            ui,
                            &format!("{base}/cutoff"),
                            "Cutoff",
                            16.0..=18000.0,
                            true,
                            &|v| {
                                if v >= 1000.0 {
                                    format!("{:.1} kHz", v / 1000.0)
                                } else {
                                    format!("{v:.0} Hz")
                                }
                            },
                        );
                        changed |= self.knob(
                            ui,
                            &format!("{base}/res"),
                            "Resonance",
                            0.0..=1.0,
                            false,
                            &pct,
                        );
                        changed |= self.knob(
                            ui,
                            &format!("{base}/drive"),
                            "Drive",
                            0.0..=1.0,
                            false,
                            &pct,
                        );
                        changed |= self.knob(
                            ui,
                            &format!("{base}/keytrack"),
                            "Key track",
                            -1.0..=1.0,
                            false,
                            &pct,
                        );
                        changed |= self.knob(
                            ui,
                            &format!("{base}/envAmt"),
                            "Env 2 amt",
                            -8.0..=8.0,
                            false,
                            &|v| format!("{v:+.1} oct"),
                        );
                        changed |= self.knob(
                            ui,
                            &format!("{base}/velAmt"),
                            "Velocity",
                            -8.0..=8.0,
                            false,
                            &|v| format!("{v:+.1} oct"),
                        );
                    });
                });
            });
        }
        changed
    }

    fn modulators(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let secs = |v: f64| {
            if v < 1.0 {
                format!("{:.0} ms", v * 1000.0)
            } else {
                format!("{v:.2} s")
            }
        };
        ui.label(RichText::new("Envelopes").strong());
        ui.horizontal_top(|ui| {
            for e in 0..4 {
                let base = format!("/env/{e}");
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.label(["Amp (env 1)", "Filter (env 2)", "Env 3", "Env 4"][e]);
                        ui.horizontal(|ui| {
                            changed |= self.knob(
                                ui,
                                &format!("{base}/a"),
                                "Attack",
                                0.001..=10.0,
                                true,
                                &secs,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/h"),
                                "Hold",
                                0.0..=5.0,
                                false,
                                &secs,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/d"),
                                "Decay",
                                0.001..=10.0,
                                true,
                                &secs,
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/s"),
                                "Sustain",
                                0.0..=1.0,
                                false,
                                &|v| format!("{:.0}%", v * 100.0),
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/r"),
                                "Release",
                                0.001..=10.0,
                                true,
                                &secs,
                            );
                        });
                    });
                });
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("LFOs").strong());
        let shapes = [
            ("sine", "Sine"),
            ("triangle", "Triangle"),
            ("saw", "Saw"),
            ("ramp", "Ramp"),
            ("square", "Square"),
            ("sh", "Sample & hold"),
            ("smooth", "Smooth random"),
        ];
        ui.horizontal_top(|ui| {
            for l in 0..4 {
                let base = format!("/lfo/{l}");
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("LFO {}", l + 1));
                            changed |= self.choice(
                                ui,
                                &format!("{base}/shape"),
                                &format!("lfoshape{l}"),
                                &shapes,
                            );
                        });
                        ui.horizontal(|ui| {
                            changed |= self.toggle(ui, &format!("{base}/sync"), "Sync");
                            changed |= self.toggle(ui, &format!("{base}/retrig"), "Retrigger");
                        });
                        ui.horizontal(|ui| {
                            let synced = self.patches[index(self.inst)]
                                .pointer(&format!("{base}/sync"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            if synced {
                                changed |= self.knob(
                                    ui,
                                    &format!("{base}/div"),
                                    "Beats",
                                    0.0625..=16.0,
                                    true,
                                    &|v| format!("{v:.2}"),
                                );
                            } else {
                                changed |= self.knob(
                                    ui,
                                    &format!("{base}/rate"),
                                    "Rate",
                                    0.02..=40.0,
                                    true,
                                    &|v| format!("{v:.2} Hz"),
                                );
                            }
                            changed |= self.knob(
                                ui,
                                &format!("{base}/phase"),
                                "Phase",
                                0.0..=1.0,
                                false,
                                &|v| format!("{:.0}%", v * 100.0),
                            );
                            changed |= self.knob(
                                ui,
                                &format!("{base}/delay"),
                                "Fade in",
                                0.0..=5.0,
                                false,
                                &secs,
                            );
                        });
                    });
                });
            }
        });
        changed
    }

    fn matrix(&mut self, ui: &mut Ui) -> bool {
        let k = index(self.inst);
        let dests = mod_destinations();
        let slots = self.patches[k]["mod"].as_array().map_or(0, Vec::len);
        let mut changed = false;
        let mut remove = None;
        ui.label(
            RichText::new("Each slot sends a source to a destination, scaled by its amount.")
                .weak(),
        );
        for i in 0..slots {
            let base = format!("/mod/{i}");
            ui.horizontal(|ui| {
                let src: Vec<(&str, &str)> = MOD_SOURCES.iter().map(|s| (*s, *s)).collect();
                let dst: Vec<(&str, &str)> =
                    dests.iter().map(|d| (d.0.as_str(), d.0.as_str())).collect();
                changed |= self.choice(ui, &format!("{base}/src"), &format!("msrc{i}"), &src);
                ui.label("to");
                changed |= self.choice(ui, &format!("{base}/dest"), &format!("mdst{i}"), &dst);
                let mut amt = self.num(&format!("{base}/amt"));
                let r = ui.add(egui::Slider::new(&mut amt, -1.0..=1.0).fixed_decimals(2));
                if r.changed() {
                    self.set(&format!("{base}/amt"), json!(amt));
                    changed = true;
                }
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            if let Some(a) = self.patches[k]["mod"].as_array_mut() {
                a.remove(i);
            }
            changed = true;
        }
        if slots < 32 && ui.button("Add a slot").clicked() {
            if let Some(a) = self.patches[k]["mod"].as_array_mut() {
                a.push(json!({ "src": "lfo1", "dest": "osc0.pos", "amt": 0.3 }));
            }
            changed = true;
        }
        changed
    }

    fn globals(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let pct = |v: f64| format!("{:.0}%", v * 100.0);
        ui.horizontal_top(|ui| {
            ui.group(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        changed |= self.toggle(ui, "/voice/mono", "Mono");
                        changed |= self.toggle(ui, "/voice/legato", "Legato");
                    });
                    ui.horizontal(|ui| {
                        changed |=
                            self.knob(ui, "/voice/poly", "Voices", 1.0..=32.0, false, &|v| {
                                format!("{v:.0}")
                            });
                        changed |= self.knob(ui, "/voice/glide", "Glide", 0.0..=2.0, false, &|v| {
                            format!("{:.0} ms", v * 1000.0)
                        });
                        changed |= self.knob(
                            ui,
                            "/voice/pbRange",
                            "Bend range",
                            0.0..=24.0,
                            false,
                            &|v| format!("{v:.0} st"),
                        );
                        changed |=
                            self.knob(ui, "/voice/velSens", "Vel. sens.", 0.0..=1.0, false, &pct);
                    });
                });
            });
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    changed |= self.knob(ui, "/level", "Level", 0.0..=1.0, false, &pct);
                    changed |= self.knob(ui, "/tune", "Tune", -12.0..=12.0, false, &|v| {
                        format!("{v:+.2} st")
                    });
                });
            });
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Macros (use them as sources in the modulation page)").strong());
        ui.horizontal(|ui| {
            for m in 0..8 {
                changed |= self.knob(
                    ui,
                    &format!("/macros/{m}"),
                    &format!("Macro {}", m + 1),
                    0.0..=1.0,
                    false,
                    &pct,
                );
            }
        });
        changed
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
    /// A key played from the screen or the computer keyboard: the project's selected
    /// channel while its page is open, otherwise the synth on the other pages.
    pub(crate) fn note(&mut self, key: u8, vel: f64, on: bool) {
        let c = if self.shell.keys_to_project() && self.project.has_player {
            Command::PlayerNote {
                channel: self.project.channel,
                key,
                vel,
                on,
            }
        } else if on {
            Command::NoteOn { key, vel }
        } else {
            Command::NoteOff { key }
        };
        self.send(c);
    }

    /// Save and undo, on the Project page.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (save, undo, redo) = ctx.input(|i| {
            let c = i.modifiers.command;
            (
                c && i.key_pressed(egui::Key::S),
                c && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                c && (i.key_pressed(egui::Key::Y)
                    || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
            )
        });
        if save {
            self.save_project();
        }
        let space =
            ctx.input(|i| i.key_pressed(egui::Key::Space)) && ctx.memory(|m| m.focused().is_none());
        if space {
            self.toggle_play();
        }
        if undo {
            self.project.undo();
        }
        if redo {
            self.project.redo();
        }
    }

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
                        self.note(note, 0.8, true);
                    } else if !pressed {
                        if let Some(p) = self.typed.iter().position(|(k, _)| *k == key) {
                            let (_, note) = self.typed.remove(p);
                            self.note(note, 0.0, false);
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

    pub(crate) fn keyboard(&mut self, ui: &mut Ui) {
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
                self.note(old, 0.0, false);
            }
            if let Some(new) = pressed {
                self.note(new, 0.8, true);
            }
            self.clicked = pressed;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.keys(ctx);
        self.shortcuts(ctx);
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        self.poll_project(ctx);
        self.sync_bound();
        for path in dropped {
            if path.extension().is_some_and(|e| e == "las") {
                self.open_project(&path.to_string_lossy());
                continue;
            }
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
        let ctx = ui.ctx().clone();
        self.paint_theme(&ctx);
        let p = crate::theme::pal();
        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(p.panel2)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| self.status_bar(ui));
        if self.shell.browser {
            egui::Panel::left("browser")
                .default_size(260.0)
                .size_range(200.0..=420.0)
                .frame(
                    egui::Frame::new()
                        .fill(p.panel)
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ui, |ui| self.browser(ui));
        }
        let area = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg))
            .show(ui, |ui| ui.max_rect())
            .inner;
        self.windows(&ctx, area);
    }
}

fn style(ctx: &egui::Context) {
    crate::theme::apply(ctx, "lumen");
}
