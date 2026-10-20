// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening and playing a project. A project is decoded here, a player for it is
//! built on a worker thread (building is the slow part), and the audio thread then
//! takes the player over.

use crate::app::App;
use crate::audio::Command;
use crate::mixer::{self, MixEdit};
use crate::playlist::{self, PlEdit, PlaylistState};
use crate::roll::{self, Edit, RollState};
use crate::widgets::ACCENT;
use eframe::egui::{self, Color32, RichText, Ui};
use las_engine::timeline::PPQ_F;
use las_engine::{PlayMode, Player};
use las_project::{decode, encode, Project, Sample};
use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// An open project.
pub struct Loaded {
    pub path: String,
    pub project: Arc<Project>,
    pub samples: Arc<BTreeMap<String, Sample>>,
    pub warnings: Vec<String>,
}

/// A player being built, and what to do with it when it is ready.
struct Building {
    done: Receiver<Result<Player, String>>,
    /// Where to start playing; `None` only readies the player for live keys.
    from: Option<f64>,
}

#[derive(Default)]
pub struct ProjectState {
    pub path: String,
    pub loaded: Option<Loaded>,
    building: Option<Building>,
    /// The pattern the "Play pattern" button loops.
    pub pattern: usize,
    /// The channel that live keys play.
    pub channel: usize,
    /// Whether the pattern (not the song) is what plays, for the playhead.
    pub looping: bool,
    pub error: Option<String>,
    /// Whether a player has been handed to the audio thread (so Stop has something).
    pub has_player: bool,
    /// Rack or piano roll.
    pub page: Page,
    pub roll: RollState,
    pub playlist: PlaylistState,
    /// Where "Play song" starts, in ticks.
    pub start_tick: f64,
    /// Earlier and later states of the project, for undo and redo.
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    /// Changed since it was opened or saved.
    pub dirty: bool,
    /// The player no longer matches the project and is rebuilt when things are quiet.
    stale: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Rack,
    Roll,
    Playlist,
    Mixer,
}

impl ProjectState {
    pub fn open(&mut self, path: &str) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let b = decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
        self.loaded = Some(Loaded {
            path: path.to_string(),
            project: Arc::new(b.project),
            samples: Arc::new(b.samples),
            warnings: b.warnings,
        });
        self.pattern = 0;
        self.channel = 0;
        self.start_tick = 0.0;
        self.has_player = false;
        self.building = None;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.stale = false;
        Ok(())
    }

    /// Carry out the edits of one frame on the open project.
    pub fn edit(&mut self, edits: &[Edit]) {
        let (pattern, channel) = (self.pattern, self.channel);
        let Some(l) = &mut self.loaded else { return };
        let Some(id) = l.project.channels.get(channel).map(|c| c.id.clone()) else {
            return;
        };
        for e in edits {
            match e {
                Edit::Begin => {
                    self.undo.push(l.project.clone());
                    if self.undo.len() > 200 {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                Edit::Audition { .. } => {}
                _ => {
                    roll::apply(Arc::make_mut(&mut l.project), pattern, &id, e);
                    self.dirty = true;
                    self.stale = true;
                }
            }
        }
    }

    /// Carry out the playlist edits of one frame.
    pub fn edit_playlist(&mut self, edits: &[PlEdit]) {
        let Some(l) = &mut self.loaded else { return };
        for e in edits {
            match e {
                PlEdit::Begin => {
                    self.undo.push(l.project.clone());
                    if self.undo.len() > 200 {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                PlEdit::Seek(t) => self.start_tick = *t,
                _ => {
                    playlist::apply(Arc::make_mut(&mut l.project), e);
                    self.dirty = true;
                    self.stale = true;
                }
            }
        }
    }

    /// Carry out the mixer edits of one frame.
    pub fn edit_mixer(&mut self, edits: &[MixEdit]) {
        let Some(l) = &mut self.loaded else { return };
        for e in edits {
            if *e == MixEdit::Begin {
                self.undo.push(l.project.clone());
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.redo.clear();
            } else {
                mixer::apply(Arc::make_mut(&mut l.project), e);
                self.dirty = true;
                self.stale = true;
            }
        }
    }

    pub fn undo(&mut self) {
        if let (Some(l), Some(prev)) = (&mut self.loaded, self.undo.pop()) {
            self.redo.push(std::mem::replace(&mut l.project, prev));
            self.dirty = true;
            self.stale = true;
        }
    }

    pub fn redo(&mut self) {
        if let (Some(l), Some(next)) = (&mut self.loaded, self.redo.pop()) {
            self.undo.push(std::mem::replace(&mut l.project, next));
            self.dirty = true;
            self.stale = true;
        }
    }

    /// Write the project back to the file it came from.
    pub fn save(&mut self) -> Result<(), String> {
        let Some(l) = &self.loaded else {
            return Ok(());
        };
        let bytes = encode(&l.project, &l.samples, true).map_err(|e| e.to_string())?;
        let tmp = format!("{}.tmp", l.path);
        std::fs::write(&tmp, bytes).map_err(|e| format!("{tmp}: {e}"))?;
        std::fs::rename(&tmp, &l.path).map_err(|e| format!("{}: {e}", l.path))?;
        self.dirty = false;
        Ok(())
    }

    /// Start building a player for the open project.
    fn start(&mut self, mode: PlayMode, from: Option<f64>, sample_rate: f64) {
        let Some(l) = &self.loaded else { return };
        let (project, samples) = (l.project.clone(), l.samples.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Player::new(project, &samples, sample_rate, mode));
        });
        self.building = Some(Building { done: rx, from });
        self.error = None;
    }

    pub fn is_building(&self) -> bool {
        self.building.is_some()
    }
}

fn snap_name(ticks: f64) -> &'static str {
    match ticks as u32 {
        96 => "Beat",
        48 => "1/2 beat",
        24 => "1/4 beat",
        12 => "1/8 beat",
        _ => "Off",
    }
}

/// Bars, beats and ticks (all counted from 1 for bars and beats) of a tick position.
pub fn bar_beat(tick: f64, sig: (u32, u32)) -> String {
    let beat_ticks = PPQ_F * 4.0 / f64::from(sig.1.max(1));
    let bar_ticks = beat_ticks * f64::from(sig.0.max(1));
    let bar = (tick / bar_ticks).floor();
    let rest = tick - bar * bar_ticks;
    let beat = (rest / beat_ticks).floor();
    let sub = rest - beat * beat_ticks;
    format!("{}:{}:{:03}", bar as i64 + 1, beat as i64 + 1, sub as i64)
}

impl App {
    /// Open a project and tell the player side about it.
    pub(crate) fn open_project(&mut self, path: &str) {
        self.stop_project();
        self.project.path = path.to_string();
        match self.project.open(path) {
            Ok(()) => {
                self.project.error = None;
                self.view = crate::app::View::Project;
                // ready for keys played live
                if let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) {
                    self.project.start(PlayMode::Song, None, rate);
                }
            }
            Err(e) => self.project.error = Some(e),
        }
    }

    pub(crate) fn save_project(&mut self) {
        match self.project.save() {
            Ok(()) => self.project.error = None,
            Err(e) => self.project.error = Some(e),
        }
    }

    fn playlist(&mut self, ui: &mut Ui, playhead: Option<f64>) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let edits = playlist::draw(
            ui,
            &project,
            self.project.pattern,
            self.project.start_tick,
            &mut self.project.playlist,
            playhead,
        );
        drop(project);
        self.project.edit_playlist(&edits);
    }

    fn roll(&mut self, ui: &mut Ui, playhead: Option<f64>) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        // the channels to pick from sit above the roll
        ui.horizontal_wrapped(|ui| {
            for (i, c) in project.channels.iter().enumerate() {
                ui.selectable_value(&mut self.project.channel, i, &c.name);
            }
        });
        let edits = roll::draw(
            ui,
            &project,
            self.project.pattern,
            self.project.channel,
            &mut self.project.roll,
            playhead,
        );
        drop(project);
        for e in &edits {
            if let Edit::Audition { key, on } = e {
                self.note((*key).clamp(0, 127) as u8, 0.8, *on);
            }
        }
        self.project.edit(&edits);
    }

    fn play_project(&mut self, mode: PlayMode, from: f64) {
        let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) else {
            return;
        };
        self.stop_project();
        self.project.looping = matches!(mode, PlayMode::Pattern(_));
        self.project.start(mode, Some(from), rate);
    }

    pub(crate) fn stop_project(&mut self) {
        if self.project.has_player {
            self.send(Command::PlayerStop);
        }
    }

    /// Hand a finished player to the audio thread. Called every frame.
    pub(crate) fn poll_project(&mut self, ctx: &egui::Context) {
        let quiet = !ctx.input(|i| i.pointer.any_down())
            && !self.audio.as_ref().is_some_and(|a| a.meters.playing());
        if self.project.stale && quiet && self.project.building.is_none() {
            self.project.stale = false;
            if let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) {
                self.project.start(PlayMode::Song, None, rate);
            }
        }
        let Some(b) = &self.project.building else {
            return;
        };
        let got = match b.done.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the player could not be built".into()),
        };
        let from = b.from;
        self.project.building = None;
        match got {
            Ok(p) => {
                if let Some(a) = &mut self.audio {
                    a.set_player(p);
                    if let Some(from) = from {
                        a.send(Command::PlayerPlay(from));
                    }
                    self.project.has_player = true;
                }
            }
            Err(e) => self.project.error = Some(e),
        }
    }

    pub(crate) fn project_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Project");
            ui.add(
                egui::TextEdit::singleline(&mut self.project.path)
                    .hint_text("path to a .las file, or drop one on the window")
                    .desired_width(360.0),
            );
            if ui.button("Open").clicked() {
                let p = self.project.path.trim().to_string();
                self.open_project(&p);
            }
            if let Some(l) = &self.project.loaded {
                ui.separator();
                ui.label(
                    RichText::new(&l.project.name)
                        .size(16.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.label(
                    RichText::new(format!(
                        "{} BPM, {}/{}, {} channels, {} patterns, {} samples",
                        l.project.bpm,
                        l.project.time_sig.0,
                        l.project.time_sig.1,
                        l.project.channels.len(),
                        l.project.patterns.len(),
                        l.samples.len()
                    ))
                    .weak(),
                );
            }
        });
        if let Some(e) = &self.project.error {
            ui.colored_label(Color32::from_rgb(235, 70, 60), e);
        }
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let warnings = l.warnings.clone();
        for w in &warnings {
            ui.label(RichText::new(w).color(Color32::from_rgb(230, 180, 60)));
        }
        let (playing, pos) = self
            .audio
            .as_ref()
            .map(|a| (a.meters.playing(), a.meters.position()))
            .unwrap_or((false, 0.0));
        let busy = self.project.is_building();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Play song"))
                .on_hover_text("From the start marker on the playlist ruler")
                .clicked()
            {
                let from = self.project.start_tick;
                self.play_project(PlayMode::Song, from);
            }
            if ui
                .add_enabled(
                    !busy && !project.patterns.is_empty(),
                    egui::Button::new("Play pattern"),
                )
                .clicked()
            {
                let i = self.project.pattern;
                self.play_project(PlayMode::Pattern(i), 0.0);
            }
            if ui.button("Stop").clicked() {
                self.stop_project();
            }
            if busy {
                ui.spinner();
                ui.label("Loading the sounds...");
            } else {
                let shown = if playing { pos } else { 0.0 };
                ui.label(
                    RichText::new(bar_beat(shown, project.time_sig))
                        .monospace()
                        .size(18.0),
                );
            }
            ui.separator();
            ui.label("Pattern");
            for (i, p) in project.patterns.iter().enumerate() {
                let name = if p.name.is_empty() {
                    format!("{}", i + 1)
                } else {
                    p.name.clone()
                };
                ui.selectable_value(&mut self.project.pattern, i, name);
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.project.page, Page::Rack, "Channel rack");
            ui.selectable_value(&mut self.project.page, Page::Roll, "Piano roll");
            ui.selectable_value(&mut self.project.page, Page::Playlist, "Playlist");
            ui.selectable_value(&mut self.project.page, Page::Mixer, "Mixer");
            ui.separator();
            if self.project.page == Page::Roll {
                ui.label("Snap");
                let snap = &mut self.project.roll.snap;
                egui::ComboBox::from_id_salt("snap")
                    .selected_text(snap_name(*snap))
                    .show_ui(ui, |ui| {
                        for v in [96.0, 48.0, 24.0, 12.0, 1.0] {
                            ui.selectable_value(snap, v, snap_name(v));
                        }
                    });
                ui.label("Zoom");
                ui.add(egui::Slider::new(&mut self.project.roll.zoom, 0.1..=2.0).show_value(false));
                ui.separator();
            }
            if ui
                .add_enabled(self.project.dirty, egui::Button::new("Save"))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save_project();
            }
            if self.project.dirty {
                ui.label(RichText::new("unsaved changes").weak());
            }
            let hint = match self.project.page {
                Page::Rack => "Pick a channel to play it with the keys below.",
                Page::Roll => "Click or drag to place notes, drag to move, right click deletes.",
                Page::Playlist => {
                    "Double click places the chosen pattern; drag moves, drag an edge resizes, right click deletes, click the ruler to set the start."
                }
                Page::Mixer => "Faders, pan, mute and solo; click an effect to switch it on or off.",
            };
            ui.label(RichText::new(hint).weak().size(11.0));
        });
        ui.separator();
        let head = (playing && self.project.looping).then_some(pos);
        match self.project.page {
            Page::Rack => self.rack(ui, head),
            Page::Roll => self.roll(ui, head),
            Page::Playlist => {
                let song_head = (playing && !self.project.looping).then_some(pos);
                self.playlist(ui, song_head);
            }
            Page::Mixer => {
                let edits = mixer::draw(ui, &project);
                drop(project);
                self.project.edit_mixer(&edits);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (ProjectState, String) {
        let src = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../las-engine/tests/fixtures/engines.las"
        );
        let dir = std::env::temp_dir().join(format!("las-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!(
            "{}.las",
            las_project::create::uid("t").replace(|c: char| !c.is_alphanumeric(), "")
        );
        let path = dir.join(name);
        std::fs::copy(src, &path).unwrap();
        let mut s = ProjectState::default();
        s.open(path.to_str().unwrap()).unwrap();
        (s, path.to_string_lossy().into_owned())
    }

    fn count(s: &ProjectState) -> usize {
        let p = &s.loaded.as_ref().unwrap().project;
        p.patterns[s.pattern]
            .notes
            .get(&p.channels[s.channel].id)
            .map_or(0, Vec::len)
    }

    #[test]
    fn edits_undo_redo_and_save() {
        let (mut s, path) = fixture();
        let before = count(&s);
        assert!(!s.dirty);
        s.edit(&[Edit::Begin, Edit::Add(roll::new_note(60, 0.0, 24.0))]);
        assert_eq!(count(&s), before + 1);
        assert!(s.dirty && s.stale);
        s.undo();
        assert_eq!(count(&s), before);
        s.redo();
        assert_eq!(count(&s), before + 1);
        // a new gesture drops what could be redone
        s.undo();
        s.edit(&[Edit::Begin, Edit::Add(roll::new_note(61, 0.0, 24.0))]);
        s.redo();
        assert_eq!(count(&s), before + 1);

        s.save().unwrap();
        assert!(!s.dirty);
        let mut again = ProjectState::default();
        again.open(&path).unwrap();
        assert_eq!(count(&again), before + 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn undo_with_nothing_to_undo_does_nothing() {
        let (mut s, path) = fixture();
        s.undo();
        s.redo();
        assert!(!s.dirty);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn positions_read_as_bars_and_beats() {
        assert_eq!(bar_beat(0.0, (4, 4)), "1:1:000");
        assert_eq!(bar_beat(96.0 * 5.0 + 7.0, (4, 4)), "2:2:007");
        assert_eq!(bar_beat(96.0 * 3.0, (3, 4)), "2:1:000");
    }
}
