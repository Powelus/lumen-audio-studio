// SPDX-License-Identifier: GPL-3.0-or-later
//! The main menu (File, Edit, Add, Patterns, View, Options, Help) and what its items
//! do: new, open, save as, the demo song, channels and patterns.

use crate::app::App;
use crate::audio::Command;
use crate::project::RenameTarget;
use crate::theme;
use eframe::egui::{self, RichText, Ui};
use las_project::create::{add_channel, default_project, pattern, INSTRUMENTS};
use las_project::Project;

/// The demo song of the web version.
const DEMO: &[u8] = include_bytes!("../../las-project/tests/fixtures/demo.las");

/// Something that replaces the open project, and so asks about unsaved changes first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    New,
    Open,
    Demo,
    Quit,
}

/// Give something of a project a new name.
pub fn rename(p: &mut Project, target: RenameTarget, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    match target {
        RenameTarget::Channel(i) => {
            if let Some(c) = p.channels.get_mut(i) {
                c.name = name.to_string();
            }
        }
        RenameTarget::Pattern(i) => {
            if let Some(x) = p.patterns.get_mut(i) {
                x.name = name.to_string();
            }
        }
        RenameTarget::Track(i) => {
            while p.playlist.tracks.len() <= i {
                let n = p.playlist.tracks.len();
                p.playlist
                    .tracks
                    .push(las_project::create::playlist_track(n));
            }
            p.playlist.tracks[i].name = name.to_string();
        }
        RenameTarget::Marker(id) => {
            if let Some(m) = p.markers.iter_mut().flatten().find(|m| m.id == id) {
                m.name = name.chars().take(80).collect();
            }
        }
    }
}

/// A copy of a pattern with new ids, placed after it.
pub fn clone_pattern(p: &mut Project, index: usize) -> Option<usize> {
    let mut copy = p.patterns.get(index)?.clone();
    copy.id = las_project::create::uid("pat_");
    copy.name = format!("{} copy", copy.name);
    for notes in copy.notes.values_mut() {
        for n in notes {
            n.id = las_project::create::uid("n");
        }
    }
    p.patterns.insert(index + 1, copy);
    Some(index + 1)
}

/// Remove a pattern and the playlist clips of it. The last pattern stays.
pub fn delete_pattern(p: &mut Project, index: usize) -> bool {
    if p.patterns.len() <= 1 || index >= p.patterns.len() {
        return false;
    }
    let id = p.patterns.remove(index).id;
    p.playlist.clips.retain(|c| c.ref_id != id);
    true
}

/// Add a WAV file to a project as a sampler channel of its own. The file's bytes go
/// into the project's samples; returns the channel's index.
pub fn import_wav(
    p: &mut Project,
    samples: &mut std::collections::BTreeMap<String, las_project::Sample>,
    name: &str,
    bytes: Vec<u8>,
) -> Result<usize, String> {
    let audio =
        las_engine::library::decode_wav(&bytes).ok_or("not a WAV file this app can read")?;
    let id = las_project::create::uid("smp_");
    p.samples.push(las_project::SampleRef {
        id: id.clone(),
        name: name.to_string(),
        duration: Some(audio.frames() as f64 / audio.sample_rate),
        ..las_project::SampleRef::default()
    });
    samples.insert(
        id.clone(),
        las_project::Sample {
            name: name.to_string(),
            mime: "audio/wav".into(),
            data: bytes,
        },
    );
    let at = add_channel(p, "sampler");
    p.channels[at].name = name.to_string();
    p.channels[at].sample_id = Some(id);
    let insert = p.channels[at].mixer_track;
    if let Some(t) = p.mixer.get_mut(insert) {
        t.name = name.to_string();
    }
    Ok(at)
}

/// Add a WAV file to a project as an audio clip in the playlist, as FL Studio does when
/// a sample is dropped on it: an audio clip channel plays the file, and a clip as long
/// as the file (at the project's tempo) sits at `at` ticks on `track`. Returns the
/// channel's index.
pub fn import_audio_clip(
    p: &mut Project,
    samples: &mut std::collections::BTreeMap<String, las_project::Sample>,
    name: &str,
    bytes: Vec<u8>,
    at: f64,
    track: usize,
) -> Result<usize, String> {
    let audio =
        las_engine::library::decode_wav(&bytes).ok_or("not a WAV file this app can read")?;
    let seconds = audio.frames() as f64 / audio.sample_rate;
    let id = las_project::create::uid("smp_");
    p.samples.push(las_project::SampleRef {
        id: id.clone(),
        name: name.to_string(),
        duration: Some(seconds),
        ..las_project::SampleRef::default()
    });
    samples.insert(
        id.clone(),
        las_project::Sample {
            name: name.to_string(),
            mime: "audio/wav".into(),
            data: bytes,
        },
    );
    let ch = add_channel(p, "audioclip");
    p.channels[ch].name = name.to_string();
    p.channels[ch].sample_id = Some(id);
    let insert = p.channels[ch].mixer_track;
    if let Some(t) = p.mixer.get_mut(insert) {
        t.name = name.to_string();
    }
    let ticks = seconds * p.bpm / 60.0 * f64::from(las_project::PPQ);
    p.playlist.clips.push(las_project::Clip {
        id: las_project::create::uid("clip_"),
        kind: las_project::ClipKind::Audio,
        ref_id: p.channels[ch].id.clone(),
        track,
        start: at.max(0.0),
        length: ticks.max(1.0),
        ..Default::default()
    });
    Ok(ch)
}

/// Add a factory sample as a sampler channel.
pub fn add_factory(p: &mut Project, id: &str, name: &str) -> usize {
    if !p.samples.iter().any(|s| s.id == id) {
        p.samples.push(las_project::SampleRef {
            id: id.to_string(),
            name: name.to_string(),
            factory: Some(true),
            ..las_project::SampleRef::default()
        });
    }
    let at = add_channel(p, "sampler");
    p.channels[at].name = name.to_string();
    p.channels[at].sample_id = Some(id.to_string());
    let insert = p.channels[at].mixer_track;
    if let Some(t) = p.mixer.get_mut(insert) {
        t.name = name.to_string();
    }
    at
}

impl App {
    /// Pick WAV files and add each as a sampler channel.
    pub(crate) fn import_dialog(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav", "wave"])
            .set_title("Import samples")
            .pick_files();
        for f in files.into_iter().flatten() {
            self.import_path(&f);
        }
    }

    /// Add the WAV file at `path` as a sampler channel.
    pub(crate) fn import_path(&mut self, path: &std::path::Path) {
        let name = path
            .file_stem()
            .map_or("Sample".to_string(), |s| s.to_string_lossy().into_owned());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.project.error = Some(format!("{}: {e}", path.display()));
                return;
            }
        };
        let mut result = Ok(0);
        self.project.modify_with_samples(true, |p, samples| {
            result = import_wav(p, samples, &name, bytes);
        });
        match result {
            Ok(at) => {
                self.project.channel = at;
                self.project.error = None;
            }
            Err(e) => self.project.error = Some(format!("{}: {e}", path.display())),
        }
    }

    /// Add the WAV file at `path` as an audio clip at `at` ticks on `track`.
    pub(crate) fn import_clip_path(&mut self, path: &std::path::Path, at: f64, track: usize) {
        let name = path
            .file_stem()
            .map_or("Audio".to_string(), |s| s.to_string_lossy().into_owned());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.project.error = Some(format!("{}: {e}", path.display()));
                return;
            }
        };
        let mut result = Ok(0);
        self.project.modify_with_samples(true, |p, samples| {
            result = import_audio_clip(p, samples, &name, bytes, at, track);
        });
        match result {
            Ok(ch) => {
                self.project.channel = ch;
                self.project.error = None;
            }
            Err(e) => self.project.error = Some(format!("{}: {e}", path.display())),
        }
    }

    pub(crate) fn add_factory_channel(&mut self, id: &str, name: &str) {
        let mut at = 0;
        self.project.modify(true, |p| at = add_factory(p, id, name));
        self.project.channel = at;
    }

    /// Run an action, asking first when it would lose changes.
    pub(crate) fn guard(&mut self, a: Action) {
        if self.project.dirty {
            self.shell.pending = Some(a);
        } else {
            self.run(a);
        }
    }

    fn run(&mut self, a: Action) {
        match a {
            Action::New => self.new_project(),
            Action::Open => self.open_dialog(),
            Action::Demo => self.open_demo(),
            Action::Quit => self.quit = true,
        }
    }

    pub(crate) fn new_project(&mut self) {
        self.stop_project();
        self.project
            .install(default_project(), Default::default(), "", Vec::new());
        self.project.path.clear();
        self.bound = None;
        self.ready_project();
    }

    pub(crate) fn open_demo(&mut self) {
        self.stop_project();
        match self.project.open_bytes(DEMO, "") {
            Ok(()) => {
                self.project.path.clear();
                self.bound = None;
                self.ready_project();
            }
            Err(e) => self.project.error = Some(e),
        }
    }

    pub(crate) fn open_dialog(&mut self) {
        let f = rfd::FileDialog::new()
            .add_filter("Lumen project", &["las"])
            .set_title("Open a project")
            .pick_file();
        if let Some(path) = f {
            self.bound = None;
            self.open_project(&path.to_string_lossy());
        }
    }

    pub(crate) fn save_as_dialog(&mut self) {
        let name = self
            .project
            .loaded
            .as_ref()
            .map_or("Untitled".to_string(), |l| l.project.name.clone());
        let f = rfd::FileDialog::new()
            .add_filter("Lumen project", &["las"])
            .set_file_name(format!("{name}.las"))
            .set_title("Save the project as")
            .save_file();
        if let Some(mut path) = f {
            if path.extension().is_none() {
                path.set_extension("las");
            }
            match self.project.save_to(&path.to_string_lossy()) {
                Ok(()) => self.project.error = None,
                Err(e) => self.project.error = Some(e),
            }
        }
    }

    /// The question about unsaved changes, when one is pending.
    pub(crate) fn discard_dialog(&mut self, ctx: &egui::Context) {
        let Some(a) = self.shell.pending else {
            return;
        };
        let mut choice = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("This project has changes that are not saved.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(false);
                    }
                    if ui.button("Cancel").clicked() {
                        self.shell.pending = None;
                    }
                });
            });
        if let Some(save) = choice {
            self.shell.pending = None;
            if save {
                self.save_project();
                if self.project.dirty {
                    return; // the save did not happen
                }
            }
            self.run(a);
        }
    }

    /// The window that asks for a name, when one is wanted.
    pub(crate) fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some((target, mut text)) = self.project.rename.take() else {
            return;
        };
        let (mut ok, mut cancel) = (false, false);
        egui::Window::new("Rename")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(220.0));
                r.request_focus();
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                ui.horizontal(|ui| {
                    ok |= ui.button("OK").clicked();
                    cancel |= ui.button("Cancel").clicked();
                });
            });
        if ok {
            let name = text.clone();
            self.project.modify(true, move |p| rename(p, target, &name));
        } else if !cancel {
            self.project.rename = Some((target, text));
        }
    }

    /// Every step undo and redo can reach; a click goes to that step.
    pub(crate) fn history_window(&mut self, ctx: &egui::Context) {
        let mut open = self.shell.open.history;
        let (back, forward) = self.project.history();
        let mut go = None;
        egui::Window::new("History")
            .open(&mut open)
            .default_size([260.0, 360.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let n = back.len() as i32;
                    if ui.selectable_label(false, "Opened").clicked() {
                        go = Some(-n);
                    }
                    for (i, label) in back.iter().enumerate() {
                        let steps = i as i32 + 1 - n;
                        if ui.selectable_label(steps == 0, label).clicked() && steps != 0 {
                            go = Some(steps);
                        }
                    }
                    for (i, label) in forward.iter().enumerate() {
                        if ui
                            .add(egui::Button::selectable(
                                false,
                                RichText::new(label).weak().italics(),
                            ))
                            .clicked()
                        {
                            go = Some(i as i32 + 1);
                        }
                    }
                });
            });
        if let Some(s) = go {
            self.project.travel(s);
        }
        self.shell.open.history = open;
    }

    pub(crate) fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.shell.open.about;
        egui::Window::new("About")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new("Lumen Audio Studio").strong().size(18.0));
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(4.0);
                ui.label("A free, open source digital audio workstation.");
                ui.label("Licensed under the GNU General Public License, version 3 or later.");
                ui.hyperlink("https://github.com/Powelus/lumen-audio-studio");
            });
        self.shell.open.about = open;
    }

    pub(crate) fn main_menu(&mut self, ui: &mut Ui) {
        let loaded = self.project.loaded.is_some();
        ui.menu_button("File", |ui| {
            if ui.button("New          Ctrl+N").clicked() {
                self.guard(Action::New);
                ui.close();
            }
            if ui.button("Open...      Ctrl+O").clicked() {
                self.guard(Action::Open);
                ui.close();
            }
            if ui.button("Open demo song").clicked() {
                self.guard(Action::Demo);
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(loaded, egui::Button::new("Save          Ctrl+S"))
                .clicked()
            {
                self.save_project();
                ui.close();
            }
            if ui
                .add_enabled(loaded, egui::Button::new("Save as...  Ctrl+Shift+S"))
                .clicked()
            {
                self.save_as_dialog();
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(loaded, egui::Button::new("Import samples..."))
                .clicked()
            {
                self.import_dialog();
                ui.close();
            }
            if ui
                .add_enabled(loaded, egui::Button::new("Export...    Ctrl+R"))
                .clicked()
            {
                self.shell.open.export = true;
                ui.close();
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                self.guard(Action::Quit);
                ui.close();
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.button("Undo          Ctrl+Z").clicked() {
                self.project.undo();
                ui.close();
            }
            if ui.button("Redo          Ctrl+Shift+Z").clicked() {
                self.project.redo();
                ui.close();
            }
            ui.separator();
            if ui.button("History...").clicked() {
                self.shell.open.history = true;
                ui.close();
            }
        });
        ui.menu_button("Add", |ui| {
            for (id, name, _) in INSTRUMENTS {
                if ui.add_enabled(loaded, egui::Button::new(*name)).clicked() {
                    self.add_channel(id);
                    ui.close();
                }
            }
        });
        ui.menu_button("Patterns", |ui| {
            if ui
                .add_enabled(loaded, egui::Button::new("New pattern     F4"))
                .clicked()
            {
                self.new_pattern();
                ui.close();
            }
            if ui
                .add_enabled(loaded, egui::Button::new("Rename pattern..."))
                .clicked()
            {
                let i = self.project.pattern;
                let name = self
                    .project
                    .loaded
                    .as_ref()
                    .and_then(|l| l.project.patterns.get(i))
                    .map(|x| x.name.clone());
                if let Some(name) = name {
                    self.project.rename = Some((RenameTarget::Pattern(i), name));
                }
                ui.close();
            }
            if ui
                .add_enabled(loaded, egui::Button::new("Clone pattern"))
                .clicked()
            {
                let i = self.project.pattern;
                self.project.modify(true, |p| {
                    clone_pattern(p, i);
                });
                self.project.pattern = i + 1;
                ui.close();
            }
            if ui
                .add_enabled(loaded, egui::Button::new("Delete pattern"))
                .clicked()
            {
                let i = self.project.pattern;
                self.project.modify(true, |p| {
                    delete_pattern(p, i);
                });
                self.project.pattern = i.saturating_sub(1);
                ui.close();
            }
        });
        ui.menu_button("View", |ui| {
            let o = self.shell.open;
            for (label, on) in [
                ("Playlist     F5", o.playlist),
                ("Channel rack F6", o.rack),
                ("Piano roll   F7", o.roll),
                ("Mixer        F9", o.mixer),
                ("Synth", o.synth),
            ] {
                if ui.selectable_label(on, label).clicked() {
                    let w = &mut self.shell.open;
                    match label.split_whitespace().next().unwrap_or("") {
                        "Playlist" => w.playlist = !on,
                        "Channel" => w.rack = !on,
                        "Piano" => w.roll = !on,
                        "Mixer" => w.mixer = !on,
                        _ => w.synth = !on,
                    }
                    ui.close();
                }
            }
            let b = self.shell.browser;
            if ui.selectable_label(b, "Browser      F8").clicked() {
                self.shell.browser = !b;
                ui.close();
            }
        });
        ui.menu_button("Options", |ui| {
            ui.menu_button("Theme", |ui| {
                let current = self.shell.theme.clone();
                for t in theme::themes() {
                    if ui
                        .selectable_label(current == t.id, t.name)
                        .on_hover_text(t.description)
                        .clicked()
                    {
                        self.choose_theme(t.id);
                        ui.close();
                    }
                }
                if ui
                    .selectable_label(current == theme::SYSTEM, "Match the system")
                    .clicked()
                {
                    self.choose_theme(theme::SYSTEM);
                    ui.close();
                }
            });
        });
        ui.menu_button("Help", |ui| {
            if ui.button("About").clicked() {
                self.shell.open.about = true;
                ui.close();
            }
        });
    }

    pub(crate) fn new_pattern(&mut self) {
        let n = self
            .project
            .loaded
            .as_ref()
            .map_or(0, |l| l.project.patterns.len());
        self.project.modify(true, |p| {
            let mut x = pattern(n, None);
            x.name = format!("Pattern {}", n + 1);
            p.patterns.push(x);
        });
        self.project.pattern = n;
    }

    pub(crate) fn add_channel(&mut self, plugin: &str) {
        let mut at = 0;
        self.project.modify(true, |p| at = add_channel(p, plugin));
        self.project.channel = at;
        self.send(Command::PreviewStop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(seconds: f32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
        for i in 0..(24000.0 * seconds) as usize {
            let v = (i as f32 * 0.05).sin() * 0.6 * (1.0 - i as f32 / (24000.0 * seconds));
            w.write_sample((v * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        out.into_inner()
    }

    #[test]
    fn a_wav_dropped_on_the_playlist_becomes_an_audio_clip_that_plays_there() {
        let mut p = las_project::create::empty_project("x");
        p.bpm = 120.0;
        let mut samples = Default::default();
        // one bar in at 120 BPM is two seconds; the file lasts one second
        let bar = f64::from(las_project::PPQ) * 4.0;
        let ch = import_audio_clip(&mut p, &mut samples, "Vox", wav(1.0), bar, 2).unwrap();
        let c = &p.channels[ch];
        assert_eq!(
            (c.name.as_str(), c.plugin_id.as_str()),
            ("Vox", "audioclip")
        );
        let clip = p.playlist.clips.last().unwrap();
        assert_eq!(clip.kind, las_project::ClipKind::Audio);
        assert_eq!((clip.ref_id.as_str(), clip.track), (c.id.as_str(), 2));
        assert_eq!(clip.start, bar);
        // one second at 120 BPM is two beats
        assert!((clip.length - f64::from(las_project::PPQ) * 2.0).abs() < 1.0);
        let r = las_engine::render_mix_with(
            &p,
            &samples,
            &las_engine::RenderOptions {
                sample_rate: 24000.0,
                tail: 0.2,
                ..Default::default()
            },
        )
        .unwrap();
        let level = |a: f64, b: f64| {
            r.left[(a * 24000.0) as usize..(b * 24000.0) as usize]
                .iter()
                .fold(0f32, |m, s| m.max(s.abs()))
        };
        assert!(level(0.0, 1.9) < 1e-4, "silent before the clip");
        assert!(
            level(2.0, 2.5) > 0.1,
            "the clip sounds where it was dropped"
        );
        assert!(import_audio_clip(&mut p, &mut samples, "x", b"no".to_vec(), 0.0, 0).is_err());
    }

    #[test]
    fn an_imported_wav_becomes_a_sampler_channel_that_plays() {
        let mut p = default_project();
        let mut samples = Default::default();
        let at = import_wav(&mut p, &mut samples, "My hit", wav(0.5)).unwrap();
        let c = &p.channels[at];
        assert_eq!(
            (c.name.as_str(), c.plugin_id.as_str()),
            ("My hit", "sampler")
        );
        let id = c.sample_id.clone().unwrap();
        assert!(samples.contains_key(&id));
        assert!(p.samples.iter().any(|s| s.id == id && s.duration.is_some()));
        assert_eq!(p.mixer[c.mixer_track].name, "My hit");
        // it sounds: a note on the new channel in a pattern of its own
        p.patterns[0].notes.clear();
        let ch = c.id.clone();
        p.patterns[0]
            .notes
            .insert(ch, vec![las_project::create::step_note(0, 60, 0.9)]);
        let r = las_engine::render_mix_with(
            &p,
            &samples,
            &las_engine::RenderOptions {
                sample_rate: 24000.0,
                tail: 0.3,
                ..Default::default()
            },
        )
        .unwrap();
        let peak = r.left.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05, "the imported sample is heard, peak {peak}");
        // something that is not a WAV is refused and changes nothing
        let before = p.channels.len();
        assert!(import_wav(&mut p, &mut samples, "x", b"nope".to_vec()).is_err());
        assert_eq!(p.channels.len(), before);
    }

    #[test]
    fn a_factory_sample_is_added_once_as_a_reference() {
        let mut p = default_project();
        let n = p.samples.len();
        let a = add_factory(&mut p, "factory:kick-deep", "Kick Deep");
        let b = add_factory(&mut p, "factory:kick-deep", "Kick Deep");
        assert_eq!(p.samples.len(), n + 1);
        assert_ne!(a, b);
        assert_eq!(
            p.channels[b].sample_id.as_deref(),
            Some("factory:kick-deep")
        );
    }

    #[test]
    fn the_demo_song_is_a_project() {
        let b = las_project::decode(DEMO).unwrap();
        assert!(!b.project.channels.is_empty());
    }

    #[test]
    fn things_are_renamed_and_blank_names_are_ignored() {
        let mut p = default_project();
        rename(&mut p, RenameTarget::Channel(0), "  Big kick ");
        rename(&mut p, RenameTarget::Pattern(0), "Intro");
        rename(&mut p, RenameTarget::Track(30), "Drums");
        assert_eq!(p.channels[0].name, "Big kick");
        assert_eq!(p.patterns[0].name, "Intro");
        assert_eq!(p.playlist.tracks[30].name, "Drums");
        rename(&mut p, RenameTarget::Channel(0), "   ");
        rename(&mut p, RenameTarget::Channel(99), "x");
        assert_eq!(p.channels[0].name, "Big kick");
    }

    #[test]
    fn patterns_clone_with_new_ids_and_delete_with_their_clips() {
        let mut p = default_project();
        let id0 = p.patterns[0].id.clone();
        let n0 = p.patterns[0].notes.values().next().unwrap()[0].id.clone();
        assert_eq!(clone_pattern(&mut p, 0), Some(1));
        assert_eq!(p.patterns.len(), 2);
        assert_ne!(p.patterns[1].id, id0);
        assert_ne!(p.patterns[1].notes.values().next().unwrap()[0].id, n0);
        assert_eq!(p.patterns[1].name, "Pattern 1 copy");
        assert_eq!(clone_pattern(&mut p, 9), None);
        assert!(delete_pattern(&mut p, 0));
        assert!(p.playlist.clips.is_empty(), "its clip went with it");
        assert!(!delete_pattern(&mut p, 0), "the last pattern stays");
    }
}
