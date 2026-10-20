// SPDX-License-Identifier: GPL-3.0-or-later
//! The main menu (File, Edit, Add, Patterns, View, Options, Help) and what its items
//! do: new, open, save as, the demo song, channels and patterns.

use crate::app::App;
use crate::audio::Command;
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

impl App {
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

    #[test]
    fn the_demo_song_is_a_project() {
        let b = las_project::decode(DEMO).unwrap();
        assert!(!b.project.channels.is_empty());
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
