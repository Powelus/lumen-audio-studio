// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening and playing a project. A project is decoded here, a player for it is
//! built on a worker thread (building is the slow part), and the audio thread then
//! takes the player over.

use crate::app::App;
use crate::audio::Command;
use crate::auto::{self, AutoEdit, AutoRequest, AutoState};
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

/// What a rename dialog renames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Channel(usize),
    Pattern(usize),
    Track(usize),
    Marker(String),
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
    /// The automation clip whose editor is open, and its state.
    pub open_auto: Option<String>,
    pub auto_state: AutoState,
    /// Where "Play song" starts, in ticks.
    pub start_tick: f64,
    /// Earlier and later states of the project, for undo and redo.
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    /// Changed since it was opened or saved.
    pub dirty: bool,
    /// A name being asked for, and what it is for.
    pub rename: Option<(RenameTarget, String)>,
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
        self.open_auto = None;
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
                PlEdit::OpenAuto(id) => self.open_auto = Some(id.clone()),
                PlEdit::AskRenameTrack(t) => {
                    let name = l
                        .project
                        .playlist
                        .tracks
                        .get(*t)
                        .map_or_else(|| format!("Track {}", t + 1), |x| x.name.clone());
                    self.rename = Some((RenameTarget::Track(*t), name));
                }
                PlEdit::AskRenameMarker(id) => {
                    let name = l
                        .project
                        .markers
                        .iter()
                        .flatten()
                        .find(|m| &m.id == id)
                        .map_or_else(String::new, |m| m.name.clone());
                    self.rename = Some((RenameTarget::Marker(id.clone()), name));
                }
                _ => {
                    playlist::apply(Arc::make_mut(&mut l.project), e);
                    self.dirty = true;
                    self.stale = true;
                }
            }
        }
    }

    /// Make an automation clip for a request, at the start marker, and open its editor.
    pub fn create_automation(&mut self, req: &AutoRequest) {
        let at = self.start_tick;
        let mut id = String::new();
        self.modify(true, |p| id = auto::create_clip(p, req, at));
        self.open_auto = Some(id);
    }

    /// Carry out the edits of an automation clip's curve.
    pub fn edit_auto(&mut self, edits: &[AutoEdit]) {
        let Some(l) = &mut self.loaded else { return };
        for e in edits {
            if *e == AutoEdit::Begin {
                self.undo.push(l.project.clone());
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.redo.clear();
            } else {
                auto::apply(Arc::make_mut(&mut l.project), e);
                self.dirty = true;
                self.stale = true;
            }
        }
    }

    /// Carry out the mixer edits of one frame.
    pub fn edit_mixer(&mut self, edits: &[MixEdit]) {
        let mut pending: Vec<AutoRequest> = Vec::new();
        let Some(l) = &mut self.loaded else { return };
        for e in edits {
            if let MixEdit::Automate(req) = e {
                pending.push(req.clone());
            } else if let MixEdit::OpenEffect { insert, slot } = e {
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
        for r in pending {
            self.create_automation(&r);
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
        let mut pending: Vec<AutoRequest> = Vec::new();
        let Some(l) = &mut self.loaded else { return };
        for e in edits {
            if let ChanEdit::Automate(req) = e {
                pending.push(req.clone());
            } else if let ChanEdit::AskRename(i) = e {
                let name = l.project.channels.get(*i).map(|c| c.name.clone());
                if let Some(name) = name {
                    self.rename = Some((RenameTarget::Channel(*i), name));
                }
            } else if *e == ChanEdit::Begin {
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
        for r in pending {
            self.create_automation(&r);
        }
    }

    /// Change the project and its samples together (undo covers the project; a sample
    /// that no channel uses any more is harmless).
    pub fn modify_with_samples(
        &mut self,
        begin: bool,
        f: impl FnOnce(&mut Project, &mut BTreeMap<String, Sample>),
    ) {
        let Some(l) = &mut self.loaded else { return };
        if begin {
            self.undo.push(l.project.clone());
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        f(Arc::make_mut(&mut l.project), Arc::make_mut(&mut l.samples));
        self.dirty = true;
        self.stale = true;
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

    /// The steps undo and redo can reach, each described by what it changed: the
    /// earlier steps (oldest first) and the later ones (next first).
    pub fn history(&self) -> (Vec<String>, Vec<String>) {
        let Some(l) = &self.loaded else {
            return (Vec::new(), Vec::new());
        };
        let mut back = Vec::new();
        for (i, before) in self.undo.iter().enumerate() {
            let after = self.undo.get(i + 1).unwrap_or(&l.project);
            back.push(describe_change(before, after));
        }
        let mut forward = Vec::new();
        let mut cur = &l.project;
        for next in self.redo.iter().rev() {
            forward.push(describe_change(cur, next));
            cur = next;
        }
        (back, forward)
    }

    /// Undo or redo until `steps` from now (negative goes back).
    pub fn travel(&mut self, steps: i32) {
        for _ in 0..steps.unsigned_abs() {
            if steps < 0 {
                self.undo();
            } else {
                self.redo();
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
        // the outlines of the samples audio clips play, made once
        let missing: Vec<String> = project
            .channels
            .iter()
            .filter(|c| c.plugin_id == "audioclip")
            .filter_map(|c| c.sample_id.clone())
            .filter(|id| !self.project.playlist.waves.contains_key(id))
            .collect();
        if !missing.is_empty() {
            let mut lib = las_engine::library::Library::new(22_050.0, &l.samples);
            for id in missing {
                let wave = lib.get(&id).map_or_else(
                    || playlist::Wave {
                        peaks: Vec::new(),
                        step: 1.0,
                    },
                    |a| playlist::Wave::of(&a.channels, a.sample_rate),
                );
                self.project.playlist.waves.insert(id, Arc::new(wave));
            }
        }
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
                RichText::new("Double click places the pattern, drag moves, right click deletes, click the ruler to set the start. Drop a WAV file here for an audio clip.")
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
            ui.menu_button("Tools", |ui| {
                let ids: Vec<String> = self.project.roll.selected.iter().cloned().collect();
                ui.label(if ids.is_empty() {
                    "On all notes"
                } else {
                    "On the selected notes"
                });
                let mut run = None;
                for (name, op, tip) in roll::NOTE_TOOLS {
                    if ui.button(name).on_hover_text(tip).clicked() {
                        run = Some(match op {
                            roll::NoteTool::Humanize(_) => roll::NoteTool::Humanize(
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(1, |d| d.as_nanos() as u64),
                            ),
                            o => o,
                        });
                    }
                }
                if let Some((root, steps)) = self.project.roll.scale {
                    if ui
                        .button("Snap to scale")
                        .on_hover_text(
                            "Move keys outside the highlighted scale to the nearest one in it",
                        )
                        .clicked()
                    {
                        run = Some(roll::NoteTool::ToScale(root, steps));
                    }
                }
                if let Some(op) = run {
                    let ch = self.project.channel;
                    self.project
                        .edit_channel(ch, &[Edit::Begin, Edit::Tool { op, ids }]);
                    ui.close();
                }
            });
            ui.checkbox(&mut self.project.roll.ghost, "Ghost notes");
            let roll = &mut self.project.roll;
            let scale_name = roll.scale.map_or("No scale".to_string(), |(root, steps)| {
                let name = roll::SCALES
                    .iter()
                    .find(|s| s.1 == steps)
                    .map_or("", |s| s.0);
                format!(
                    "{} {name}",
                    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
                        [root as usize % 12]
                )
            });
            ui.menu_button(scale_name, |ui| {
                if ui.button("No scale").clicked() {
                    roll.scale = None;
                    ui.close();
                }
                for (root, rn) in [
                    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
                ]
                .iter()
                .enumerate()
                {
                    ui.menu_button(*rn, |ui| {
                        for (name, steps) in roll::SCALES {
                            if ui.button(name).clicked() {
                                roll.scale = Some((root as i32, steps));
                                ui.close();
                            }
                        }
                    });
                }
            })
            .response
            .on_hover_text("Highlight a scale: keys outside it are dimmed");
            let stamp_name = roll.stamp.map_or("Stamp: off".to_string(), |iv| {
                format!(
                    "Stamp: {}",
                    roll::STAMPS.iter().find(|s| s.1 == iv).map_or("", |s| s.0)
                )
            });
            ui.menu_button(stamp_name, |ui| {
                if ui.button("Off (one note)").clicked() {
                    roll.stamp = None;
                    ui.close();
                }
                for (name, iv) in roll::STAMPS {
                    if ui.button(name).clicked() {
                        roll.stamp = Some(iv);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Each click draws this chord on the key clicked");
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
        if self.shell.record {
            if self.shell.recorder.is_none() {
                match crate::record::Recorder::open() {
                    Ok(r) => self.shell.recorder = Some(r),
                    Err(e) => self.project.error = Some(format!("Recording: {e}")),
                }
            }
            if let Some(r) = &mut self.shell.recorder {
                r.begin();
                self.shell.record_from = if self.project.looping { 0.0 } else { from };
                self.shell.status = "Recording...".into();
            }
        }
    }

    pub(crate) fn stop_project(&mut self) {
        if self.project.has_player {
            self.send(Command::PlayerStop);
        }
        let take = self
            .shell
            .recorder
            .as_mut()
            .filter(|r| r.is_capturing())
            .and_then(|r| r.end());
        if let Some(wav) = take {
            let at = self.shell.record_from;
            let n = self.project.loaded.as_ref().map_or(0, |l| {
                l.project
                    .channels
                    .iter()
                    .filter(|c| c.plugin_id == "audioclip")
                    .count()
            });
            let mut result = Ok(0);
            self.project.modify_with_samples(true, |p, samples| {
                let track = crate::record::free_track(p, at);
                result = crate::menus::import_audio_clip(
                    p,
                    samples,
                    &format!("Take {}", n + 1),
                    wav,
                    at,
                    track,
                );
            });
            match result {
                Ok(_) => self.shell.status = "The take is in the playlist.".into(),
                Err(e) => self.project.error = Some(format!("Recording: {e}")),
            }
        }
    }

    /// Hand a finished player to the audio thread. Called every frame.
    pub(crate) fn poll_project(&mut self, ctx: &egui::Context) {
        // an edit takes effect once the pointer is up: a player is built for the edited
        // project, and if the song is playing it carries on from where it is
        if let Some(r) = &mut self.shell.recorder {
            r.drain();
        }
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
                    // a new player does not know about the metronome switch
                    let metro = self.shell.metronome;
                    a.send(Command::Metronome(metro));
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

/// What changed between two states of a project, in a few words.
pub fn describe_change(a: &Project, b: &Project) -> String {
    let notes = |p: &Project| {
        p.patterns
            .iter()
            .map(|x| x.notes.values().map(Vec::len).sum::<usize>())
            .sum::<usize>()
    };
    let count = |n: usize, m: usize, what: &str| -> Option<String> {
        match n.cmp(&m) {
            std::cmp::Ordering::Less => Some(format!("Add {what}")),
            std::cmp::Ordering::Greater => Some(format!("Delete {what}")),
            std::cmp::Ordering::Equal => None,
        }
    };
    if let Some(s) = count(a.channels.len(), b.channels.len(), "channel") {
        return s;
    }
    if let Some(s) = count(a.patterns.len(), b.patterns.len(), "pattern") {
        return s;
    }
    if let Some(s) = count(notes(a), notes(b), "notes") {
        return s;
    }
    if let Some(s) = count(a.playlist.clips.len(), b.playlist.clips.len(), "clip") {
        return s;
    }
    if let Some(s) = count(a.automation.len(), b.automation.len(), "automation") {
        return s;
    }
    if a.bpm != b.bpm {
        return format!("Tempo {:.1}", b.bpm);
    }
    macro_rules! json {
        ($v:expr) => {
            serde_json::to_string($v).unwrap_or_default()
        };
    }
    if json!(&a.mixer) != json!(&b.mixer) {
        return "Mixer".into();
    }
    if json!(&a.channels) != json!(&b.channels) {
        // the first channel that changed
        let name = a
            .channels
            .iter()
            .zip(&b.channels)
            .find(|(x, y)| json!(x) != json!(y))
            .map_or("Channel".to_string(), |(_, y)| y.name.clone());
        return format!("Edit {name}");
    }
    if json!(&a.patterns) != json!(&b.patterns) {
        return "Move notes".into();
    }
    if json!(&a.playlist) != json!(&b.playlist) {
        return "Arrange".into();
    }
    if json!(&a.automation) != json!(&b.automation) {
        return "Automation".into();
    }
    "Edit".into()
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

    #[test]
    fn history_names_each_step_and_travels_to_any_of_them() {
        let (mut s, _) = fixture();
        let channels = |s: &ProjectState| s.loaded.as_ref().unwrap().project.channels.len();
        let n0 = channels(&s);
        s.modify(true, |p| {
            las_project::create::add_channel(p, "polaris");
        });
        s.modify(true, |p| p.bpm = 99.0);
        s.modify(true, |p| p.channels[0].name = "Renamed".into());
        let (back, fwd) = s.history();
        assert_eq!(back, vec!["Add channel", "Tempo 99.0", "Edit Renamed"]);
        assert!(fwd.is_empty());
        // back to the start, then forward two steps
        s.travel(-3);
        assert_eq!(channels(&s), n0);
        assert_eq!(s.history().1.len(), 3);
        s.travel(2);
        assert_eq!(channels(&s), n0 + 1);
        assert_eq!(s.loaded.as_ref().unwrap().project.bpm, 99.0);
        assert_eq!(
            s.history(),
            (
                vec!["Add channel".to_string(), "Tempo 99.0".to_string()],
                vec!["Edit Renamed".to_string()]
            )
        );
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
