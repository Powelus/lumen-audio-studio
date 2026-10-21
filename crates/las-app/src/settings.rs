// SPDX-License-Identifier: GPL-3.0-or-later
//! The app's settings, kept in `settings.json` in the app's folder: the user's sample
//! folders (shown in the browser), extra plugin folders to scan, autosave, and the
//! projects opened lately. The Settings window (Options > Settings) edits them.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{self, RichText, Ui};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How many projects File > Open recent lists.
pub const RECENT: usize = 10;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Folders of samples, shown in the browser's Samples tab.
    pub sample_folders: Vec<PathBuf>,
    /// Folders to look for CLAP and VST3 plugins in, besides the standard ones.
    pub plugin_folders: Vec<PathBuf>,
    /// Whether the standard plugin folders (and CLAP_PATH, VST3_PATH) are scanned too.
    pub standard_plugin_folders: bool,
    /// Minutes between autosaves of a project with changes; 0 for none.
    pub autosave_minutes: u32,
    /// Projects opened or saved lately, the latest first.
    pub recent: Vec<String>,
    /// Which windows were open when the app closed.
    pub windows: Vec<String>,
    /// The size of everything, in percent of what suits the screen (100 by default).
    pub ui_scale: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sample_folders: Vec::new(),
            plugin_folders: Vec::new(),
            standard_plugin_folders: true,
            autosave_minutes: 5,
            recent: Vec::new(),
            windows: Vec::new(),
            ui_scale: 100,
        }
    }
}

fn file() -> Option<PathBuf> {
    Some(crate::presets::config_dir()?.join("settings.json"))
}

impl Settings {
    /// The saved settings, or the defaults.
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(f, b);
        }
    }

    /// Where plugins are looked for.
    pub fn plugin_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = if self.standard_plugin_folders {
            las_host::all_search_paths()
        } else {
            Vec::new()
        };
        for d in &self.plugin_folders {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        dirs
    }

    /// Put a project at the top of the recent list.
    pub fn remember(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(RECENT);
    }

    /// Add a folder to a list once.
    pub fn add_folder(list: &mut Vec<PathBuf>, dir: PathBuf) -> bool {
        if list.contains(&dir) {
            return false;
        }
        list.push(dir);
        true
    }
}

/// The layout is made for this many pixels; a screen of another size scales it.
pub const DESIGN: (f32, f32) = (1280.0, 800.0);

/// Pixels per point for a window of `physical` pixels: the layout of [`DESIGN`] fills
/// it, never smaller than 90% (so 720p still fits) and never larger than 300%, times
/// the user's own scale in percent.
pub fn auto_scale(physical: (f32, f32), percent: u32) -> f32 {
    let fit = (physical.0 / DESIGN.0).min(physical.1 / DESIGN.1);
    (fit.clamp(0.9, 3.0) * percent.clamp(50, 200) as f32 / 100.0).max(0.5)
}

/// What a folder holds: its folders and its sound files, sorted by name.
#[derive(Clone, Default)]
pub struct Listing {
    pub dirs: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

/// Whether the app can read a file as a sample.
pub fn is_sound(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
}

pub fn list(dir: &Path) -> Listing {
    let mut l = Listing::default();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return l;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            l.dirs.push(p);
        } else if is_sound(&p) {
            l.files.push(p);
        }
    }
    let key = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    l.dirs.sort_by_key(key);
    l.files.sort_by_key(key);
    l
}

/// The sound files under a folder whose name has `query` in it (up to `limit`).
pub fn search(dir: &Path, query: &str, limit: usize) -> Vec<PathBuf> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let l = list(&d);
        for f in l.files {
            let hit = f
                .file_stem()
                .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q));
            if hit {
                out.push(f);
                if out.len() >= limit {
                    return out;
                }
            }
        }
        stack.extend(l.dirs.into_iter().rev());
    }
    out
}

/// The browser's view of the user's folders: listings read once, when first opened.
#[derive(Default)]
pub struct FolderCache {
    listings: BTreeMap<PathBuf, Listing>,
}

impl FolderCache {
    pub fn get(&mut self, dir: &Path) -> &Listing {
        self.listings
            .entry(dir.to_path_buf())
            .or_insert_with(|| list(dir))
    }

    pub fn clear(&mut self) {
        self.listings.clear();
    }
}

/// When the project was last autosaved.
pub struct Autosave {
    pub last: Instant,
}

impl Default for Autosave {
    fn default() -> Self {
        Self {
            last: Instant::now(),
        }
    }
}

/// The folder autosaves go to.
pub fn autosave_dir() -> Option<PathBuf> {
    Some(crate::presets::config_dir()?.join("autosave"))
}

impl App {
    /// Size everything for the window: text and shapes are drawn at the real pixel
    /// size, so a big screen gets a sharp, larger interface rather than a blown up one.
    pub(crate) fn scale_ui(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let size = ctx.content_rect().size() * ppp;
        if size.x < 10.0 || size.y < 10.0 {
            return;
        }
        let want = auto_scale((size.x, size.y), self.settings.ui_scale);
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let zoom = want / native;
        if (ctx.zoom_factor() - zoom).abs() > 0.01 {
            ctx.set_zoom_factor(zoom);
        }
    }

    /// Rescan the plugin folders, in the background.
    pub(crate) fn rescan_plugins(&self) {
        let dirs = self.settings.plugin_dirs();
        std::thread::spawn(move || {
            las_host::rescan(&dirs);
        });
    }

    /// Save a copy of a project with changes every few minutes, to the autosave folder
    /// (the project's own file is left alone).
    pub(crate) fn autosave(&mut self) {
        let every = self.settings.autosave_minutes;
        if every == 0 || self.autosaved.last.elapsed() < Duration::from_secs(u64::from(every) * 60)
        {
            return;
        }
        self.autosaved.last = Instant::now();
        if !self.project.dirty {
            return;
        }
        let Some(dir) = autosave_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let Some(l) = &self.project.loaded else {
            return;
        };
        let name = Path::new(&l.path)
            .file_stem()
            .map_or("Untitled".to_string(), |s| s.to_string_lossy().into_owned());
        let Ok(bytes) = las_project::encode(&l.project, &l.samples, true) else {
            return;
        };
        let to = dir.join(format!("{name}.autosave.las"));
        if std::fs::write(&to, bytes).is_ok() {
            self.status = format!("Autosaved to {}", to.display());
        }
    }

    /// The windows open now, by name.
    fn open_windows(&self) -> Vec<String> {
        let o = &self.shell.open;
        [
            ("browser", self.shell.browser),
            ("rack", o.rack),
            ("playlist", o.playlist),
            ("roll", o.roll),
            ("mixer", o.mixer),
        ]
        .iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| n.to_string())
        .collect()
    }

    /// Keep the list of open windows in the settings, for the next start.
    pub(crate) fn remember_windows(&mut self) {
        let now = self.open_windows();
        if now != self.settings.windows {
            self.settings.windows = now;
            self.settings.save();
        }
    }

    /// Open the windows that were open last time.
    pub(crate) fn restore_windows(&mut self) {
        if self.settings.windows.is_empty() {
            return;
        }
        let has = |n: &str| self.settings.windows.iter().any(|w| w == n);
        let (browser, rack, playlist, roll, mixer) = (
            has("browser"),
            has("rack"),
            has("playlist"),
            has("roll"),
            has("mixer"),
        );
        self.shell.browser = browser;
        let o = &mut self.shell.open;
        o.rack = rack;
        o.playlist = playlist;
        o.roll = roll;
        o.mixer = mixer;
    }

    /// The Settings window.
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.shell.open.settings;
        let mut changed = false;
        let mut rescan = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_size([560.0, 460.0])
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                let s = &mut self.settings;
                ui.heading("Sample folders");
                ui.label(
                    RichText::new("Shown in the browser's Samples tab, under the factory kit.")
                        .weak(),
                );
                changed |= folder_list(ui, "samples", &mut s.sample_folders, "Add sample folder");
                ui.add_space(10.0);
                ui.heading("Plugin folders");
                changed |= ui
                    .checkbox(
                        &mut s.standard_plugin_folders,
                        "Also scan the standard CLAP and VST3 folders",
                    )
                    .changed();
                changed |= folder_list(ui, "plugins", &mut s.plugin_folders, "Add plugin folder");
                if ui.button("Rescan plugins now").clicked() {
                    rescan = true;
                }
                ui.add_space(10.0);
                ui.heading("Display");
                ui.horizontal(|ui| {
                    ui.label("Size of everything");
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut s.ui_scale, 50..=200)
                                .suffix("%")
                                .step_by(5.0),
                        )
                        .on_hover_text("100% fits the layout to the screen")
                        .changed();
                    if ui.small_button("Reset").clicked() {
                        s.ui_scale = 100;
                        changed = true;
                    }
                });
                ui.add_space(10.0);
                ui.heading("Saving");
                ui.horizontal(|ui| {
                    ui.label("Autosave a copy every");
                    changed |= ui
                        .add(egui::DragValue::new(&mut s.autosave_minutes).range(0..=60))
                        .changed();
                    ui.label("minutes (0 for never)");
                });
                if let Some(d) = autosave_dir() {
                    ui.label(
                        RichText::new(format!("Autosaves go to {}", d.display()))
                            .size(11.0)
                            .color(pal().muted),
                    );
                }
                if ui.button("Clear the recent projects list").clicked() {
                    s.recent.clear();
                    changed = true;
                }
            });
        self.shell.open.settings = open;
        if changed || rescan {
            self.settings.save();
            self.folders.clear();
        }
        if rescan {
            self.rescan_plugins();
        }
    }
}

/// What a click in the folder trees asks for.
pub enum FileAction {
    /// Hear the file.
    Preview(PathBuf),
    /// Add it as a sampler channel.
    Add(PathBuf),
}

fn file_row(ui: &mut Ui, f: &Path, out: &mut Option<FileAction>) {
    let name = f
        .file_stem()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let r = ui
        .selectable_label(false, name)
        .on_hover_text("Click to hear it, double click to add it as a channel");
    if r.double_clicked() {
        *out = Some(FileAction::Add(f.to_path_buf()));
    } else if r.clicked() {
        *out = Some(FileAction::Preview(f.to_path_buf()));
    }
}

/// A folder and what is in it, opened when clicked.
fn tree(ui: &mut Ui, cache: &mut FolderCache, dir: &Path, out: &mut Option<FileAction>) {
    let name = dir.file_name().map_or(dir.display().to_string(), |n| {
        n.to_string_lossy().into_owned()
    });
    egui::CollapsingHeader::new(name)
        .id_salt(dir)
        .show(ui, |ui| {
            let l = cache.get(dir).clone();
            for d in &l.dirs {
                tree(ui, cache, d, out);
            }
            for f in &l.files {
                file_row(ui, f, out);
            }
            if l.dirs.is_empty() && l.files.is_empty() {
                ui.label(RichText::new("No WAV files here.").weak());
            }
        });
}

impl App {
    /// The user's sample folders in the browser, searched by `query` when it is not empty.
    pub(crate) fn folders_ui(&mut self, ui: &mut Ui, query: &str) {
        let p = pal();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("MY FOLDERS")
                    .size(11.0)
                    .strong()
                    .color(p.muted),
            );
            if ui
                .small_button("+")
                .on_hover_text("Add a sample folder")
                .clicked()
            {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title("Add sample folder")
                    .pick_folder()
                {
                    if Settings::add_folder(&mut self.settings.sample_folders, d) {
                        self.settings.save();
                    }
                }
            }
            if ui
                .small_button("Refresh")
                .on_hover_text("Read the folders again")
                .clicked()
            {
                self.folders.clear();
            }
        });
        let mut action = None;
        let folders = self.settings.sample_folders.clone();
        if folders.is_empty() {
            ui.label(
                RichText::new("Add folders of samples with + or in Options > Settings.")
                    .weak()
                    .size(11.0),
            );
        }
        for dir in &folders {
            if query.is_empty() {
                tree(ui, &mut self.folders, dir, &mut action);
            } else {
                for f in search(dir, query, 50) {
                    file_row(ui, &f, &mut action);
                }
            }
        }
        match action {
            Some(FileAction::Preview(f)) => self.preview_file(&f),
            Some(FileAction::Add(f)) if self.project.loaded.is_some() => self.import_path(&f),
            _ => {}
        }
    }

    /// Play a WAV file through the output.
    fn preview_file(&mut self, path: &Path) {
        let Some(a) = std::fs::read(path)
            .ok()
            .and_then(|b| las_engine::library::decode_wav(&b))
        else {
            self.status = format!("{} could not be read", path.display());
            return;
        };
        let left: std::sync::Arc<[f32]> = std::sync::Arc::from(a.channels[0].clone());
        let right: std::sync::Arc<[f32]> =
            std::sync::Arc::from(a.channels.get(1).unwrap_or(&a.channels[0]).clone());
        self.send(crate::audio::Command::Preview(Box::new(
            crate::audio::Preview {
                left,
                right,
                rate: a.sample_rate,
            },
        )));
    }
}

/// A list of folders with Remove buttons and an Add button; whether it changed.
fn folder_list(ui: &mut Ui, id: &str, list: &mut Vec<PathBuf>, add: &str) -> bool {
    let mut changed = false;
    let mut remove = None;
    egui::Grid::new(("folders", id))
        .num_columns(2)
        .show(ui, |ui| {
            for (i, d) in list.iter().enumerate() {
                let missing = !d.is_dir();
                let text = RichText::new(d.display().to_string());
                ui.label(if missing {
                    text.strikethrough().weak()
                } else {
                    text
                })
                .on_hover_text(if missing {
                    "This folder is not there"
                } else {
                    ""
                });
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if list.is_empty() {
        ui.label(RichText::new("None yet.").weak());
    }
    if let Some(i) = remove {
        list.remove(i);
        changed = true;
    }
    if ui.button(add).clicked() {
        if let Some(d) = rfd::FileDialog::new().set_title(add).pick_folder() {
            changed |= Settings::add_folder(list, d);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_old_files_get_defaults() {
        let mut s = Settings::default();
        s.sample_folders.push("/a".into());
        s.remember("x.las");
        let back: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let old: Settings = serde_json::from_str(r#"{"recent": ["y.las"]}"#).unwrap();
        assert!(old.standard_plugin_folders);
        assert_eq!(old.autosave_minutes, 5);
        assert_eq!(old.recent, vec!["y.las"]);
    }

    #[test]
    fn the_layout_scales_with_the_screen() {
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        // 720p fits at 90%, the design size is 100%
        assert!(near(auto_scale((1280.0, 720.0), 100), 0.9));
        assert!(near(auto_scale((1280.0, 800.0), 100), 1.0));
        // 1080p, 1440p and 4K grow, so nothing is tiny
        assert!(near(auto_scale((1920.0, 1080.0), 100), 1.35));
        assert!(near(auto_scale((2560.0, 1440.0), 100), 1.8));
        assert!(near(auto_scale((3840.0, 2160.0), 100), 2.7));
        // a narrow window is held by its width
        assert!(near(auto_scale((1280.0, 1440.0), 100), 1.0));
        // the user's own scale on top
        assert!(near(auto_scale((1920.0, 1080.0), 120), 1.62));
        assert!(near(auto_scale((1280.0, 720.0), 50), 0.5));
    }

    #[test]
    fn recent_projects_go_to_the_top_once() {
        let mut s = Settings::default();
        for i in 0..15 {
            s.remember(&format!("{i}.las"));
        }
        s.remember("3.las");
        assert_eq!(s.recent.len(), RECENT);
        assert_eq!(s.recent[0], "3.las");
        assert_eq!(s.recent.iter().filter(|p| *p == "3.las").count(), 1);
        s.remember("");
        assert_eq!(s.recent[0], "3.las");
    }

    #[test]
    fn plugin_folders_add_to_the_standard_ones() {
        let mut s = Settings {
            standard_plugin_folders: false,
            ..Settings::default()
        };
        assert!(Settings::add_folder(&mut s.plugin_folders, "/p".into()));
        assert!(!Settings::add_folder(&mut s.plugin_folders, "/p".into()));
        assert_eq!(s.plugin_dirs(), vec![PathBuf::from("/p")]);
        s.standard_plugin_folders = true;
        let all = s.plugin_dirs();
        assert_eq!(all.last(), Some(&PathBuf::from("/p")));
        assert!(all.len() > 1);
    }

    #[test]
    fn folders_list_sound_files_and_search_finds_them_deep() {
        let root = std::env::temp_dir().join(format!("las-folders-{}", std::process::id()));
        let deep = root.join("Drums").join("Kicks");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join("b.wav"), b"").unwrap();
        std::fs::write(root.join("A.WAV"), b"").unwrap();
        std::fs::write(root.join("notes.txt"), b"").unwrap();
        std::fs::write(root.join(".hidden.wav"), b"").unwrap();
        std::fs::write(deep.join("big kick.wav"), b"").unwrap();
        let l = list(&root);
        let names: Vec<_> = l
            .files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["A.WAV", "b.wav"]);
        assert_eq!(l.dirs, vec![root.join("Drums")]);
        assert_eq!(search(&root, "KICK", 10), vec![deep.join("big kick.wav")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
