// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening and playing a project. A project is decoded here, a player for it is
//! built on a worker thread (building is the slow part), and the audio thread then
//! takes the player over.

use crate::app::App;
use crate::audio::Command;
use crate::widgets::ACCENT;
use eframe::egui::{self, Color32, RichText, Ui};
use las_engine::timeline::PPQ_F;
use las_engine::{PlayMode, Player};
use las_project::{decode, Project, Sample};
use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// An open project.
pub struct Loaded {
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
}

impl ProjectState {
    pub fn open(&mut self, path: &str) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let b = decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
        self.loaded = Some(Loaded {
            project: Arc::new(b.project),
            samples: Arc::new(b.samples),
            warnings: b.warnings,
        });
        self.pattern = 0;
        self.channel = 0;
        self.has_player = false;
        self.building = None;
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
    pub(crate) fn poll_project(&mut self) {
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
            ui.label("Project file");
            ui.add(egui::TextEdit::singleline(&mut self.project.path).desired_width(420.0));
            if ui.button("Open").clicked() {
                let p = self.project.path.trim().to_string();
                self.open_project(&p);
            }
        });
        ui.label(RichText::new("Or drop a .las file on the window.").weak());
        if let Some(e) = &self.project.error {
            ui.colored_label(Color32::from_rgb(235, 70, 60), e);
        }
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let warnings = l.warnings.clone();
        ui.add_space(8.0);
        ui.label(
            RichText::new(&project.name)
                .size(20.0)
                .strong()
                .color(ACCENT),
        );
        ui.label(
            RichText::new(format!(
                "{} BPM, {}/{}, {} channels, {} patterns, {} samples",
                project.bpm,
                project.time_sig.0,
                project.time_sig.1,
                project.channels.len(),
                project.patterns.len(),
                l.samples.len()
            ))
            .weak(),
        );
        for w in &warnings {
            ui.label(RichText::new(w).color(Color32::from_rgb(230, 180, 60)));
        }
        ui.add_space(8.0);
        let (playing, pos) = self
            .audio
            .as_ref()
            .map(|a| (a.meters.playing(), a.meters.position()))
            .unwrap_or((false, 0.0));
        let busy = self.project.is_building();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Play song"))
                .clicked()
            {
                self.play_project(PlayMode::Song, 0.0);
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
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
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
        ui.add_space(8.0);
        ui.label(RichText::new("Pick a channel to play it with the keys below.").weak());
        let head = (playing && self.project.looping).then_some(pos);
        self.rack(ui, head);
    }
}
