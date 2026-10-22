// SPDX-License-Identifier: GPL-3.0-or-later
//! Workflow tools brought in from other DAWs, kept FL-like and simple: the command
//! palette (Ctrl+K), Capture MIDI (Ctrl+Shift+C), render to audio clip and consolidate,
//! one-click sidechain, a reference track with A/B, a tuner, project templates and
//! restoring the autosave after a crash.

pub mod bounce;
pub mod capture;
pub mod palette;
pub mod reference;
pub mod templates;
pub mod tuner;

use crate::app::App;
use crate::audio::{Command, Preview, RefTrack};
use crate::mixer::MixEdit;
use bounce::{Scope, Source};
use eframe::egui::{self, Key, RichText, Ui};
use palette::{Cmd, Item};
use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

/// What to do with a finished render.
enum Apply {
    Place {
        name: String,
        at: f64,
        mute: Option<Source>,
    },
    Replace {
        ids: HashSet<String>,
        at: f64,
        track: usize,
    },
}

/// A render running in the background.
struct Running {
    done: Receiver<Result<Vec<u8>, String>>,
    then: Apply,
}

pub struct Workflow {
    pub palette: palette::Palette,
    /// Every note played lately, for Capture MIDI.
    pub capture: capture::SharedLog,
    pub tuner: tuner::Tuner,
    pub reference: Option<reference::Reference>,
    /// The reference is heard (B) instead of the mix (A).
    pub ab: bool,
    pub reference_open: bool,
    mix_loudness: Option<Receiver<Result<f64, String>>>,
    /// Mute what was rendered once its audio clip is in.
    pub mute_source: bool,
    running: Vec<Running>,
    pub safety: Option<templates::Safety>,
    last_autosave: Instant,
    /// Projects opened or saved lately, newest first.
    pub recent: Vec<String>,
    seen_path: String,
    /// The name being typed for a new template.
    pub template_name: Option<String>,
    /// A message for the status bar.
    pub note: Option<String>,
}

impl Default for Workflow {
    fn default() -> Self {
        Self {
            palette: Default::default(),
            capture: capture::CaptureLog::shared(),
            tuner: Default::default(),
            reference: None,
            ab: false,
            reference_open: false,
            mix_loudness: None,
            mute_source: true,
            running: Vec::new(),
            safety: None,
            last_autosave: Instant::now(),
            recent: Vec::new(),
            seen_path: String::new(),
            template_name: None,
            note: None,
        }
    }
}

const RECENT_KEEP: usize = 10;

fn recent_file() -> Option<std::path::PathBuf> {
    Some(crate::presets::config_dir()?.join("recent.txt"))
}

impl Workflow {
    /// The workflow tools of the running app: crash safety and the recent list on.
    pub fn start() -> Self {
        let mut w = Self::default();
        w.safety = templates::dir("autosave").map(templates::Safety::start);
        w.recent = recent_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| {
                s.lines()
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        w
    }

    /// Remember a project file as recently used.
    fn remember(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(RECENT_KEEP);
        if let Some(f) = recent_file() {
            let _ = std::fs::create_dir_all(f.parent().unwrap_or(&f));
            let _ = std::fs::write(f, self.recent.join("\n"));
        }
    }
}

impl Drop for Workflow {
    fn drop(&mut self) {
        // a clean exit; a panic leaves the lock so the autosave is offered next time
        if !std::thread::panicking() {
            if let Some(s) = &self.safety {
                s.finish();
            }
        }
    }
}

impl App {
    /// Key played: remember it for Capture MIDI.
    pub(crate) fn remember_key(&self, key: u8, vel: f64, on: bool) {
        if let Ok(mut log) = self.wf.capture.lock() {
            log.push(key, vel, on);
        }
    }

    /// Every frame: shortcuts, finished renders, the tuner's signal, the autosave.
    pub(crate) fn workflow_logic(&mut self, ctx: &egui::Context) {
        let (palette, capture) = ctx.input(|i| {
            let c = i.modifiers.command;
            (
                c && !i.modifiers.shift && i.key_pressed(Key::K),
                c && i.modifiers.shift && i.key_pressed(Key::C),
            )
        });
        if palette {
            if self.wf.palette.open {
                self.wf.palette.open = false;
            } else {
                self.wf.palette.show("");
            }
        }
        if capture {
            self.capture_midi();
        }
        self.poll_renders();
        if let Some(rx) = &self.wf.mix_loudness {
            if let Ok(r) = rx.try_recv() {
                self.wf.mix_loudness = None;
                if let (Ok(l), Some(rf)) = (r, &mut self.wf.reference) {
                    rf.mix_lufs = Some(l);
                    let g = rf.gain();
                    self.send(Command::ReferenceGain(g));
                }
            }
        }
        if let (Some(_), Some(a)) = (self.wf.tuner.insert, self.audio.as_mut()) {
            let sr = f64::from(a.sample_rate);
            let n = a.tuner.slots();
            let got: Vec<f32> = (0..n).filter_map(|_| a.tuner.pop().ok()).collect();
            self.wf.tuner.feed(got.into_iter(), sr);
        }
        // recent projects
        if self.project.has_file() && self.project.path != self.wf.seen_path {
            self.wf.seen_path = self.project.path.clone();
            let p = self.wf.seen_path.clone();
            self.wf.remember(&p);
        }
        // the autosave
        if self.project.dirty
            && self.wf.last_autosave.elapsed() >= templates::AUTOSAVE_EVERY
            && !self.wf.safety.as_ref().is_some_and(|s| s.crashed)
        {
            self.wf.last_autosave = Instant::now();
            if let (Some(l), Some(s)) = (&self.project.loaded, &self.wf.safety) {
                if let Ok(bytes) = las_project::encode(&l.project, &l.samples, true) {
                    if s.write(&bytes).is_ok() {
                        self.wf.note = Some("Autosaved".into());
                    }
                }
            }
        }
    }

    /// The workflow windows: palette, tuner, reference, restore and template dialogs.
    pub(crate) fn workflow_windows(&mut self, ctx: &egui::Context) {
        self.palette_window(ctx);
        self.tuner_window(ctx);
        self.reference_window(ctx);
        self.restore_dialog(ctx);
        self.template_dialog(ctx);
    }

    /// Everything the palette can find.
    fn palette_items(&self) -> Vec<Item> {
        let mut items = palette::actions();
        for (i, (name, what)) in templates::BUILT_IN.iter().enumerate() {
            items.push(
                Item::new(
                    format!("New from template: {name}"),
                    "Template",
                    Cmd::Template(i),
                )
                .hint(*what),
            );
        }
        if let Some(d) = templates::dir("templates") {
            for (name, path) in templates::user_templates(&d) {
                items.push(
                    Item::new(
                        format!("New from template: {name}"),
                        "Template",
                        Cmd::UserTemplate(path.to_string_lossy().into_owned()),
                    )
                    .hint("saved"),
                );
            }
        }
        for (id, name, _) in las_project::create::INSTRUMENTS {
            items.push(
                Item::new(
                    format!("Add {name}"),
                    "Instrument",
                    Cmd::AddInstrument(id.to_string()),
                )
                .hint("new channel"),
            );
        }
        for d in las_dsp::fx::registry::descriptors() {
            items.push(
                Item::new(d.name.clone(), "Effect", Cmd::AddEffect(d.id.clone()))
                    .hint("on the selected insert"),
            );
        }
        // Polaris's list is that of the face last opened (Monolith, Zenith, TriOsc...)
        let face = las_dsp::presets::bank(&self.face_bank)
            .map_or_else(|| "Polaris".to_string(), |b| b.name.clone());
        for (k, list) in self.entries.iter().enumerate() {
            let inst = ["Facet", "Mist", face.as_str()][k];
            for (i, e) in list.iter().enumerate().skip(1) {
                items.push(Item::new(e.name.clone(), "Preset", Cmd::Preset(k, i)).hint(inst));
            }
        }
        for s in las_engine::factory::FACTORY_SAMPLES {
            items.push(
                Item::new(s.name, "Sample", Cmd::Sample(s.id.into(), s.name.into())).hint(s.folder),
            );
        }
        for p in &self.wf.recent {
            let name = std::path::Path::new(p)
                .file_name()
                .map_or(p.clone(), |n| n.to_string_lossy().into_owned());
            items.push(Item::new(name, "Recent", Cmd::Recent(p.clone())).hint(p.clone()));
        }
        for t in crate::theme::themes() {
            items.push(Item::new(
                format!("Theme: {}", t.name),
                "Option",
                Cmd::Theme(t.id.to_string()),
            ));
        }
        items
    }

    fn palette_window(&mut self, ctx: &egui::Context) {
        if !self.wf.palette.open {
            return;
        }
        let items = self.palette_items();
        let hits = palette::search(&items, &self.wf.palette.query, 12);
        let (up, down, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        let pal = &mut self.wf.palette;
        if down {
            pal.selected = (pal.selected + 1).min(hits.len().saturating_sub(1));
        }
        if up {
            pal.selected = pal.selected.saturating_sub(1);
        }
        pal.selected = pal.selected.min(hits.len().saturating_sub(1));
        let mut run = None;
        let p = crate::theme::pal();
        egui::Window::new("Quick search")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size([520.0, 0.0])
            .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
            .show(ctx, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut pal.query)
                        .hint_text(
                            "Search actions, windows, instruments, effects, presets, samples...",
                        )
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Heading),
                );
                if pal.focus || !r.has_focus() {
                    r.request_focus();
                    pal.focus = false;
                }
                if r.changed() {
                    pal.selected = 0;
                }
                ui.add_space(4.0);
                if hits.is_empty() {
                    ui.label(RichText::new("Nothing matches").color(p.muted));
                }
                for (row, &i) in hits.iter().enumerate() {
                    let it = &items[i];
                    let sel = row == pal.selected;
                    let resp = ui
                        .horizontal(|ui| {
                            ui.set_width(ui.available_width());
                            let b = ui.add(
                                egui::Button::selectable(sel, RichText::new(&it.label).size(14.0))
                                    .min_size(egui::vec2(330.0, 22.0)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(it.kind).size(11.0).color(p.accent_text),
                                    );
                                    if !it.hint.is_empty() {
                                        ui.label(RichText::new(&it.hint).size(11.0).color(p.muted));
                                    }
                                },
                            );
                            b
                        })
                        .inner;
                    if resp.clicked() {
                        run = Some(it.cmd.clone());
                    }
                    if resp.hovered() {
                        pal.selected = row;
                    }
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new("Up and Down choose, Enter runs, Esc closes")
                        .size(10.5)
                        .color(p.muted),
                );
            });
        if enter {
            run = hits.get(pal.selected).map(|&i| items[i].cmd.clone());
        }
        if esc {
            pal.open = false;
        }
        if let Some(c) = run {
            self.wf.palette.open = false;
            self.run_cmd(c);
        }
    }

    /// Carry out a palette command.
    pub(crate) fn run_cmd(&mut self, c: Cmd) {
        use crate::menus::Action;
        let loaded = self.project.loaded.is_some();
        match c {
            Cmd::New => self.guard(Action::New),
            Cmd::Open => self.guard(Action::Open),
            Cmd::Demo => self.guard(Action::Demo),
            Cmd::Save => self.save_project(),
            Cmd::SaveAs => self.save_as_dialog(),
            Cmd::Import if loaded => self.import_dialog(),
            Cmd::Export => self.shell.open.show(crate::raise::Win::Export),
            Cmd::Undo => self.project.undo(),
            Cmd::Redo => self.project.redo(),
            Cmd::History => self.shell.open.show(crate::raise::Win::History),
            Cmd::NewPattern if loaded => self.new_pattern(),
            Cmd::Shortcuts => self.shell.open.show(crate::raise::Win::Keys),
            Cmd::About => self.shell.open.show(crate::raise::Win::About),
            Cmd::Capture => self.capture_midi(),
            Cmd::Consolidate => self.consolidate(),
            Cmd::RenderChannel => {
                let c = self.project.channel;
                self.bounce(Source::Channel(c), Scope::Song);
            }
            Cmd::SaveTemplate => self.wf.template_name = Some(String::new()),
            Cmd::Template(i) => self.new_from(templates::built_in(i)),
            Cmd::UserTemplate(path) => match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| las_project::decode(&b).map_err(|e| e.to_string()))
            {
                Ok(b) => {
                    self.stop_project();
                    self.project
                        .install(templates::fresh(b.project), b.samples, "", b.warnings);
                    self.project.path.clear();
                    self.bound = None;
                    self.ready_project();
                }
                Err(e) => self.project.error = Some(format!("{path}: {e}")),
            },
            Cmd::Window(w) => {
                let o = &mut self.shell.open;
                match w {
                    "playlist" => o.show(crate::raise::Win::Playlist),
                    "rack" => o.show(crate::raise::Win::Rack),
                    "roll" => o.show(crate::raise::Win::Roll),
                    "mixer" => o.show(crate::raise::Win::Mixer),
                    "synth" => o.show(crate::raise::Win::Synth),
                    "browser" => self.shell.browser = true,
                    "tuner" => {
                        let i = self.project.mixer_state.selected;
                        self.open_tuner(i);
                    }
                    _ => self.wf.reference_open = true,
                }
            }
            Cmd::AddInstrument(id) if loaded => {
                self.add_channel(&id);
                let at = self.project.channel;
                self.open_channel(at);
            }
            Cmd::AddEffect(id) if loaded => {
                let insert = self.project.mixer_state.selected;
                let slot = self
                    .project
                    .loaded
                    .as_ref()
                    .and_then(|l| l.project.mixer.get(insert))
                    .map_or(0, |t| {
                        t.slots
                            .iter()
                            .position(Option::is_none)
                            .unwrap_or(t.slots.len())
                    });
                self.project.edit_mixer(&[
                    MixEdit::Begin,
                    MixEdit::AddEffect {
                        insert,
                        slot,
                        plugin: id,
                    },
                    MixEdit::OpenEffect { insert, slot },
                ]);
                self.shell.open.show(crate::raise::Win::Mixer);
            }
            Cmd::Preset(k, i) => {
                let inst = [
                    crate::audio::Instrument::Facet,
                    crate::audio::Instrument::Mist,
                    crate::audio::Instrument::Polaris,
                ][k.min(2)];
                // the synth the preset is for (Polaris's list is the face last opened)
                let plugin = ["facet", "mist", self.face_bank.as_str()][k.min(2)].to_string();
                let name = self.entries[k.min(2)].get(i).map(|e| e.name.clone());
                if loaded {
                    // into the open channel when it is that synth, else into a new
                    // channel of it: never into a channel of another synth
                    let open = self.bound.and_then(|b| match b {
                        crate::inst::Bound::Synth(c) => Some(c),
                        _ => None,
                    });
                    let fits = open.filter(|&c| {
                        self.project
                            .loaded
                            .as_ref()
                            .and_then(|l| l.project.channels.get(c))
                            .is_some_and(|ch| ch.plugin_id == plugin)
                    });
                    let ch = match fits {
                        Some(c) => c,
                        None => {
                            self.add_channel(&plugin);
                            self.project.channel
                        }
                    };
                    self.open_channel(ch);
                    // opening may have rebuilt the list: find the preset by its name
                    let at = name
                        .and_then(|n| self.entries[k.min(2)].iter().position(|e| e.name == n))
                        .unwrap_or(i);
                    self.load_preset(inst, at);
                } else {
                    self.load_preset(inst, i);
                    self.switch(inst);
                    self.shell.open.show(crate::raise::Win::Synth);
                }
            }
            Cmd::Sample(id, name) if loaded => self.add_factory_channel(&id, &name),
            Cmd::Recent(p) => {
                if self.project.dirty {
                    self.wf.note = Some("Save or discard the changes first (Ctrl+S)".into());
                } else {
                    self.open_project(&p);
                }
            }
            Cmd::Theme(t) => self.choose_theme(&t),
            _ => self.wf.note = Some("Open a project first".into()),
        }
    }

    /// Start a new project from a template (asking about unsaved changes is the
    /// menu's job; templates come from the menu and the palette only when clean or
    /// confirmed).
    fn new_from(&mut self, p: las_project::Project) {
        if self.project.dirty {
            self.wf.note = Some("Save or discard the changes first (Ctrl+S)".into());
            return;
        }
        self.stop_project();
        self.project
            .install(templates::fresh(p), Default::default(), "", Vec::new());
        self.project.path.clear();
        self.bound = None;
        self.ready_project();
    }

    /// File menu entries: templates.
    pub(crate) fn template_menu(&mut self, ui: &mut Ui) {
        ui.menu_button("New from template", |ui| {
            for (i, (name, what)) in templates::BUILT_IN.iter().enumerate() {
                if ui.button(*name).on_hover_text(*what).clicked() {
                    self.run_cmd(Cmd::Template(i));
                    ui.close();
                }
            }
            if let Some(d) = templates::dir("templates") {
                let saved = templates::user_templates(&d);
                if !saved.is_empty() {
                    ui.separator();
                }
                for (name, path) in saved {
                    if ui.button(name).clicked() {
                        self.run_cmd(Cmd::UserTemplate(path.to_string_lossy().into_owned()));
                        ui.close();
                    }
                }
            }
        });
        if ui
            .add_enabled(
                self.project.loaded.is_some(),
                egui::Button::new("Save as template..."),
            )
            .on_hover_text("Keep this project as a starting point for new ones")
            .clicked()
        {
            self.wf.template_name = Some(String::new());
            ui.close();
        }
    }

    /// Tools menu entries (in View): palette, capture, consolidate, tuner, reference.
    pub(crate) fn tools_menu(&mut self, ui: &mut Ui) {
        if ui.button("Quick search...      Ctrl+K").clicked() {
            self.wf.palette.show("");
            ui.close();
        }
        if ui
            .button("Capture MIDI      Ctrl+Shift+C")
            .on_hover_text(
                "Write the notes you just played into the pattern, even when not recording",
            )
            .clicked()
        {
            self.capture_midi();
            ui.close();
        }
        if ui
            .button("Consolidate selected clips")
            .on_hover_text("Render the selected playlist clips into one audio clip")
            .clicked()
        {
            self.consolidate();
            ui.close();
        }
        ui.checkbox(
            &mut self.wf.mute_source,
            "Mute the source after Render to audio clip",
        )
        .on_hover_text(
            "Render to audio clip is in the right-click menu of a channel or a mixer insert",
        );
        if ui.button("Tuner").clicked() {
            let i = self.project.mixer_state.selected;
            self.open_tuner(i);
            ui.close();
        }
        if ui.button("Reference track (A/B)").clicked() {
            self.wf.reference_open = true;
            ui.close();
        }
    }

    /// Capture MIDI: the notes just played go into the current pattern, on the
    /// selected channel.
    pub(crate) fn capture_midi(&mut self) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let p = &l.project;
        let Some(ch) = p.channels.get(self.project.channel).map(|c| c.id.clone()) else {
            self.wf.note = Some("Capture: choose a channel first".into());
            return;
        };
        let pat = self.project.pattern.min(p.patterns.len().saturating_sub(1));
        let pattern_len = match p.patterns.get(pat) {
            Some(x) if x.length > 0.0 => x.length,
            Some(x) => {
                let bar = las_engine::timeline::ticks_per_bar(p.time_sig);
                let end = x
                    .notes
                    .values()
                    .flatten()
                    .map(|n| n.pos + n.len)
                    .fold(0.0, f64::max);
                (end / bar).ceil().max(1.0) * bar
            }
            None => 0.0,
        };
        let playhead = self
            .audio
            .as_ref()
            .filter(|a| a.meters.playing())
            .map(|a| a.meters.position());
        let (events, now) = match self.wf.capture.lock() {
            Ok(log) => (log.events(), log.now()),
            Err(_) => return,
        };
        let notes = capture::capture(
            &events,
            &capture::Timing {
                now,
                bpm: p.bpm,
                playhead,
                pattern_len: if self.project.looping {
                    pattern_len
                } else {
                    0.0
                },
            },
        );
        if notes.is_empty() {
            self.wf.note = Some("Capture: play something on the keyboard first".into());
            return;
        }
        let n = notes.len();
        self.project.modify(true, move |p| {
            if let Some(x) = p.patterns.get_mut(pat) {
                x.notes.entry(ch).or_default().extend(notes);
            }
        });
        if let Ok(mut log) = self.wf.capture.lock() {
            log.clear();
        }
        self.wf.note = Some(format!(
            "Captured {n} note{} into the pattern{}",
            if n == 1 { "" } else { "s" },
            if playhead.is_some() {
                ", in time with the song"
            } else {
                ""
            }
        ));
    }

    /// Render a channel or insert to an audio clip, in the background.
    pub(crate) fn bounce(&mut self, what: Source, scope: Scope) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(job) = bounce::job(&l.project, what, scope) else {
            return;
        };
        let at = match scope {
            Scope::Song => 0.0,
            Scope::Pattern(i) => bounce::pattern_start(&l.project, i),
        };
        let samples = l.samples.clone();
        let then = Apply::Place {
            name: job.name.clone(),
            at,
            mute: self.wf.mute_source.then_some(what),
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(bounce::run(&job, &samples));
        });
        self.wf.running.push(Running { done: rx, then });
        self.wf.note = Some("Rendering to an audio clip...".into());
    }

    /// Consolidate the playlist's selected clips into one audio clip.
    pub(crate) fn consolidate(&mut self) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let ids = self.project.playlist.selected.clone();
        let Some((job, at, track)) = bounce::consolidate_job(&l.project, &ids) else {
            self.wf.note = Some("Consolidate: select clips in the playlist first".into());
            return;
        };
        let samples = l.samples.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(bounce::run(&job, &samples));
        });
        self.wf.running.push(Running {
            done: rx,
            then: Apply::Replace { ids, at, track },
        });
        self.wf.note = Some("Consolidating...".into());
    }

    fn poll_renders(&mut self) {
        let mut i = 0;
        while i < self.wf.running.len() {
            let Ok(r) = self.wf.running[i].done.try_recv() else {
                i += 1;
                continue;
            };
            let job = self.wf.running.remove(i);
            let wav = match r {
                Ok(w) => w,
                Err(e) => {
                    self.wf.note = Some(format!("Render: {e}"));
                    continue;
                }
            };
            let mut result = Ok(0);
            match job.then {
                Apply::Place { name, at, mute } => {
                    self.project.modify_with_samples(true, |p, s| {
                        result = bounce::place(p, s, wav, &name, at, mute);
                    });
                    if result.is_ok() {
                        self.wf.note = Some(format!("{name}: on the playlist"));
                    }
                }
                Apply::Replace { ids, at, track } => {
                    self.project.modify_with_samples(true, |p, s| {
                        result = bounce::replace_clips(p, s, &ids, wav, at, track);
                    });
                    self.project.playlist.selected.clear();
                    if result.is_ok() {
                        self.wf.note = Some("Consolidated into one audio clip".into());
                    }
                }
            }
            if let Err(e) = result {
                self.wf.note = Some(format!("Render: {e}"));
            }
        }
    }

    /// The mixer's own edits for the workflow tools; the rest goes on to the project.
    pub(crate) fn workflow_mix_edits(&mut self, edits: Vec<MixEdit>) -> Vec<MixEdit> {
        let mut rest = Vec::with_capacity(edits.len());
        for e in edits {
            match e {
                MixEdit::Render { insert, pattern } => {
                    let scope = if pattern {
                        Scope::Pattern(self.project.pattern)
                    } else {
                        Scope::Song
                    };
                    self.bounce(Source::Insert(insert), scope);
                }
                MixEdit::Tuner { insert } => self.open_tuner(insert),
                MixEdit::Sidechain { insert, source } => {
                    self.wf.note = Some(format!(
                        "Insert {insert} now ducks when insert {source} plays (its compressor's threshold sets how much)"
                    ));
                    rest.push(MixEdit::Sidechain { insert, source });
                }
                other => rest.push(other),
            }
        }
        rest
    }

    /// A render asked for from the channel rack.
    pub(crate) fn rack_render(&mut self, channel: usize, pattern: bool) {
        let scope = if pattern {
            Scope::Pattern(self.project.pattern)
        } else {
            Scope::Song
        };
        self.bounce(Source::Channel(channel), scope);
    }

    /// The Capture button of the channel rack, with how many notes are waiting.
    pub(crate) fn capture_button(&mut self, ui: &mut Ui) {
        let n = self.wf.capture.lock().map_or(0, |l| l.phrase_notes());
        let label = if n > 0 {
            format!("Capture ({n})")
        } else {
            "Capture".to_string()
        };
        if ui
            .add_enabled(n > 0, egui::Button::new(label))
            .on_hover_text("Capture MIDI (Ctrl+Shift+C): put the notes you just played on the keyboard into this pattern, on the selected channel. Works without recording.")
            .on_disabled_hover_text("Play something on the keyboard; Capture then turns it into notes")
            .clicked()
        {
            self.capture_midi();
        }
    }

    fn open_tuner(&mut self, insert: usize) {
        self.wf.tuner.insert = Some(insert);
        self.wf.tuner.clear();
        self.send(Command::Tuner(Some(insert)));
    }

    fn tuner_window(&mut self, ctx: &egui::Context) {
        let Some(insert) = self.wf.tuner.insert else {
            return;
        };
        let name = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.mixer.get(insert))
            .map_or(format!("insert {insert}"), |t| {
                if t.name.is_empty() {
                    format!("insert {insert}")
                } else {
                    t.name.clone()
                }
            });
        let mut open = true;
        egui::Window::new("Tuner")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| tuner::draw(ui, &mut self.wf.tuner, &name));
        if !open {
            self.wf.tuner.insert = None;
            self.send(Command::Tuner(None));
        }
    }

    /// The A/B switch and reference loading, shown above the mixer.
    pub(crate) fn reference_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let has = self.wf.reference.is_some();
            let label = if self.wf.ab { "B: reference" } else { "A: mix" };
            let r = ui
                .add_enabled(has, egui::Button::selectable(self.wf.ab, label))
                .on_hover_text(
                    "Switch between your mix (A) and the reference track (B), matched in loudness",
                )
                .on_disabled_hover_text("Load a reference track to compare your mix with");
            if r.clicked() {
                self.set_ab(!self.wf.ab);
            }
            if ui
                .button(if has {
                    "Reference..."
                } else {
                    "Load reference..."
                })
                .on_hover_text("A finished song to compare your mix with; it is never exported")
                .clicked()
            {
                self.wf.reference_open = true;
            }
            if let Some(n) = self.wf.note.as_deref() {
                ui.label(RichText::new(n).weak());
            }
        });
    }

    fn set_ab(&mut self, on: bool) {
        self.wf.ab = on && self.wf.reference.is_some();
        self.send(Command::ReferenceOn(self.wf.ab));
    }

    fn load_reference(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav", "wave"])
            .set_title("Load a reference track")
            .pick_file()
        else {
            return;
        };
        let name = path
            .file_stem()
            .map_or("Reference".into(), |s| s.to_string_lossy().into_owned());
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| reference::Reference::from_wav(&name, &b))
        {
            Ok(r) => self.install_reference(r),
            Err(e) => self.wf.note = Some(format!("{}: {e}", path.display())),
        }
    }

    pub(crate) fn install_reference(&mut self, r: reference::Reference) {
        let bpm = self
            .project
            .loaded
            .as_ref()
            .map_or(120.0, |l| l.project.bpm);
        self.send(Command::Reference(Some(Box::new(RefTrack {
            audio: Preview {
                left: r.left.clone(),
                right: r.right.clone(),
                rate: r.rate,
            },
            gain: r.gain(),
            bpm,
        }))));
        // measure the mix to match the levels
        if let Some(l) = &self.project.loaded {
            let (p, s) = (l.project.clone(), l.samples.clone());
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(reference::mix_loudness(&p, &s));
            });
            self.wf.mix_loudness = Some(rx);
        }
        self.wf.reference = Some(r);
    }

    fn reference_window(&mut self, ctx: &egui::Context) {
        if !self.wf.reference_open {
            return;
        }
        let mut open = true;
        let (mut load, mut clear, mut ab) = (false, false, None);
        egui::Window::new("Reference track")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Compare your mix with a finished song while it plays.");
                ui.label(
                    RichText::new("The reference skips the master's effects, is matched to your mix's loudness and is never exported.")
                        .weak()
                        .size(11.0),
                );
                ui.add_space(6.0);
                match &self.wf.reference {
                    Some(r) => {
                        ui.label(RichText::new(&r.name).strong());
                        let mix = r
                            .mix_lufs
                            .map_or("measuring the mix...".to_string(), |m| format!("mix {m:.1} LUFS"));
                        ui.label(format!(
                            "{:.1} LUFS, {mix}, gain {:+.1} dB",
                            r.lufs,
                            20.0 * f64::from(r.gain()).log10()
                        ));
                        ui.horizontal(|ui| {
                            if ui.selectable_label(!self.wf.ab, "A: mix").clicked() {
                                ab = Some(false);
                            }
                            if ui.selectable_label(self.wf.ab, "B: reference").clicked() {
                                ab = Some(true);
                            }
                            if ui.button("Remove").clicked() {
                                clear = true;
                            }
                        });
                    }
                    None => {
                        ui.label(RichText::new("No reference loaded").weak());
                    }
                }
                if ui.button("Load a WAV file...").clicked() {
                    load = true;
                }
            });
        if let Some(on) = ab {
            self.set_ab(on);
        }
        if load {
            self.load_reference();
        }
        if clear {
            self.wf.reference = None;
            self.set_ab(false);
            self.send(Command::Reference(None));
        }
        self.wf.reference_open = open;
    }

    fn restore_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self
            .wf
            .safety
            .as_ref()
            .filter(|s| s.crashed)
            .map(|s| s.autosave_path())
        else {
            return;
        };
        let mut choice = None;
        egui::Window::new("Restore last autosave?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Lumen did not close properly last time.");
                ui.label("The project was autosaved a few minutes before. Bring it back?");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Restore last autosave").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Start fresh").clicked() {
                        choice = Some(false);
                    }
                });
            });
        let Some(restore) = choice else { return };
        if let Some(s) = &mut self.wf.safety {
            s.crashed = false;
        }
        if restore {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    self.stop_project();
                    match self.project.open_bytes(&bytes, "") {
                        Ok(()) => {
                            self.project.path.clear();
                            self.project.dirty = true;
                            self.bound = None;
                            self.ready_project();
                            self.wf.note =
                                Some("Restored the autosave: save it with Ctrl+S".into());
                        }
                        Err(e) => self.project.error = Some(e),
                    }
                }
                Err(e) => self.project.error = Some(e.to_string()),
            }
        }
    }

    fn template_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.wf.template_name.take() else {
            return;
        };
        let (mut ok, mut cancel) = (false, false);
        egui::Window::new("Save as template")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Name of the template:");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(220.0));
                // Enter confirms (the box asks for the focus every frame, so it never
                // shows as lost)
                ok |= (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                r.request_focus();
                cancel |= ui.input(|i| i.key_pressed(Key::Escape));
                ui.horizontal(|ui| {
                    ok |= ui.button("Save").clicked();
                    cancel |= ui.button("Cancel").clicked();
                });
            });
        if cancel {
            return;
        }
        if !ok {
            self.wf.template_name = Some(name);
            return;
        }
        let (Some(dir), Some(l)) = (templates::dir("templates"), &self.project.loaded) else {
            return;
        };
        let file = dir.join(format!("{}.las", templates::file_name(&name)));
        let r = las_project::encode(&l.project, &l.samples, true)
            .map_err(|e| e.to_string())
            .and_then(|b| {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                std::fs::write(&file, b).map_err(|e| e.to_string())
            });
        self.wf.note = Some(match r {
            Ok(()) => format!("Saved the template \"{}\"", name.trim()),
            Err(e) => format!("Template: {e}"),
        });
    }
}
