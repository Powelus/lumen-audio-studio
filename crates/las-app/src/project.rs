// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening and playing a project. A project is decoded here, a player for it is
//! built on a worker thread (building is the slow part), and the audio thread then
//! takes the player over.

use crate::app::App;
use crate::audio::Command;
use crate::mixer::{self, MixEdit};
use crate::playlist::{self, PlEdit, PlaylistState};
use crate::roll::{self, Edit, RollState};
use crate::steps::{self, ChanEdit};
use eframe::egui::{self, RichText, Ui};
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
    from: Start,
}

/// What a freshly built player does.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Start {
    /// Nothing: it only waits for keys played live.
    Ready,
    /// Play from this tick.
    At(f64),
    /// Carry on from wherever the song has got to, if it is still playing (an edit
    /// made while playing).
    Resume,
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
    pub roll: RollState,
    pub playlist: PlaylistState,
    pub mixer_state: mixer::MixerState,
    /// Where "Play song" starts, in ticks.
    pub start_tick: f64,
    /// Earlier and later states of the project, for undo and redo.
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    /// Changed since it was opened or saved.
    pub dirty: bool,
    /// The mixer effect whose window is open: (insert, slot).
    pub open_effect: Option<(usize, usize)>,
    /// The kind and time of the last coalesced edit.
    last_edit: Option<(String, std::time::Instant)>,
    /// The player no longer matches the project and is rebuilt when things are quiet.
    stale: bool,
}

impl ProjectState {
    pub fn open(&mut self, path: &str) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        self.open_bytes(&bytes, path)
            .map_err(|e| format!("{path}: {e}"))
    }

    /// Open a project from its bytes; `path` is where it will be saved ("" for none).
    pub fn open_bytes(&mut self, bytes: &[u8], path: &str) -> Result<(), String> {
        let b = decode(bytes).map_err(|e| e.to_string())?;
        self.install(b.project, b.samples, path, b.warnings);
        Ok(())
    }

    /// Make a project the open one.
    pub fn install(
        &mut self,
        project: Project,
        samples: BTreeMap<String, Sample>,
        path: &str,
        warnings: Vec<String>,
    ) {
        self.loaded = Some(Loaded {
            path: path.to_string(),
            project: Arc::new(project),
            samples: Arc::new(samples),
            warnings,
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
        self.last_edit = None;
        self.open_effect = None;
    }

    /// Whether the project has a file to be saved to.
    pub fn has_file(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| !l.path.is_empty())
    }

    /// Carry out the edits of one frame on the open project.
    pub fn edit(&mut self, edits: &[Edit]) {
        self.edit_channel(self.channel, edits);
    }

    /// Carry out edits on a channel's notes in the chosen pattern.
    pub fn edit_channel(&mut self, channel: usize, edits: &[Edit]) {
        let pattern = self.pattern;
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
            if let MixEdit::OpenEffect { insert, slot } = e {
                self.open_effect = Some((*insert, *slot));
            } else if *e == MixEdit::Begin {
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

    /// Change the project in some other way (`begin` starts an undo step).
    pub fn modify(&mut self, begin: bool, f: impl FnOnce(&mut Project)) {
        let Some(l) = &mut self.loaded else { return };
        if begin {
            self.undo.push(l.project.clone());
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        f(Arc::make_mut(&mut l.project));
        self.dirty = true;
        self.stale = true;
    }

    /// Carry out channel setting edits (level, pan, mute, swing).
    pub fn edit_chans(&mut self, edits: &[ChanEdit]) {
        let Some(l) = &mut self.loaded else { return };
        for e in edits {
            if *e == ChanEdit::Begin {
                self.undo.push(l.project.clone());
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.redo.clear();
            } else {
                steps::apply(Arc::make_mut(&mut l.project), e);
                self.dirty = true;
                self.stale = true;
            }
        }
    }

    /// Change the project, starting an undo step only when this is a different kind of
    /// change from the last one or more than a moment after it: a knob drag is one step.
    pub fn modify_coalesced(&mut self, key: &str, f: impl FnOnce(&mut Project)) {
        let now = std::time::Instant::now();
        let fresh = match &self.last_edit {
            Some((k, t)) => {
                k != key || now.duration_since(*t) > std::time::Duration::from_millis(1200)
            }
            None => true,
        };
        self.last_edit = Some((key.to_string(), now));
        self.modify(fresh, f);
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
        let Some(path) = self.loaded.as_ref().map(|l| l.path.clone()) else {
            return Ok(());
        };
        self.save_to(&path)
    }

    /// Write the project to a file, which is then the one it belongs to.
    pub fn save_to(&mut self, path: &str) -> Result<(), String> {
        let Some(l) = &mut self.loaded else {
            return Ok(());
        };
        if path.is_empty() {
            return Err("no file to save to".into());
        }
        let bytes = encode(&l.project, &l.samples, true).map_err(|e| e.to_string())?;
        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| format!("{tmp}: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{path}: {e}"))?;
        l.path = path.to_string();
        self.path = path.to_string();
        self.dirty = false;
        Ok(())
    }

    /// Start building a player for the open project.
    fn start(&mut self, mode: PlayMode, from: Start, sample_rate: f64) {
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

fn playlist_snap_name(ticks: f64) -> &'static str {
    match ticks as u32 {
        384 => "Bar",
        96 => "Beat",
        24 => "1/4 beat",
        _ => "Off",
    }
}

pub(crate) fn snap_name(ticks: f64) -> &'static str {
    match ticks as u32 {
        96 => "Beat",
        48 => "1/2 beat",
        24 => "1/4 beat",
        12 => "1/8 beat",
        _ => "Off",
    }
}

impl App {
    /// Open a project and tell the player side about it.
    pub(crate) fn open_project(&mut self, path: &str) {
        self.stop_project();
        self.project.path = path.to_string();
        match self.project.open(path) {
            Ok(()) => {
                self.project.error = None;
                // ready for keys played live
                if let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) {
                    self.project.start(PlayMode::Song, Start::Ready, rate);
                }
            }
            Err(e) => self.project.error = Some(e),
        }
    }

    /// Save: to the file the project has, or ask for one.
    pub(crate) fn save_project(&mut self) {
        if !self.project.has_file() {
            self.save_as_dialog();
            return;
        }
        match self.project.save() {
            Ok(()) => self.project.error = None,
            Err(e) => self.project.error = Some(e),
        }
    }

    /// Make the project that was just installed ready to play and to take live keys.
    pub(crate) fn ready_project(&mut self) {
        self.project.error = None;
        if let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) {
            self.project.start(PlayMode::Song, Start::Ready, rate);
        }
    }

    pub(crate) fn playlist(&mut self, ui: &mut Ui, playhead: Option<f64>) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        ui.horizontal(|ui| {
            for (t, name) in [
                (playlist::Tool::Draw, "Draw"),
                (playlist::Tool::Select, "Select"),
                (playlist::Tool::Slice, "Slice"),
                (playlist::Tool::Mute, "Mute"),
                (playlist::Tool::Delete, "Delete"),
            ] {
                ui.selectable_value(&mut self.project.playlist.tool, t, name);
            }
            ui.separator();
            ui.label("Snap");
            let snap = &mut self.project.playlist.snap;
            egui::ComboBox::from_id_salt("playlist-snap")
                .selected_text(playlist_snap_name(*snap))
                .show_ui(ui, |ui| {
                    for v in [384.0, 96.0, 24.0, 1.0] {
                        ui.selectable_value(snap, v, playlist_snap_name(v));
                    }
                });
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.project.playlist.zoom, 0.03..=0.6).show_value(false));
            ui.label(
                RichText::new("Double click places the pattern, drag moves, right click deletes, click the ruler to set the start.")
                    .weak()
                    .size(11.0),
            );
        });
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

    pub(crate) fn roll(&mut self, ui: &mut Ui, playhead: Option<f64>) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        // the channels to pick from, and the snap and zoom, sit above the roll
        ui.horizontal_wrapped(|ui| {
            for (i, c) in project.channels.iter().enumerate() {
                ui.selectable_value(&mut self.project.channel, i, &c.name);
            }
            ui.separator();
            for (t, name) in [
                (roll::Tool::Draw, "Draw"),
                (roll::Tool::Select, "Select"),
                (roll::Tool::Delete, "Delete"),
            ] {
                ui.selectable_value(&mut self.project.roll.tool, t, name);
            }
            ui.separator();
            ui.label("Snap");
            let snap = &mut self.project.roll.snap;
            egui::ComboBox::from_id_salt("roll-snap")
                .selected_text(snap_name(*snap))
                .show_ui(ui, |ui| {
                    for v in [96.0, 48.0, 24.0, 12.0, 1.0] {
                        ui.selectable_value(snap, v, snap_name(v));
                    }
                });
            if ui.button("Quantize").clicked() {
                let snap = self.project.roll.snap;
                let pattern = self.project.pattern;
                let ch = self.project.channel;
                self.project
                    .edit_channel(ch, &[Edit::Begin, Edit::Quantize { snap }]);
                let _ = pattern;
            }
            ui.checkbox(&mut self.project.roll.ghost, "Ghost notes");
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.project.roll.zoom, 0.1..=2.0).show_value(false));
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

    pub(crate) fn play_project(&mut self, mode: PlayMode, from: f64) {
        let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) else {
            return;
        };
        self.stop_project();
        self.project.looping = matches!(mode, PlayMode::Pattern(_));
        self.project.start(mode, Start::At(from), rate);
    }

    pub(crate) fn stop_project(&mut self) {
        if self.project.has_player {
            self.send(Command::PlayerStop);
        }
    }

    /// Hand a finished player to the audio thread. Called every frame.
    pub(crate) fn poll_project(&mut self, ctx: &egui::Context) {
        // an edit takes effect once the pointer is up: a player is built for the edited
        // project, and if the song is playing it carries on from where it is
        let quiet = !ctx.input(|i| i.pointer.any_down());
        if self.project.stale && quiet && self.project.building.is_none() {
            self.project.stale = false;
            if let Some(rate) = self.audio.as_ref().map(|a| a.sample_rate as f64) {
                let playing = self.audio.as_ref().is_some_and(|a| a.meters.playing());
                let (mode, from) = match (playing, self.project.looping) {
                    (true, true) => (PlayMode::Pattern(self.project.pattern), Start::Resume),
                    (true, false) => (PlayMode::Song, Start::Resume),
                    (false, _) => (PlayMode::Song, Start::Ready),
                };
                self.project.start(mode, from, rate);
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
                    match from {
                        Start::Ready => {}
                        Start::At(t) => a.send(Command::PlayerPlay(t)),
                        Start::Resume => {
                            if a.meters.playing() {
                                let t = a.meters.position();
                                a.send(Command::PlayerPlay(t));
                            }
                        }
                    }
                    self.project.has_player = true;
                }
            }
            Err(e) => self.project.error = Some(e),
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
}
