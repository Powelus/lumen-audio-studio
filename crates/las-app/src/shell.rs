// SPDX-License-Identifier: GPL-3.0-or-later
//! The window around everything, laid out like the web version: a top bar with the
//! transport and its displays, a browser on the left, floating windows for the
//! editors, and a status bar.

use crate::app::App;
use crate::audio::{Command, Instrument, Preview};
use crate::mixer;
use crate::steps::{self, StepState};
use crate::theme::{self, pal};
use crate::widgets::{knob, meter};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui};
use las_engine::factory::FACTORY_SAMPLES;
use las_engine::{Mode, PlayMode, RenderOptions};
use las_project::{Pattern, PPQ};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BrowserTab {
    Samples,
    Plugins,
    Presets,
    Project,
}

/// Which windows are open.
#[derive(Clone, Copy)]
pub struct Windows {
    pub rack: bool,
    pub playlist: bool,
    pub roll: bool,
    pub mixer: bool,
    pub synth: bool,
    pub export: bool,
    pub about: bool,
}

/// What the export window holds.
pub struct Export {
    pub path: String,
    pub format: usize,
    pub normalize: bool,
    pub pattern_only: bool,
    working: Option<Receiver<Result<String, String>>>,
    pub message: Option<Result<String, String>>,
}

/// The formats the export window offers: label, FLAC, bits.
pub const FORMATS: [(&str, bool, u32); 5] = [
    ("WAV 16 bit", false, 16),
    ("WAV 24 bit", false, 24),
    ("WAV 32 bit float", false, 32),
    ("FLAC 16 bit", true, 16),
    ("FLAC 24 bit", true, 24),
];

pub struct Shell {
    pub tab: BrowserTab,
    pub browser: bool,
    pub open: Windows,
    /// Song mode plays the playlist; pattern mode loops the chosen pattern.
    pub song_mode: bool,
    pub metronome: bool,
    pub search: String,
    /// The theme chosen ("system" or an id) and the one painted.
    pub theme: String,
    applied: String,
    taps: Vec<Instant>,
    pub steps: StepState,
    pub export: Export,
    pub status: String,
    /// An action waiting for an answer about unsaved changes.
    pub pending: Option<crate::menus::Action>,
    placed: bool,
}

impl Shell {
    pub fn new() -> Self {
        Self {
            tab: BrowserTab::Samples,
            browser: true,
            open: Windows {
                rack: true,
                playlist: true,
                roll: false,
                mixer: false,
                synth: false,
                export: false,
                about: false,
            },
            song_mode: true,
            metronome: false,
            search: String::new(),
            theme: theme::saved_choice(),
            applied: String::new(),
            taps: Vec::new(),
            steps: StepState::default(),
            export: Export {
                path: String::new(),
                format: 0,
                normalize: false,
                pattern_only: false,
                working: None,
                message: None,
            },
            status: "Ready".into(),
            pending: None,
            placed: false,
        }
    }

    /// Whether keys played on the computer keyboard go to the project's channel.
    pub fn keys_to_project(&self) -> bool {
        !self.open.synth
    }
}

/// The tempo that a series of taps makes, from the last four intervals.
pub fn tap_bpm(taps: &[Instant]) -> Option<f64> {
    if taps.len() < 2 {
        return None;
    }
    let recent = &taps[taps.len().saturating_sub(5)..];
    let span = recent
        .last()?
        .duration_since(*recent.first()?)
        .as_secs_f64();
    let bpm = 60.0 * (recent.len() - 1) as f64 / span.max(1e-6);
    (30.0..=300.0)
        .contains(&bpm)
        .then_some((bpm * 100.0).round() / 100.0)
}

/// `BAR:BEAT:STEP`, as the web version's display shows it (two digits for beat and step).
pub fn bar_beat_step(tick: f64, sig: (u32, u32)) -> String {
    let beat = f64::from(PPQ) * 4.0 / f64::from(sig.1.max(1));
    let bar = beat * f64::from(sig.0.max(1));
    let b = (tick / bar).floor().max(0.0);
    let rest = tick - b * bar;
    let bt = (rest / beat).floor();
    let step = ((rest - bt * beat) / f64::from(las_project::TICKS_PER_STEP)).floor();
    format!(
        "{}:{:02}:{:02}",
        b as i64 + 1,
        bt as i64 + 1,
        step as i64 + 1
    )
}

/// The factory samples whose name or folder contains the search text.
pub fn matching_samples(query: &str) -> Vec<&'static las_engine::factory::FactorySample> {
    let q = query.trim().to_lowercase();
    FACTORY_SAMPLES
        .iter()
        .filter(|s| {
            q.is_empty()
                || s.name.to_lowercase().contains(&q)
                || s.folder.to_lowercase().contains(&q)
        })
        .collect()
}

/// The folders of the factory kit, in order of first appearance.
pub fn sample_folders() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for s in FACTORY_SAMPLES {
        if !v.contains(&s.folder) {
            v.push(s.folder);
        }
    }
    v
}

/// A tag for a factory sample: its id without the prefix, up to the first dash.
fn tag(id: &str) -> &str {
    id.trim_start_matches("factory:")
        .split(['-', ':'])
        .next()
        .unwrap_or("")
}

/// A segmented toggle group. Returns the clicked index.
fn segmented(ui: &mut Ui, labels: &[&str], active: &[bool]) -> Option<usize> {
    let p = pal();
    let mut hit = None;
    egui::Frame::new()
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(8.0)
        .inner_margin(2.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
            ui.horizontal(|ui| {
                for (i, l) in labels.iter().enumerate() {
                    let on = active[i];
                    let text = RichText::new(*l).size(12.0).strong().color(if on {
                        p.on_accent
                    } else {
                        p.text2
                    });
                    let b = egui::Button::new(text)
                        .fill(if on { p.accent } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(6.0)
                        .min_size(vec2(0.0, 26.0));
                    if ui.add(b).clicked() {
                        hit = Some(i);
                    }
                }
            });
        });
    hit
}

enum Icon {
    Play,
    Stop,
    Metronome,
}

fn icon_button(ui: &mut Ui, icon: Icon, active: bool, tip: &str) -> egui::Response {
    let p = pal();
    let (r, resp) = ui.allocate_exact_size(vec2(34.0, 30.0), Sense::click());
    let bg = if active {
        p.accent
    } else if resp.hovered() {
        p.raised2
    } else {
        p.btn
    };
    ui.painter().rect_filled(r, 7.0, bg);
    ui.painter()
        .rect_stroke(r, 7.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let ink = if active { p.on_accent } else { p.text };
    let c = r.center();
    match icon {
        Icon::Play => {
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    pos2(c.x - 4.5, c.y - 6.5),
                    pos2(c.x - 4.5, c.y + 6.5),
                    pos2(c.x + 6.5, c.y),
                ],
                ink,
                Stroke::NONE,
            ));
        }
        Icon::Stop => {
            ui.painter()
                .rect_filled(Rect::from_center_size(c, vec2(11.0, 11.0)), 2.0, ink);
        }
        Icon::Metronome => {
            // a tall triangle with the pendulum arm across it
            ui.painter().add(egui::Shape::closed_line(
                vec![
                    pos2(c.x - 6.0, c.y + 7.0),
                    pos2(c.x + 6.0, c.y + 7.0),
                    pos2(c.x + 2.5, c.y - 7.0),
                    pos2(c.x - 2.5, c.y - 7.0),
                ],
                Stroke::new(1.5, ink),
            ));
            ui.painter().line_segment(
                [pos2(c.x, c.y + 4.0), pos2(c.x + 4.0, c.y - 6.0)],
                Stroke::new(1.5, ink),
            );
        }
    }
    resp.on_hover_text(tip)
}

/// A display like the web version's LCD: a dark well with lit digits and a caption.
fn lcd(ui: &mut Ui, text: &str, caption: &str, width: f32) -> egui::Response {
    let p = pal();
    let (r, resp) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click_and_drag());
    ui.painter().rect_filled(r, 7.0, p.well_deep);
    ui.painter()
        .rect_stroke(r, 7.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    ui.painter().text(
        pos2(r.center().x, r.top() + 15.0),
        Align2::CENTER_CENTER,
        text,
        FontId::monospace(19.0),
        p.lcd,
    );
    ui.painter().text(
        pos2(r.center().x, r.bottom() - 7.0),
        Align2::CENTER_CENTER,
        caption,
        FontId::proportional(8.5),
        p.muted,
    );
    resp
}

fn logo(ui: &mut Ui) {
    let p = pal();
    let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
    ui.painter().rect_filled(r, 8.0, p.accent);
    ui.painter().rect_filled(
        Rect::from_min_max(r.center(), r.max),
        8.0,
        p.accent2.gamma_multiply(0.85),
    );
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        "L",
        FontId::proportional(17.0),
        p.on_accent,
    );
}

impl App {
    /// Give egui the colours of the theme in effect (when it changes).
    pub(crate) fn paint_theme(&mut self, ctx: &egui::Context) {
        let dark = ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark);
        let id = theme::resolve(&self.shell.theme, dark);
        if self.shell.applied != id {
            theme::apply(ctx, id);
            self.shell.applied = id.to_string();
        }
    }

    pub(crate) fn choose_theme(&mut self, choice: &str) {
        self.shell.theme = choice.to_string();
        theme::save_choice(choice);
    }

    pub(crate) fn top_bar(&mut self, ui: &mut Ui) {
        let p = pal();
        let (playing, pos) = self
            .audio
            .as_ref()
            .map(|a| (a.meters.playing(), a.meters.position()))
            .unwrap_or((false, 0.0));
        let sig = self
            .project
            .loaded
            .as_ref()
            .map_or((4, 4), |l| l.project.time_sig);
        let bpm = self
            .project
            .loaded
            .as_ref()
            .map_or(120.0, |l| l.project.bpm);
        let busy = self.project.is_building();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
            logo(ui);
            ui.label(RichText::new("LAS").strong().size(17.0));
            ui.menu_button(RichText::new("\u{2630}").size(16.0), |ui| {
                ui.horizontal(|ui| self.main_menu(ui));
            });
            ui.add_space(4.0);
            // transport
            let song = self.shell.song_mode;
            if let Some(i) = segmented(ui, &["PAT", "SONG"], &[!song, song]) {
                self.shell.song_mode = i == 1;
            }
            if icon_button(ui, Icon::Play, playing, "Play (Space)").clicked() && !busy {
                self.toggle_play();
            }
            if icon_button(ui, Icon::Stop, false, "Stop").clicked() {
                self.stop_project();
                self.send(Command::PreviewStop);
            }
            let metro = self.shell.metronome;
            if icon_button(ui, Icon::Metronome, metro, "Metronome").clicked() {
                self.shell.metronome = !metro;
                self.send(Command::Metronome(!metro));
            }
            // tempo
            let r = lcd(ui, &format!("{bpm:.2}"), "BPM", 86.0);
            if r.drag_started() {
                self.project.modify(true, |_| {});
            }
            if r.dragged() {
                let d = -f64::from(r.drag_delta().y) * 0.25;
                if d != 0.0 {
                    self.project
                        .modify(false, |pr| pr.bpm = (pr.bpm + d).clamp(30.0, 300.0));
                }
            }
            r.on_hover_text("Drag up and down to change the tempo");
            if ui
                .add(
                    egui::Button::new(RichText::new("TAP").size(11.0).strong())
                        .min_size(vec2(40.0, 30.0)),
                )
                .clicked()
            {
                let now = Instant::now();
                self.shell
                    .taps
                    .retain(|t| now.duration_since(*t) < Duration::from_secs(2));
                self.shell.taps.push(now);
                if let Some(b) = tap_bpm(&self.shell.taps) {
                    self.project.modify(false, |pr| pr.bpm = b);
                }
            }
            let shown = if playing { pos } else { 0.0 };
            lcd(ui, &bar_beat_step(shown, sig), "BAR:BEAT:STEP", 104.0);
            // pattern
            if let Some(l) = &self.project.loaded {
                let project = l.project.clone();
                let name = project
                    .patterns
                    .get(self.project.pattern)
                    .map_or("-".to_string(), |x| x.name.clone());
                egui::ComboBox::from_id_salt("pattern")
                    .selected_text(name)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (i, x) in project.patterns.iter().enumerate() {
                            ui.selectable_value(&mut self.project.pattern, i, &x.name);
                        }
                    });
                if ui.button("+").on_hover_text("New pattern").clicked() {
                    let n = project.patterns.len() + 1;
                    self.project.modify(true, |pr| {
                        pr.patterns.push(Pattern {
                            id: las_project::create::uid("p"),
                            name: format!("Pattern {n}"),
                            color: "#7c5cff".into(),
                            ..Default::default()
                        });
                    });
                    self.project.pattern = n - 1;
                }
            }
            // windows
            let o = self.shell.open;
            let labels = ["Rack", "Playlist", "Roll", "Mixer", "Synth", "Browser"];
            let act = [
                o.rack,
                o.playlist,
                o.roll,
                o.mixer,
                o.synth,
                self.shell.browser,
            ];
            if let Some(i) = segmented(ui, &labels, &act) {
                match i {
                    0 => self.shell.open.rack = !o.rack,
                    1 => self.shell.open.playlist = !o.playlist,
                    2 => self.shell.open.roll = !o.roll,
                    3 => self.shell.open.mixer = !o.mixer,
                    4 => self.shell.open.synth = !o.synth,
                    _ => self.shell.browser = !self.shell.browser,
                }
            }
            if ui
                .add_enabled(
                    self.project.dirty,
                    egui::Button::new("Save").min_size(vec2(0.0, 30.0)),
                )
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save_project();
            }
            if ui
                .add_enabled(
                    self.project.loaded.is_some(),
                    egui::Button::new("Export").min_size(vec2(0.0, 30.0)),
                )
                .clicked()
            {
                self.shell.open.export = true;
            }
            // master
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(a) = &self.audio {
                    let (l, r) = a.meters.peaks();
                    meter(ui, r, vec2(8.0, 26.0));
                    meter(ui, l, vec2(8.0, 26.0));
                }
                let mut vol = self.volume;
                if knob(ui, &mut vol, 0.0..=1.0, 0.8, "Master", false, &|v| {
                    format!("{:.0}%", v * 100.0)
                }) {
                    self.volume = vol;
                    self.send(Command::Volume(vol as f32));
                }
            });
        });
        let _ = p;
    }

    /// Play or stop with the mode the PAT/SONG switch is on.
    pub(crate) fn toggle_play(&mut self) {
        let playing = self.audio.as_ref().is_some_and(|a| a.meters.playing());
        if playing {
            self.stop_project();
        } else if self.shell.song_mode {
            let from = self.project.start_tick;
            self.play_project(PlayMode::Song, from);
        } else {
            let i = self.project.pattern;
            self.play_project(PlayMode::Pattern(i), 0.0);
        }
    }

    pub(crate) fn status_bar(&mut self, ui: &mut Ui) {
        let p = pal();
        ui.horizontal_centered(|ui| {
            let msg = if let Some(e) = &self.project.error {
                RichText::new(e).color(p.hot)
            } else if self.project.is_building() {
                RichText::new("Loading the sounds...").color(p.muted)
            } else {
                RichText::new(&self.shell.status).color(p.muted)
            };
            ui.label(msg.size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let audio = match &self.audio {
                    Some(a) => {
                        let load = a.meters.load();
                        RichText::new(format!(
                            "{} at {} Hz   DSP {:.0}%   voices {}   dropouts {}",
                            a.device,
                            a.sample_rate,
                            load * 100.0,
                            a.meters.voices(),
                            a.meters.dropouts()
                        ))
                        .color(if load > 0.7 { p.hot } else { p.muted })
                    }
                    None => {
                        RichText::new(format!("No sound: {}", self.status_audio())).color(p.hot)
                    }
                };
                ui.label(audio.size(12.0));
                ui.separator();
                ui.label(
                    RichText::new(format!("Keys oct {}", self.octave))
                        .size(12.0)
                        .color(p.muted),
                );
                ui.separator();
                let name = self
                    .project
                    .loaded
                    .as_ref()
                    .map_or("No project".to_string(), |l| {
                        let dirty = if self.project.dirty { " \u{2022}" } else { "" };
                        format!("{}{dirty}", l.project.name)
                    });
                ui.label(RichText::new(name).size(12.0).color(p.text2));
            });
        });
    }

    fn status_audio(&self) -> String {
        self.status.clone()
    }

    pub(crate) fn browser(&mut self, ui: &mut Ui) {
        let p = pal();
        ui.horizontal(|ui| {
            for (t, name) in [
                (BrowserTab::Samples, "SAMPLES"),
                (BrowserTab::Plugins, "PLUGINS"),
                (BrowserTab::Presets, "PRESETS"),
                (BrowserTab::Project, "PROJECT"),
            ] {
                let on = self.shell.tab == t;
                let text = RichText::new(name).size(11.5).strong().color(if on {
                    p.accent_text
                } else {
                    p.muted
                });
                if ui.selectable_label(on, text).clicked() {
                    self.shell.tab = t;
                }
            }
        });
        ui.separator();
        match self.shell.tab {
            BrowserTab::Samples => self.samples_tab(ui),
            BrowserTab::Plugins => self.plugins_tab(ui),
            BrowserTab::Presets => {
                ui.add_space(4.0);
                ui.label(RichText::new("Presets of the synth").weak());
                ui.horizontal_wrapped(|ui| {
                    for (inst, name) in [
                        (Instrument::Polaris, "Polaris"),
                        (Instrument::Facet, "Facet"),
                        (Instrument::Mist, "Mist"),
                    ] {
                        if ui.selectable_label(self.inst == inst, name).clicked() {
                            self.switch(inst);
                        }
                    }
                });
                ui.add_space(4.0);
                self.preset_picker(ui);
            }
            BrowserTab::Project => self.project_tab(ui),
        }
    }

    fn samples_tab(&mut self, ui: &mut Ui) {
        let p = pal();
        ui.add(
            egui::TextEdit::singleline(&mut self.shell.search)
                .hint_text("Search...")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        let query = self.shell.search.clone();
        let hits = matching_samples(&query);
        let mut play: Option<&'static str> = None;
        let mut add: Option<(&'static str, &'static str)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(
                RichText::new("\u{2605} LUMEN FACTORY KIT")
                    .size(11.0)
                    .strong()
                    .color(p.muted),
            );
            for folder in sample_folders() {
                let items: Vec<_> = hits.iter().filter(|s| s.folder == folder).collect();
                if items.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(format!("{folder}   {}", items.len()))
                    .id_salt(folder)
                    .default_open(folder == "Kicks" || !query.is_empty())
                    .show(ui, |ui| {
                        for s in items {
                            ui.horizontal(|ui| {
                                let r = ui.selectable_label(false, s.name).on_hover_text(
                                    "Click to hear it, double click to add it as a channel",
                                );
                                if r.double_clicked() {
                                    add = Some((s.id, s.name));
                                } else if r.clicked() {
                                    play = Some(s.id);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(tag(s.id)).size(10.5).color(p.muted),
                                        );
                                    },
                                );
                            });
                        }
                    });
            }
            if hits.is_empty() {
                ui.label(RichText::new("Nothing matches.").weak());
            }
            ui.add_space(10.0);
            ui.label(
                RichText::new("MY SAMPLES")
                    .size(11.0)
                    .strong()
                    .color(p.muted),
            );
            if ui.button("Import files...").clicked() {
                self.import_dialog();
            }
            match &self.project.loaded {
                Some(l) if !l.project.samples.is_empty() => {
                    for s in &l.project.samples {
                        ui.label(&s.name);
                    }
                }
                Some(_) => {
                    ui.label(RichText::new("This project has no samples of its own.").weak());
                }
                None => {
                    ui.label(RichText::new("Open a project to see its samples.").weak());
                }
            }
        });
        if let Some(id) = play {
            self.audition(id);
        }
        if let Some((id, name)) = add {
            self.add_factory_channel(id, name);
        }
    }

    /// Play a factory sample through the output.
    fn audition(&mut self, id: &str) {
        let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) else {
            return;
        };
        if let Some(a) = las_engine::factory::render_factory(id, rate) {
            let left: std::sync::Arc<[f32]> = std::sync::Arc::from(a.channels[0].clone());
            let right: std::sync::Arc<[f32]> =
                std::sync::Arc::from(a.channels.get(1).unwrap_or(&a.channels[0]).clone());
            self.send(Command::Preview(Box::new(Preview {
                left,
                right,
                rate: a.sample_rate,
            })));
        }
    }

    fn plugins_tab(&mut self, ui: &mut Ui) {
        let p = pal();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(
                RichText::new("INSTRUMENTS")
                    .size(11.0)
                    .strong()
                    .color(p.muted),
            );
            for (name, inst, what) in [
                ("Polaris", Some(Instrument::Polaris), "Wavetable synth"),
                ("Facet", Some(Instrument::Facet), "Four-operator FM synth"),
                ("Mist", Some(Instrument::Mist), "Granular synth"),
                ("Vortex", None, "Wavetable synth with a 3D view"),
                ("Monolith", None, "Subtractive synth"),
                ("Zenith", None, "Supersaw and pads"),
                ("TriOsc", None, "Three oscillator synth"),
                ("Nebula", None, "Layered macro instrument"),
                ("Sampler", None, "Plays a sample across the keys"),
                ("LumaPad", None, "16 pads with choke groups"),
            ] {
                ui.horizontal(|ui| {
                    let enabled = inst.is_some();
                    let r = ui
                        .add_enabled(enabled, egui::Button::new(name).min_size(vec2(90.0, 0.0)))
                        .on_disabled_hover_text(
                            "The engine plays it; its window is not in the native app yet.",
                        );
                    if r.clicked() {
                        if let Some(i) = inst {
                            self.bound = None;
                            self.switch(i);
                            self.shell.open.synth = true;
                        }
                    }
                    ui.label(RichText::new(what).size(11.0).color(p.muted));
                });
            }
            ui.add_space(10.0);
            ui.label(RichText::new("EFFECTS").size(11.0).strong().color(p.muted));
            let mut last = String::new();
            for d in las_dsp::fx::registry::descriptors() {
                if d.category != last {
                    last = d.category.clone();
                    ui.label(RichText::new(&last).size(10.5).color(p.accent_text));
                }
                ui.label(&d.name).on_hover_text(&d.description);
            }
        });
    }

    fn project_tab(&mut self, ui: &mut Ui) {
        let p = pal();
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.project.path)
                .hint_text("path to a .las file")
                .desired_width(f32::INFINITY),
        );
        if ui.button("Open").clicked() {
            let path = self.project.path.trim().to_string();
            self.open_project(&path);
        }
        ui.label(
            RichText::new("Or drop a .las file on the window.")
                .weak()
                .size(11.0),
        );
        if let Some(l) = &self.project.loaded {
            ui.add_space(8.0);
            ui.label(
                RichText::new(&l.project.name)
                    .strong()
                    .size(15.0)
                    .color(p.accent_text),
            );
            ui.label(format!(
                "{} BPM, {}/{}",
                l.project.bpm, l.project.time_sig.0, l.project.time_sig.1
            ));
            ui.label(format!(
                "{} channels, {} patterns, {} samples",
                l.project.channels.len(),
                l.project.patterns.len(),
                l.samples.len()
            ));
            for w in &l.warnings {
                ui.label(RichText::new(w).color(p.warn).size(11.0));
            }
        }
    }

    /// The floating windows. `area` is the workspace they sit in.
    pub(crate) fn windows(&mut self, ctx: &egui::Context, area: Rect) {
        let a = area.shrink(6.0);
        let first = !self.shell.placed;
        self.shell.placed = true;
        let (song_head, pat_head) = match self.audio.as_ref() {
            Some(m) if m.meters.playing() => {
                let pos = m.meters.position();
                if self.project.looping {
                    (None, Some(pos))
                } else {
                    (Some(pos), None)
                }
            }
            _ => (None, None),
        };
        let mut open = self.shell.open;
        let win = |title: &str, pos: egui::Pos2, size: egui::Vec2| {
            let mut w = egui::Window::new(title)
                .constrain_to(area)
                .resizable(true)
                .default_size(size);
            if first {
                w = w.default_pos(pos);
            }
            w
        };
        let have = self.project.loaded.is_some();
        if open.playlist {
            win("Playlist - Arrangement", a.min, vec2(a.width(), a.height()))
                .open(&mut open.playlist)
                .show(ctx, |ui| {
                    if have {
                        self.playlist(ui, song_head);
                    } else {
                        self.no_project(ui);
                    }
                });
        }
        if open.rack {
            win(
                "Channel Rack",
                a.min + vec2(170.0, 80.0),
                vec2(740.0, 300.0),
            )
            .open(&mut open.rack)
            .show(ctx, |ui| {
                if have {
                    self.rack_window(ui, pat_head);
                } else {
                    self.no_project(ui);
                }
            });
        }
        if open.roll {
            win(
                "Piano Roll",
                a.min + vec2(60.0, 40.0),
                vec2(a.width() - 120.0, a.height() - 80.0),
            )
            .open(&mut open.roll)
            .show(ctx, |ui| {
                if have {
                    self.roll(ui, pat_head);
                } else {
                    self.no_project(ui);
                }
            });
        }
        if open.mixer {
            win(
                "Mixer",
                a.min + vec2(30.0, a.height() - 330.0),
                vec2(a.width() - 60.0, 310.0),
            )
            .open(&mut open.mixer)
            .show(ctx, |ui| {
                if have {
                    self.mixer_window(ui);
                } else {
                    self.no_project(ui);
                }
            });
        }
        if open.synth {
            win("Synth", a.min + vec2(200.0, 30.0), vec2(860.0, 640.0))
                .open(&mut open.synth)
                .show(ctx, |ui| match self.bound {
                    Some(crate::inst::Bound::Basic(c)) => self.basic_editor(ui, c),
                    _ => self.synth_window(ui),
                });
        }
        if let Some((insert, slot)) = self.project.open_effect {
            let mut keep = true;
            win("Effect", a.min + vec2(260.0, 120.0), vec2(620.0, 420.0))
                .open(&mut keep)
                .show(ctx, |ui| self.effect_window(ui, insert, slot));
            if !keep {
                self.project.open_effect = None;
            }
        }
        if open.export {
            egui::Window::new("Export audio")
                .open(&mut open.export)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| self.export_window(ui));
        }
        self.shell.open = open;
        self.about_window(ctx);
        self.discard_dialog(ctx);
    }

    fn no_project(&self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.label(RichText::new("No project open.").size(15.0));
        ui.label(
            RichText::new("Open one in the browser (PROJECT), or drop a .las file here.").weak(),
        );
    }

    fn rack_window(&mut self, ui: &mut Ui, playhead: Option<f64>) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let mut selected = self.project.channel;
        let out = steps::draw(
            ui,
            &project,
            self.project.pattern,
            &mut selected,
            &mut self.shell.steps,
            playhead,
        );
        ui.separator();
        ui.horizontal(|ui| {
            ui.menu_button("+ Add channel", |ui| {
                for (id, name, _) in las_project::create::INSTRUMENTS {
                    if ui.button(*name).clicked() {
                        self.add_channel(id);
                        ui.close();
                    }
                }
            });
            ui.label(
                RichText::new(
                    project
                        .patterns
                        .get(self.project.pattern)
                        .map_or("", |x| x.name.as_str()),
                )
                .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut sw = project.swing;
                let (ch, start) =
                    steps::mini_knob(ui, &mut sw, (0.0, 1.0), 0.0, "Swing", pal().accent);
                ui.label(RichText::new("SWING").size(10.0).color(pal().muted));
                if start {
                    self.project.modify(true, |_| {});
                }
                if ch {
                    self.project.modify(false, |pr| pr.swing = sw);
                }
            });
        });
        drop(project);
        self.project.channel = selected;
        if let Some(c) = out.open_roll {
            self.project.channel = c;
            self.shell.open.roll = true;
        }
        if let Some(c) = out.open_synth {
            self.open_channel(c);
        }
        for (ch, e) in &out.notes {
            self.project.edit_channel(*ch, std::slice::from_ref(e));
        }
        if !out.chans.is_empty() {
            self.project.edit_chans(&out.chans);
        }
    }

    fn mixer_window(&mut self, ui: &mut Ui) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let edits = mixer::draw(ui, &project, &mut self.project.mixer_state);
        drop(project);
        self.project.edit_mixer(&edits);
    }

    fn synth_window(&mut self, ui: &mut Ui) {
        if let (Some(b), Some(l)) = (self.bound, &self.project.loaded) {
            if let Some(c) = l.project.channels.get(b.channel()) {
                ui.label(
                    RichText::new(format!("{}  ({})", c.name, c.plugin_id))
                        .strong()
                        .size(15.0)
                        .color(pal().accent_text),
                );
            }
        } else {
            ui.label(RichText::new("Stand-alone synth (not part of the project)").weak());
        }
        ui.horizontal(|ui| {
            for (inst, name) in [
                (Instrument::Polaris, "Polaris"),
                (Instrument::Facet, "Facet"),
                (Instrument::Mist, "Mist"),
            ] {
                if ui.selectable_label(self.inst == inst, name).clicked() {
                    self.switch(inst);
                }
            }
            ui.separator();
            self.preset_picker(ui);
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view, crate::app::View::Sound, "Sound");
            let n = self.fx[crate::app::index(self.inst)].len();
            let label = if n == 0 {
                "Effects".to_string()
            } else {
                format!("Effects ({n})")
            };
            ui.selectable_value(&mut self.view, crate::app::View::Effects, label);
        });
        ui.separator();
        egui::ScrollArea::both().show(ui, |ui| {
            if self.view == crate::app::View::Effects {
                self.effects_view(ui);
                return;
            }
            match self.inst {
                Instrument::Facet => self.synth(ui),
                Instrument::Mist => self.mist_panel(ui),
                Instrument::Polaris => self.polaris_panel(ui),
            }
        });
        ui.separator();
        self.keyboard(ui);
    }

    fn export_window(&mut self, ui: &mut Ui) {
        if self.shell.export.path.is_empty() {
            if let Some(l) = &self.project.loaded {
                self.shell.export.path = format!("{}.wav", l.path.trim_end_matches(".las"));
            }
        }
        // a render that has finished
        if let Some(rx) = &self.shell.export.working {
            if let Ok(r) = rx.try_recv() {
                self.shell.export.message = Some(r);
                self.shell.export.working = None;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        let busy = self.shell.export.working.is_some();
        ui.horizontal(|ui| {
            ui.label("File");
            ui.add(egui::TextEdit::singleline(&mut self.shell.export.path).desired_width(360.0));
        });
        ui.horizontal(|ui| {
            ui.label("Format");
            let f = self.shell.export.format;
            egui::ComboBox::from_id_salt("export-format")
                .selected_text(FORMATS[f].0)
                .show_ui(ui, |ui| {
                    for (i, (name, _, _)) in FORMATS.iter().enumerate() {
                        ui.selectable_value(&mut self.shell.export.format, i, *name);
                    }
                });
            ui.checkbox(&mut self.shell.export.normalize, "Normalize to -0.3 dB");
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.shell.export.pattern_only, false, "Song");
            ui.selectable_value(&mut self.shell.export.pattern_only, true, "Chosen pattern");
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Render")).clicked() {
                self.start_export();
            }
            if busy {
                ui.spinner();
                ui.label("Rendering...");
            }
        });
        match &self.shell.export.message {
            Some(Ok(m)) => {
                ui.label(RichText::new(m).color(pal().ok));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(pal().hot));
            }
            None => {}
        }
    }

    fn start_export(&mut self) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let (project, samples) = (l.project.clone(), l.samples.clone());
        let e = &self.shell.export;
        let (path, normalize) = (e.path.trim().to_string(), e.normalize);
        let (_, flac, bits) = FORMATS[e.format];
        let mode = if e.pattern_only {
            Mode::Pattern {
                id: project
                    .patterns
                    .get(self.project.pattern)
                    .map(|p| p.id.clone()),
                loops: 1,
            }
        } else {
            Mode::Song
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let run = || -> Result<String, String> {
                let opts = RenderOptions {
                    sample_rate: 44100.0,
                    mode,
                    ..Default::default()
                };
                let r = las_engine::render_mix_with(&project, &samples, &opts)?;
                let chans = [&r.left[..], &r.right[..]];
                let scaled;
                let chans: [&[f32]; 2] = if normalize {
                    scaled = las_engine::export::normalize(&chans, -0.3);
                    [&scaled[0], &scaled[1]]
                } else {
                    chans
                };
                let rate = r.sample_rate as u32;
                let bytes = if flac {
                    las_engine::export::encode_flac(&chans, rate, bits)?
                } else {
                    let depth = las_engine::export::BitDepth::from_bits(bits).ok_or("bad depth")?;
                    las_engine::export::encode_wav(&chans, rate, depth, bits == 16)
                };
                std::fs::write(&path, bytes).map_err(|e| format!("{path}: {e}"))?;
                Ok(format!("{path}: {:.1} s", r.seconds()))
            };
            let _ = tx.send(run());
        });
        self.shell.export.working = Some(rx);
        self.shell.export.message = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_make_a_tempo() {
        let t0 = Instant::now();
        let taps: Vec<Instant> = (0..5)
            .map(|i| t0 + Duration::from_millis(500 * i))
            .collect();
        assert_eq!(tap_bpm(&taps), Some(120.0));
        assert_eq!(tap_bpm(&taps[..1]), None);
        let slow: Vec<Instant> = (0..3).map(|i| t0 + Duration::from_secs(5 * i)).collect();
        assert_eq!(tap_bpm(&slow), None, "too slow to be a tempo");
    }

    #[test]
    fn the_position_reads_as_bar_beat_step() {
        assert_eq!(bar_beat_step(0.0, (4, 4)), "1:01:01");
        assert_eq!(bar_beat_step(96.0 + 24.0, (4, 4)), "1:02:02");
        assert_eq!(bar_beat_step(384.0 * 2.0, (4, 4)), "3:01:01");
        assert_eq!(bar_beat_step(288.0, (3, 4)), "2:01:01");
    }

    #[test]
    fn the_sample_search_matches_names_and_folders() {
        assert!(!matching_samples("").is_empty());
        let kicks = matching_samples("kick");
        assert!(
            kicks.len() >= 4
                && kicks.iter().all(|s| {
                    s.name.to_lowercase().contains("kick")
                        || s.folder.to_lowercase().contains("kick")
                })
        );
        assert!(matching_samples("zzzz-no-such").is_empty());
        assert_eq!(sample_folders()[0], "Kicks");
        assert_eq!(tag("factory:kick-punch"), "kick");
    }

    #[test]
    fn exports_offer_wav_and_flac() {
        assert!(FORMATS.iter().any(|f| f.1) && FORMATS.iter().any(|f| !f.1));
        assert!(
            FORMATS.iter().all(|f| !(f.1 && f.2 == 32)),
            "no 32 bit flac"
        );
    }
}
