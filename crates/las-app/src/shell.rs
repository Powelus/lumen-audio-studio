// SPDX-License-Identifier: GPL-3.0-or-later
//! The window around everything, laid out like the web version: a top bar with the
//! transport and its displays, a browser on the left, floating windows for the
//! editors, and a status bar.

use crate::app::App;
use crate::audio::{Command, Instrument, Preview};
use crate::mixer::{self, MixEdit};
use crate::raise::Win;
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
    /// The keyboard shortcuts window.
    pub keys: bool,
    pub history: bool,
    pub settings: bool,
    pub analyser: bool,
}

/// The system's mouse cursor in the window's points, where the system reports it
/// (Windows, where no pointer moves come during a drag from Explorer).
pub fn os_cursor_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let (x, y) = las_window::cursor_pos()?;
    let (inner, native) = ctx.input(|i| {
        (
            i.viewport().inner_rect,
            i.viewport().native_pixels_per_point,
        )
    });
    let (inner, native) = (inner?, native?);
    let ppp = ctx.pixels_per_point();
    let pos = egui::pos2(
        (x as f32 - inner.min.x * native) / ppp,
        (y as f32 - inner.min.y * native) / ppp,
    );
    ctx.content_rect().contains(pos).then_some(pos)
}

/// The window on top where something is let go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Over {
    Playlist,
    Rack,
    /// Another window: the drop does not reach the playlist or the rack under it.
    Other,
    /// No window, or not known (no pointer position).
    Unknown,
}

impl Over {
    /// The window on top at `pos`.
    pub fn at(ctx: &egui::Context, pos: Option<egui::Pos2>) -> Over {
        let Some(layer) = pos.and_then(|p| ctx.layer_id_at(p)) else {
            return Over::Unknown;
        };
        if layer.order != egui::Order::Middle {
            return Over::Unknown;
        }
        if layer.id == Win::Playlist.id() {
            Over::Playlist
        } else if layer.id == Win::Rack.id() {
            Over::Rack
        } else {
            Over::Other
        }
    }
}

/// What was open when the windows were last drawn.
#[derive(Clone, Default)]
struct Seen {
    open: Option<Windows>,
    effect: Option<(usize, usize)>,
    auto: Option<String>,
    chan: Option<usize>,
    sample: Option<String>,
    bound: Option<usize>,
}

/// Something being dragged from the browser: a file of the user's, or a sample of
/// the factory kit (its id and name).
#[derive(Clone, Debug)]
pub enum Dragged {
    File(std::path::PathBuf),
    Factory(&'static str, &'static str),
    /// A preset of the browser's Presets tab: the synth it is for, and its name.
    Preset(Instrument, String),
}

impl Dragged {
    pub fn name(&self) -> String {
        match self {
            Dragged::File(p) => p
                .file_stem()
                .map_or("Sample".into(), |s| s.to_string_lossy().into_owned()),
            Dragged::Factory(_, n) => (*n).to_string(),
            Dragged::Preset(_, n) => n.clone(),
        }
    }
}

/// Give sampler channel `ch` the sample `id`, named after it.
pub fn set_channel_sample(p: &mut las_project::Project, ch: usize, id: &str, name: &str) {
    if let Some(c) = p.channels.get_mut(ch) {
        c.sample_id = Some(id.to_string());
        c.name = name.to_string();
    }
}

/// A file name for an insert's chain: its name, or "Insert n".
fn mixer_file_name(t: &las_project::MixerTrack, i: usize) -> String {
    let n = if t.name.trim().is_empty() {
        format!("Insert {i}")
    } else {
        t.name.trim().to_string()
    };
    n.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl App {
    /// Something let go over the window: on a playlist track it becomes an audio clip
    /// there (as in FL Studio), anywhere else a new sampler channel.
    pub(crate) fn drop_item(&mut self, item: Dragged, at: Option<egui::Pos2>, over: Over) {
        let place = self.drop_place(at, over);
        self.drop_item_at(item, at, over, place);
    }

    /// Where on the playlist a drop at `at` lands: the snapped tick and the track under
    /// the pointer, when the playlist is the window there (or no window is on top of it).
    pub(crate) fn drop_place(&self, at: Option<egui::Pos2>, over: Over) -> Option<(f64, usize)> {
        if !self.shell.open.playlist || !matches!(over, Over::Playlist | Over::Unknown) {
            return None;
        }
        self.project
            .playlist
            .area
            .zip(at)
            .and_then(|(a, pos)| a.place(pos))
    }

    /// Files dropped from the system: a project opens; sounds become audio clips on
    /// the playlist at the drop (each further file on the next track down, as in FL
    /// Studio), or channels when they are not let go over the playlist.
    pub(crate) fn drop_files(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        at: Option<egui::Pos2>,
        over: Over,
    ) {
        let mut sounds = Vec::new();
        for path in paths {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("las"))
            {
                self.open_project(&path.to_string_lossy());
            } else {
                sounds.push(path);
            }
        }
        if sounds.is_empty() || self.project.loaded.is_none() {
            return;
        }
        let place = self.drop_place(at, over).or_else(|| {
            // no pointer position came with the drop: over the playlist it still lands,
            // at the start marker on the first empty track
            (self.shell.open.playlist && at.is_none() && over == Over::Unknown)
                .then_some(self.project.loaded.as_ref())
                .flatten()
                .map(|l| crate::playlist::free_track(&l.project, self.project.start_tick))
        });
        let spots = crate::playlist::drop_spots(place, sounds.len());
        for (path, spot) in sounds.into_iter().zip(spots) {
            self.drop_item_at(Dragged::File(path), at, over, spot);
        }
    }

    fn drop_item_at(
        &mut self,
        item: Dragged,
        at: Option<egui::Pos2>,
        over: Over,
        place: Option<(f64, usize)>,
    ) {
        if self.project.loaded.is_none() {
            return;
        }
        // on a channel of the rack: a preset loads into it, a sample replaces a sampler's
        let row = at
            .filter(|_| self.shell.open.rack && matches!(over, Over::Rack | Over::Unknown))
            .filter(|_| place.is_none())
            .and_then(|pos| self.shell.steps.row_at(pos));
        if let Dragged::Preset(inst, name) = &item {
            match row {
                Some(ch) => self.drop_preset(ch, *inst, name),
                None => {
                    self.shell.notice = Some(
                        "Drop a preset on a channel in the channel rack to load it there.".into(),
                    )
                }
            }
            return;
        }
        if let Dragged::File(p) = &item {
            if !las_engine::library::is_audio_file(&p.to_string_lossy()) {
                self.shell.notice = Some(format!(
                    "{} is not an audio file this app can read. WAV, MP3, FLAC, OGG, AIFF and M4A work.",
                    p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())
                ));
                return;
            }
        }
        if let Some(ch) = row {
            if self.replace_sample(ch, &item) {
                return;
            }
        }
        match (item, place) {
            (Dragged::File(p), Some((tick, track))) => self.import_clip_path(&p, tick, track),
            (Dragged::File(p), None) => self.import_path(&p),
            (Dragged::Factory(id, name), Some((tick, track))) => {
                let Some(a) = las_engine::factory::render_factory(id, 44_100.0) else {
                    return;
                };
                let bytes = las_engine::library::encode_wav(&a);
                let mut result = Ok(0);
                self.project.modify_with_samples(true, |p, samples| {
                    result = crate::menus::import_audio_clip(p, samples, name, bytes, tick, track);
                });
                match result {
                    Ok(ch) => self.project.channel = ch,
                    Err(e) => self.shell.notice = Some(e),
                }
            }
            (Dragged::Factory(id, name), None) => self.add_factory_channel(id, name),
            (Dragged::Preset(..), _) => {}
        }
    }

    /// While a sound is dragged over the playlist (from the browser or from the
    /// system), the place it would land on is lit: the snapped start on the track
    /// under the pointer, a bar long.
    pub(crate) fn drop_highlight(&self, ctx: &egui::Context) {
        let from_os = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !from_os
            && !egui::DragAndDrop::payload::<Dragged>(ctx)
                .is_some_and(|d| !matches!(*d, Dragged::Preset(..)))
        {
            return;
        }
        let Some(area) = self
            .project
            .playlist
            .area
            .filter(|_| self.shell.open.playlist)
        else {
            return;
        };
        let pos = if from_os {
            self.shell.file_hover
        } else {
            ctx.input(|i| i.pointer.hover_pos().or(i.pointer.latest_pos()))
        };
        let Some(pos) = pos else {
            return;
        };
        if !matches!(Over::at(ctx, Some(pos)), Over::Playlist | Over::Unknown) {
            return;
        }
        let Some((tick, track)) = area.place(pos) else {
            return;
        };
        let x = area.left + tick as f32 * area.zoom;
        let y = area.top + track as f32 * area.row;
        let bar = f64::from(las_project::PPQ) * 4.0;
        let w = (bar as f32 * area.zoom).max(24.0);
        let r = egui::Rect::from_min_size(egui::pos2(x, y), vec2(w, area.row)).intersect(area.rect);
        let accent = theme::pal().accent;
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("drop-highlight"),
        ));
        painter.rect_filled(r, 3.0, accent.gamma_multiply(0.25));
        painter.rect_stroke(
            r,
            3.0,
            egui::Stroke::new(1.5, accent),
            egui::StrokeKind::Inside,
        );
        ctx.request_repaint();
    }

    /// Load the preset `name` of `inst` into channel `ch`, when that is its synth.
    fn drop_preset(&mut self, ch: usize, inst: Instrument, name: &str) {
        let Some(c) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(ch))
        else {
            return;
        };
        let cname = c.name.clone();
        if crate::inst::panel_for(&c.plugin_id) != Some(inst) {
            self.shell.notice = Some(format!(
                "{name} is a preset for another instrument than {cname}'s. Drop it on a channel of the same synth."
            ));
            return;
        }
        self.open_channel(ch);
        let k = crate::app::index(inst);
        match self.entries[k].iter().position(|e| e.name == name) {
            Some(i) => {
                self.load_preset(inst, i);
                self.sync_bound();
            }
            None => {
                self.shell.notice = Some(format!("{cname}'s synth has no preset called {name}."))
            }
        }
    }

    /// Put a dropped sample in sampler channel `ch` in place of its own. Returns
    /// whether it did (false when the channel is not a sampler).
    fn replace_sample(&mut self, ch: usize, item: &Dragged) -> bool {
        let is_sampler = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(ch))
            .is_some_and(|c| c.plugin_id == "sampler");
        if !is_sampler {
            return false;
        }
        match item {
            Dragged::Factory(id, name) => {
                let (id, name) = (*id, *name);
                self.project.modify(true, |p| {
                    if !p.samples.iter().any(|s| s.id == id) {
                        p.samples.push(las_project::SampleRef {
                            id: id.to_string(),
                            name: name.to_string(),
                            factory: Some(true),
                            ..Default::default()
                        });
                    }
                    set_channel_sample(p, ch, id, name);
                });
            }
            Dragged::File(path) => {
                let name = item.name();
                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        self.shell.notice = Some(format!("{}: {e}", path.display()));
                        return true;
                    }
                };
                let (audio, bytes) = match las_engine::library::import_audio(bytes) {
                    Ok(x) => x,
                    Err(e) => {
                        self.shell.notice = Some(format!("{}: {e}", path.display()));
                        return true;
                    }
                };
                self.project.modify_with_samples(true, |p, samples| {
                    let id = las_project::create::uid("smp_");
                    p.samples.push(las_project::SampleRef {
                        id: id.clone(),
                        name: name.clone(),
                        duration: Some(audio.frames() as f64 / audio.sample_rate),
                        ..Default::default()
                    });
                    samples.insert(
                        id.clone(),
                        las_project::Sample {
                            name: name.clone(),
                            mime: "audio/wav".into(),
                            data: bytes,
                        },
                    );
                    set_channel_sample(p, ch, &id, &name);
                });
            }
            Dragged::Preset(..) => return false,
        }
        self.project.channel = ch;
        true
    }

    /// While something is dragged from the browser, its name follows the pointer.
    pub(crate) fn drag_label(&self, ctx: &egui::Context) {
        self.drop_highlight(ctx);
        let Some(item) = egui::DragAndDrop::payload::<Dragged>(ctx) else {
            return;
        };
        let Some(pos) = ctx.pointer_hover_pos() else {
            return;
        };
        egui::Area::new(egui::Id::unique("drag-label"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + vec2(14.0, 10.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("\u{266A} {}", item.name()));
                    ui.label(
                        RichText::new(
                            if matches!(*item, Dragged::Preset(..)) {
                                "Drop on a channel of the same synth in the channel rack"
                            } else {
                                "Drop on the playlist for a clip, on a sampler channel to replace its sample, anywhere else for a new channel"
                            },
                        )
                        .size(11.0)
                        .weak(),
                    );
                });
            });
    }

    /// A message the user should see, with an OK button.
    pub(crate) fn notice_window(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.shell.notice.clone() else {
            return;
        };
        let mut ok = false;
        egui::Window::new("Something went wrong")
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.label(&msg);
                ui.add_space(6.0);
                ok = ui.button("OK").clicked();
            });
        if ok {
            self.shell.notice = None;
        }
    }
}

impl Windows {
    /// Close the windows that were open in `before` and are closed in `after`.
    pub fn close_from(&mut self, before: &Windows, after: &Windows) {
        macro_rules! each {
            ($($f:ident),*) => {$(
                if before.$f && !after.$f {
                    self.$f = false;
                }
            )*};
        }
        each!(rack, playlist, roll, mixer, synth, export, about, keys, history, settings, analyser);
    }
}

/// What the export window holds.
pub struct Export {
    pub path: String,
    pub format: usize,
    pub normalize: bool,
    pub pattern_only: bool,
    /// Render only the song loop (when there is one).
    pub loop_only: bool,
    /// Each mixer track as its own file (stems), in the folder of `path`.
    pub stems: bool,
    /// The project (its file, and its name) the path was made for, while the user has
    /// not typed or chosen one: another project gets a path of its own.
    pub made_for: Option<(String, String)>,
    working: Option<Receiver<Result<String, String>>>,
    pub message: Option<Result<String, String>>,
    /// How far the render has come, when it started, and the song's tempo and beats
    /// per bar (for showing bars).
    progress: Option<std::sync::Arc<las_engine::render::Progress>>,
    started: Option<Instant>,
    tempo: (f64, f64),
}

/// The formats the export window offers: label, FLAC, bits.
/// Export formats: name, kind (the file extension) and bits (kbit/s for MP3).
pub const FORMATS: [(&str, &str, u32); 7] = [
    ("WAV 16 bit", "wav", 16),
    ("WAV 24 bit", "wav", 24),
    ("WAV 32 bit float", "wav", 32),
    ("FLAC 16 bit", "flac", 16),
    ("FLAC 24 bit", "flac", 24),
    ("MP3 320 kbit/s", "mp3", 320),
    ("MP3 192 kbit/s", "mp3", 192),
];

pub struct Shell {
    /// The size of the screen last frame (to notice it getting smaller).
    pub last_screen: Option<Rect>,
    /// The height of the keyboard under the synth panels (from the last frame), so the
    /// panels scroll in what is left and the window fits the screen.
    pub synth_footer: f32,
    pub tab: BrowserTab,
    pub browser: bool,
    pub open: Windows,
    /// Song mode plays the playlist; pattern mode loops the chosen pattern.
    pub song_mode: bool,
    pub metronome: bool,
    /// Record is armed: playing captures the input.
    pub record: bool,
    pub recorder: Option<crate::record::Recorder>,
    /// Where the take being recorded starts, ticks.
    pub record_from: f64,
    /// LumaPad: the selected pad, and whether one is held.
    pub pad: usize,
    pub pad_held: Option<u8>,
    /// Nebula: the page shown, the browser's category (empty for all) and search.
    pub nebula_page: usize,
    pub nebula_cat: String,
    pub nebula_find: String,
    /// The channel whose shared Channel settings (Arp and chords) window is open; a
    /// sampler shows them as tabs of its own window instead.
    pub chan_settings: Option<usize>,
    /// The memory the app uses, megabytes, and when it was last read.
    ram: (Option<Instant>, f64),
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
    /// The project File > Open recent is to open.
    /// What was open when the windows were last drawn: a window that has opened
    /// since comes to the front, whatever opened it.
    seen: Seen,
    /// Where files dragged from the system were last seen over the window.
    pub file_hover: Option<egui::Pos2>,
    /// The egui id of the instrument window (each face has its own).
    pub synth_id: Option<egui::Id>,
    /// A message shown in a window until the user closes it.
    pub notice: Option<String>,
    pub recent_target: Option<String>,
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
                keys: false,
                history: false,
                settings: false,
                analyser: false,
            },
            song_mode: true,
            metronome: false,
            record: false,
            recorder: None,
            record_from: 0.0,
            pad: 0,
            pad_held: None,
            nebula_page: 0,
            nebula_cat: String::new(),
            nebula_find: String::new(),
            chan_settings: None,
            ram: (None, 0.0),
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
                loop_only: false,
                stems: false,
                made_for: None,
                working: None,
                message: None,
                progress: None,
                started: None,
                tempo: (120.0, 4.0),
            },
            status: "Ready".into(),
            pending: None,
            recent_target: None,
            seen: Seen::default(),
            synth_id: None,
            file_hover: None,
            notice: None,
            placed: false,
            synth_footer: 0.0,
            last_screen: None,
        }
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
    Record,
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
        Icon::Record => {
            ui.painter()
                .circle_filled(c, 6.0, Color32::from_rgb(235, 70, 60));
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

/// The memory this program uses, megabytes (0 where the system does not say).
pub fn ram_mb() -> f64 {
    memory_stats::memory_stats().map_or(0.0, |m| m.physical_mem as f64 / (1024.0 * 1024.0))
}

/// The CPU and memory panel of FL Studio's top bar: how much of each audio buffer's
/// time the engine takes, as a bar graph and a number, and the memory in use.
fn cpu_ram(ui: &mut Ui, load: f32, ram: f64) -> egui::Response {
    let p = pal();
    let (r, resp) = ui.allocate_exact_size(vec2(112.0, 30.0), Sense::hover());
    let paint = ui.painter();
    paint.rect_filled(r, 3.0, p.well_deep);
    let load = load.clamp(0.0, 1.0);
    let hot = load > 0.7;
    // the bar graph: twelve segments, as in FL
    let bars = Rect::from_min_size(r.min + vec2(5.0, 6.0), vec2(44.0, 18.0));
    let lit = (load * 12.0).ceil() as usize;
    for i in 0..12 {
        let x = bars.left() + i as f32 * (bars.width() / 12.0);
        let h = bars.height() * (0.35 + 0.65 * i as f32 / 11.0);
        let seg = Rect::from_min_max(
            pos2(x, bars.bottom() - h),
            pos2(x + bars.width() / 12.0 - 1.0, bars.bottom()),
        );
        let on = i < lit;
        let col = if !on {
            p.lcd.gamma_multiply(0.15)
        } else if i >= 9 {
            p.hot
        } else {
            p.lcd
        };
        paint.rect_filled(seg, 0.0, col);
    }
    let text = |pos: egui::Pos2, s: String, size: f32, c: Color32| {
        paint.text(pos, Align2::LEFT_TOP, s, FontId::monospace(size), c);
    };
    text(
        pos2(r.left() + 54.0, r.top() + 3.0),
        format!("{:>3.0}%", load * 100.0),
        12.0,
        if hot { p.hot } else { p.lcd },
    );
    text(
        pos2(r.left() + 54.0, r.top() + 16.0),
        format!("{ram:>4.0} MB"),
        10.5,
        p.lcd.gamma_multiply(0.8),
    );
    resp.on_hover_text(format!(
        "CPU: the engine takes {:.0}% of the time of each audio buffer\nRAM: {ram:.0} MB in use",
        load * 100.0
    ))
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
            ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
            logo(ui);
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
            let rec = self.shell.record;
            let what = match (self.settings.record_notes, self.settings.record_audio) {
                (true, true) => "notes and audio",
                (true, false) => "notes",
                (false, true) => "audio",
                (false, false) => "nothing (right click to choose)",
            };
            let r = icon_button(
                ui,
                Icon::Record,
                rec,
                &format!(
                    "Record {what}: switch on, then press Play. Notes go into the current pattern on the selected channel.\nRight click for recording options."
                ),
            );
            if r.clicked() {
                self.shell.record = !rec;
            }
            let mut changed = false;
            r.context_menu(|ui| {
                let s = &mut self.settings;
                ui.label(RichText::new("Recording").strong());
                changed |= ui.checkbox(&mut s.record_notes, "Notes (MIDI and computer keyboard)").changed();
                changed |= ui.checkbox(&mut s.record_audio, "Audio input").changed();
                ui.separator();
                changed |= ui
                    .checkbox(&mut s.quantize_recording, "Quantize notes to the piano roll's snap")
                    .changed();
                changed |= ui
                    .checkbox(&mut s.count_in, "Count in one bar")
                    .on_hover_text("The metronome clicks one bar before recording starts")
                    .changed();
            });
            if changed {
                self.settings.save();
            }
            let metro = self.shell.metronome;
            if icon_button(ui, Icon::Metronome, metro, "Metronome").clicked() {
                self.shell.metronome = !metro;
                self.send(Command::Metronome(!metro));
            }
            // tempo: while a tempo automation clip plays the song, the tempo there
            let live = self.project.loaded.as_ref().and_then(|l| {
                let song = playing && self.shell.song_mode && !self.project.looping;
                las_engine::timeline::has_tempo_automation(&l.project, song)
                    .then(|| las_engine::timeline::tempo_at(&l.project, pos, true))
            });
            let r = lcd(ui, &format!("{:.2}", live.unwrap_or(bpm)), "BPM", 86.0);
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
            let r = r.on_hover_text(
                "Drag up and down to change the tempo. Right click: Create automation clip (the tempo then follows it in the song)",
            );
            let mut automate = None;
            r.context_menu(|ui| {
                if ui.button("Create automation clip").clicked() {
                    let (lo, hi) = las_engine::timeline::TEMPO_RANGE;
                    automate = Some(crate::auto::AutoRequest {
                        target: crate::auto::master_target("tempo"),
                        name: "Tempo".into(),
                        start: ((bpm - lo) / (hi - lo)).clamp(0.0, 1.0),
                    });
                    ui.close();
                }
            });
            if let Some(r) = automate {
                self.project.create_automation(&r);
            }
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
            lcd(ui, &bar_beat_step(shown, sig), "BAR:BEAT:STEP", 96.0);
            // the memory use is read once a second
            let stale = self
                .shell
                .ram
                .0
                .is_none_or(|t| t.elapsed().as_secs_f32() > 1.0);
            if stale {
                self.shell.ram = (Some(Instant::now()), ram_mb());
            }
            let load = self.audio.as_ref().map_or(0.0, |a| a.meters.load());
            cpu_ram(ui, load, self.shell.ram.1);
            // pattern
            if let Some(l) = &self.project.loaded {
                let project = l.project.clone();
                let name = project
                    .patterns
                    .get(self.project.pattern)
                    .map_or("-".to_string(), |x| x.name.clone());
                egui::ComboBox::from_id_salt("pattern")
                    .selected_text(name)
                    .width(96.0)
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
                ui.menu_button("...", |ui| {
                    let i = self.project.pattern;
                    if ui
                        .button("Clone pattern")
                        .on_hover_text("A copy of this pattern, to change without changing it")
                        .clicked()
                    {
                        self.project.modify(true, |p| {
                            crate::menus::clone_pattern(p, i);
                        });
                        self.project.pattern = i + 1;
                        ui.close();
                    }
                    if ui.button("Rename pattern...").clicked() {
                        if let Some(x) = project.patterns.get(i) {
                            self.project.rename =
                                Some((crate::project::RenameTarget::Pattern(i), x.name.clone()));
                        }
                        ui.close();
                    }
                    if ui
                        .add_enabled(project.patterns.len() > 1, egui::Button::new("Delete pattern"))
                        .on_hover_text("Remove this pattern and its clips in the playlist")
                        .clicked()
                    {
                        self.project.modify(true, |p| {
                            crate::menus::delete_pattern(p, i);
                        });
                        self.project.pattern = i.saturating_sub(1);
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Clone, rename or delete the pattern");
            }
            // windows
            let o = self.shell.open;
            let labels = [
                "Rack", "Playlist", "Roll", "Mixer", "Synth", "Analyser", "Browser",
            ];
            let act = [
                o.rack,
                o.playlist,
                o.roll,
                o.mixer,
                o.synth,
                o.analyser,
                self.shell.browser,
            ];
            if let Some(i) = segmented(ui, &labels, &act) {
                match i {
                    0 => {
                        let c = self.covered(ui.ctx(), Win::Rack);
                        self.shell.open.toggle(Win::Rack, c);
                    }
                    1 => {
                        let c = self.covered(ui.ctx(), Win::Playlist);
                        self.shell.open.toggle(Win::Playlist, c);
                    }
                    2 => {
                        let c = self.covered(ui.ctx(), Win::Roll);
                        self.shell.open.toggle(Win::Roll, c);
                    }
                    3 => {
                        let c = self.covered(ui.ctx(), Win::Mixer);
                        self.shell.open.toggle(Win::Mixer, c);
                    }
                    4 => {
                        let c = self.covered(ui.ctx(), Win::Synth);
                        self.shell.open.toggle(Win::Synth, c);
                    }
                    5 => {
                        let c = self.covered(ui.ctx(), Win::Analyser);
                        self.shell.open.toggle(Win::Analyser, c);
                    }
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
                self.shell.open.show(crate::raise::Win::Export);
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
                RichText::new(self.wf.note.as_deref().unwrap_or(&self.shell.status)).color(p.muted)
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
                    None => RichText::new("No sound: click to set up audio").color(p.hot),
                };
                let tip = if self.audio.is_some() {
                    "Click to choose the audio device in Settings".to_string()
                } else {
                    format!(
                        "The audio output could not be opened: {}\nClick to choose a device in Settings.",
                        self.status_audio()
                    )
                };
                let r = ui
                    .add(egui::Label::new(audio.size(12.0)).sense(Sense::click()))
                    .on_hover_text(tip);
                if r.clicked() {
                    self.shell.open.show(crate::raise::Win::Settings);
                }
                if let Some(c) = self
                    .audio
                    .as_ref()
                    .map(|a| a.meters.counting())
                    .filter(|c| *c > 0)
                {
                    ui.separator();
                    ui.label(
                        RichText::new(format!("Count-in {c}"))
                            .size(12.0)
                            .strong()
                            .color(p.hot),
                    );
                }
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
        self.audio_error.clone()
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
                ui.add_space(6.0);
                self.preset_list(ui);
            }
            BrowserTab::Project => self.project_tab(ui),
        }
    }

    /// The presets of the browser's synth, each one dragged onto a channel of that synth
    /// to load it there (as in FL), or clicked to hear it.
    fn preset_list(&mut self, ui: &mut Ui) {
        let p = pal();
        ui.label(
            RichText::new("Drag one onto a channel of this synth in the rack")
                .size(11.0)
                .color(p.muted),
        );
        let inst = self.inst;
        let k = crate::app::index(inst);
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("preset-list")
            .show(ui, |ui| {
                let mut shown = "";
                for (i, e) in self.entries[k].iter().enumerate() {
                    if !crate::presets::matches(e, &self.search) {
                        continue;
                    }
                    if e.category != shown {
                        shown = &e.category;
                        ui.label(
                            RichText::new(shown)
                                .size(11.0)
                                .strong()
                                .color(p.accent_text),
                        );
                    }
                    let r = ui
                        .selectable_label(self.presets[k] == i, &e.name)
                        .interact(Sense::click_and_drag())
                        .on_hover_text(
                            "Click to load it. Drag it onto a channel to load it there.",
                        );
                    r.dnd_set_drag_payload(Dragged::Preset(inst, e.name.clone()));
                    if r.clicked() {
                        pick = Some(i);
                    }
                }
            });
        if let Some(i) = pick {
            self.load_preset(inst, i);
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
                                let r = ui
                                    .selectable_label(false, s.name)
                                    .interact(egui::Sense::click_and_drag())
                                    .on_hover_text(
                                        "Click to hear it. Drag it to the playlist or the rack. Double click adds it as a channel.",
                                    );
                                r.dnd_set_drag_payload(Dragged::Factory(s.id, s.name));
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
            self.folders_ui(ui, &query);
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
            for (name, id, what) in [
                ("Polaris", "polaris", "Wavetable synth"),
                ("Facet", "facet", "Four-operator FM synth"),
                ("Mist", "mist", "Granular synth"),
                ("Vortex", "vortex", "Wavetable synth with a 3D view"),
                ("Monolith", "monolith", "Subtractive synth"),
                ("Zenith", "zenith", "Supersaw and pads"),
                ("TriOsc", "triosc", "Three oscillator synth"),
                ("Nebula", "nebula", "Layered macro instrument"),
                ("Sampler", "sampler", "Plays a sample across the keys"),
                ("LumaPad", "lumapad", "16 pads with choke groups"),
                ("Weave", "weave", "Modular synth: modules and patch cables"),
            ] {
                ui.horizontal(|ui| {
                    let r = ui
                        .add(egui::Button::new(name).min_size(vec2(90.0, 0.0)))
                        .on_hover_text("Add as a channel and open its window");
                    if r.clicked() && self.project.loaded.is_some() {
                        self.add_channel(id);
                        let at = self.project.channel;
                        self.open_channel(at);
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
            self.hosted_list(ui);
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

    /// Whether window `w` is open behind another one.
    pub(crate) fn covered(&self, ctx: &egui::Context, w: Win) -> bool {
        let id = match w {
            Win::Synth => match self.shell.synth_id {
                Some(id) => id,
                None => return false,
            },
            _ => w.id(),
        };
        crate::raise::covered(ctx, id)
    }

    /// Bring to the front every window opened since the last frame, by whatever
    /// path: its flag, or a new effect, automation clip, channel or sample.
    fn raise_newly_opened(&mut self) {
        let now = Seen {
            open: Some(self.shell.open),
            effect: self.project.open_effect,
            auto: self.project.open_auto.clone(),
            chan: self.shell.chan_settings,
            sample: self.sample_editor.as_ref().map(|e| e.channel.clone()),
            bound: self.bound.map(|b| b.channel()),
        };
        let was = std::mem::replace(&mut self.shell.seen, now.clone());
        if let Some(before) = was.open {
            crate::raise::newly_opened(&before, &self.shell.open)
                .into_iter()
                .for_each(crate::raise::raise);
        }
        let changed = |a: bool, w: Win| {
            if a {
                crate::raise::raise(w);
            }
        };
        changed(
            now.effect.is_some() && now.effect != was.effect,
            Win::Effect,
        );
        changed(now.auto.is_some() && now.auto != was.auto, Win::Automation);
        changed(
            now.chan.is_some() && now.chan != was.chan,
            Win::ChanSettings,
        );
        changed(
            now.sample.is_some() && now.sample != was.sample,
            Win::SampleEditor,
        );
        changed(
            self.shell.open.synth && now.bound.is_some() && now.bound != was.bound,
            Win::Synth,
        );
    }

    /// The floating windows. `area` is the workspace they sit in.
    pub(crate) fn windows(&mut self, ctx: &egui::Context, area: Rect) {
        // the screen got smaller (a smaller window, a larger interface scale): windows
        // that no longer fit are laid out afresh, at their first places and sizes, so
        // no edge, title or close button is left out of reach
        let screen = ctx.content_rect();
        let shrank = self.shell.last_screen.is_some_and(|s| {
            screen.width() < s.width() - 1.0 || screen.height() < s.height() - 1.0
        });
        self.shell.last_screen = Some(screen);
        if shrank {
            let off = ctx.memory(|m| {
                m.layer_ids()
                    .filter(|l| l.order == egui::Order::Middle)
                    .filter_map(|l| m.area_rect(l.id))
                    .any(|r| !screen.expand(1.0).contains_rect(r))
            });
            if off {
                ctx.memory_mut(|m| m.reset_areas());
                ctx.data_mut(|d| d.clear());
                self.shell.placed = false;
            }
        }
        let a = area.shrink(6.0);
        self.shell.steps.room_bottom = Some(area.bottom());
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
        self.raise_newly_opened();
        let mut open = self.shell.open;
        let before = open;
        let win = |title: &str, pos: egui::Pos2, size: egui::Vec2| {
            // never bigger than the room the windows have: what does not fit scrolls or
            // is cut, and every edge stays in reach to resize
            let mut w = egui::Window::new(title)
                .constrain_to(area)
                .resizable(true)
                .max_width(area.width() - 16.0)
                .max_height(area.height() - 12.0)
                .default_size(size);
            if first {
                w = w.default_pos(pos);
            }
            w
        };
        let have = self.project.loaded.is_some();
        if open.playlist {
            crate::raise::front(ctx, Win::Playlist, Win::Playlist.id());
            win("Playlist - Arrangement", a.min, vec2(a.width(), a.height()))
                .id(Win::Playlist.id())
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
            crate::raise::front(ctx, Win::Rack, Win::Rack.id());
            win(
                "Channel Rack",
                a.min + vec2(170.0, 80.0),
                // tall enough for a song's channels; more scroll
                vec2(740.0, (a.height() - 120.0).clamp(300.0, 560.0)),
            )
            .id(Win::Rack.id())
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
            crate::raise::front(ctx, Win::Roll, Win::Roll.id());
            win(
                "Piano Roll",
                a.min + vec2(60.0, 40.0),
                vec2(a.width() - 120.0, a.height() - 80.0),
            )
            .id(Win::Roll.id())
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
            crate::raise::front(ctx, Win::Mixer, Win::Mixer.id());
            win(
                "Mixer",
                a.min + vec2(30.0, (a.height() - 470.0).max(10.0)),
                vec2(a.width() - 60.0, 450.0),
            )
            .id(Win::Mixer.id())
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
            // each face has its own window size (TriOsc is a small one)
            let face = match self.bound {
                None | Some(crate::inst::Bound::Synth(_)) => self.face_id(),
                Some(crate::inst::Bound::Nebula(_)) => "nebula".into(),
                _ => String::new(),
            };
            let size = match face.as_str() {
                "triosc" => vec2(640.0, 560.0),
                "monolith" => vec2(1060.0, 740.0),
                "zenith" => vec2(1000.0, 790.0),
                "nebula" => vec2(1100.0, 660.0),
                _ => vec2(900.0, 680.0),
            };
            let first_pos = a.min + vec2(160.0, 30.0);
            // never taller or wider than the screen leaves room for (720p, 900 px): what
            // does not fit scrolls
            let size = vec2(
                size.x.min((a.width() - 20.0).max(320.0)),
                size.y.min((a.height() - 40.0).max(240.0)),
            );
            let first_pos = pos2(
                first_pos.x.min((a.right() - size.x).max(a.left())),
                first_pos.y,
            );
            // the window is named after the channel it shows
            let title = self
                .bound
                .and_then(|b| {
                    self.project
                        .loaded
                        .as_ref()?
                        .project
                        .channels
                        .get(b.channel())
                        .map(|c| c.name.clone())
                })
                .unwrap_or_else(|| "Instrument".into());
            let id = egui::Id::unique(("synth-window", face));
            crate::raise::front(ctx, Win::Synth, id);
            self.shell.synth_id = Some(id);
            win(&title, first_pos, size)
                .id(id)
                .open(&mut open.synth)
                .show(ctx, |ui| match self.bound {
                    Some(b) if !matches!(b, crate::inst::Bound::Synth(_)) => {
                        egui::ScrollArea::both()
                            .id_salt("instrument-scroll")
                            .show(ui, |ui| self.bound_window(ui, b));
                    }
                    _ => self.synth_window(ui),
                });
        }
        if let Some(c) = self.shell.chan_settings {
            let mut keep = true;
            let name = self
                .project
                .loaded
                .as_ref()
                .and_then(|l| l.project.channels.get(c))
                .map(|ch| ch.name.clone());
            match name {
                Some(name) => {
                    crate::raise::front(ctx, Win::ChanSettings, Win::ChanSettings.id());
                    win(
                        &format!("Channel settings: {name}"),
                        a.min + vec2(300.0, 110.0),
                        vec2(460.0, 420.0),
                    )
                    .id(Win::ChanSettings.id())
                    .open(&mut keep)
                    .show(ctx, |ui| self.shared_settings(ui, c));
                }
                None => keep = false,
            }
            if !keep {
                self.shell.chan_settings = None;
            }
        }
        if let Some((insert, slot)) = self.project.open_effect {
            let mut keep = true;
            crate::raise::front(ctx, Win::Effect, Win::Effect.id());
            win("Effect", a.min + vec2(260.0, 120.0), vec2(620.0, 420.0))
                .id(Win::Effect.id())
                .open(&mut keep)
                .show(ctx, |ui| {
                    let mut skin = crate::skin::skin_for("fx");
                    skin.name = self.effect_title(insert, slot);
                    crate::skin::face(ui, skin, |ui| self.effect_window(ui, insert, slot))
                });
            if !keep {
                self.project.open_effect = None;
            }
        }
        self.sample_editor_window(ctx, area);
        if let Some(id) = self.project.open_auto.clone() {
            let mut keep = true;
            crate::raise::front(ctx, Win::Automation, Win::Automation.id());
            win("Automation", a.min + vec2(220.0, 140.0), vec2(620.0, 380.0))
                .id(Win::Automation.id())
                .open(&mut keep)
                .show(ctx, |ui| self.auto_window(ui, &id));
            if !keep {
                self.project.open_auto = None;
            }
        }
        if open.export {
            crate::raise::front(ctx, Win::Export, Win::Export.id());
            egui::Window::new("Export audio")
                .id(Win::Export.id())
                .pivot(egui::Align2::CENTER_CENTER)
                .default_pos(ctx.content_rect().center())
                .open(&mut open.export)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| self.export_window(ui));
        }
        // only what the windows' own close buttons changed is taken back: a window
        // opened from inside another one (the rack's "Piano roll") stays open
        self.shell.open.close_from(&before, &open);
        self.about_window(ctx);
        self.keys_window(ctx);
        self.history_window(ctx);
        self.settings_window(ctx);
        self.notice_window(ctx);
        self.drag_label(ctx);
        self.analyser_window(ctx);
        self.rename_dialog(ctx);
        self.discard_dialog(ctx);
        self.workflow_windows(ctx);
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
        let footer_top = ui.cursor().top();
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
            self.capture_button(ui);
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
        // how tall the buttons under the rows are, so the rows leave them room
        self.shell.steps.footer = ui.cursor().top() - footer_top;
        drop(project);
        self.project.channel = selected;
        if let Some(n) = out.steps {
            let pattern = self.project.pattern;
            self.project.modify(true, |p| {
                if let Some(pat) = p.patterns.get_mut(pattern) {
                    pat.length = n as f64 * f64::from(las_project::TICKS_PER_STEP);
                }
            });
        }
        if let Some(c) = out.open_roll {
            self.project.channel = c;
            self.shell.open.show(crate::raise::Win::Roll);
        }
        if let Some(c) = out.open_synth {
            self.open_channel(c);
        }
        if let Some(c) = out.open_settings {
            self.open_channel_settings(c);
        }
        if let Some((c, pat)) = out.render {
            self.rack_render(c, pat);
        }
        for (ch, e) in &out.notes {
            self.project.edit_channel(*ch, std::slice::from_ref(e));
        }
        if !out.chans.is_empty() {
            self.project.edit_chans(&out.chans);
        }
    }

    /// The editor of an automation clip: its name, length, curve, and a way to delete it.
    fn auto_window(&mut self, ui: &mut Ui, id: &str) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let Some(clip) = project.automation.iter().find(|c| c.id == id) else {
            ui.label("This automation clip no longer exists.");
            return;
        };
        let mut name = clip.name.clone();
        let mut bars = (clip.length / (f64::from(PPQ) * 4.0)).round().max(1.0);
        let mut delete = false;
        ui.horizontal(|ui| {
            if ui
                .add(egui::TextEdit::singleline(&mut name).desired_width(180.0))
                .changed()
            {
                let (id, n) = (clip.id.clone(), name.clone());
                self.project.modify_coalesced("auto-name", move |p| {
                    if let Some(c) = p.automation.iter_mut().find(|c| c.id == id) {
                        c.name = n;
                    }
                });
            }
            ui.label(
                RichText::new(crate::auto::describe_target(&project, &clip.target))
                    .weak()
                    .size(11.5),
            );
            ui.label("Bars");
            if ui
                .add(egui::DragValue::new(&mut bars).range(1.0..=64.0).speed(0.1))
                .changed()
            {
                let (id, len) = (clip.id.clone(), bars * f64::from(PPQ) * 4.0);
                self.project.modify_coalesced("auto-len", move |p| {
                    if let Some(c) = p.automation.iter_mut().find(|c| c.id == id) {
                        let old = c.length;
                        c.length = len;
                        for x in &mut c.points {
                            x.t = (x.t / old * len).min(len);
                        }
                    }
                    for c in &mut p.playlist.clips {
                        if c.kind == las_project::ClipKind::Automation && c.ref_id == id {
                            c.length = len;
                        }
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Delete clip").clicked() {
                    delete = true;
                }
            });
        });
        ui.label(
            RichText::new("Click to add a point and drag to move it. Right click a point for its curve (single, double, hold, stairs, smooth) or to delete it. Drag the small ring between two points up or down to bend the curve, double click it to straighten.")
                .weak()
                .size(11.0),
        );
        let mut edits = crate::auto::lfo_row(ui, clip, &mut self.project.auto_state);
        edits.extend(crate::auto::draw(ui, clip, &mut self.project.auto_state));
        let cid = clip.id.clone();
        drop(project);
        self.project.edit_auto(&edits);
        if delete {
            self.project.modify(true, |p| {
                p.automation.retain(|c| c.id != cid);
                p.playlist
                    .clips
                    .retain(|c| !(c.kind == las_project::ClipKind::Automation && c.ref_id == cid));
            });
            self.project.open_auto = None;
        }
    }

    fn mixer_window(&mut self, ui: &mut Ui) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let peaks: Vec<f32> = match &self.audio {
            Some(a) if a.meters.playing() => (0..project.mixer.len())
                .map(|i| a.meters.insert_peak(i))
                .collect(),
            _ => Vec::new(),
        };
        self.reference_bar(ui);
        let ch = self.project.channel;
        let selected = project.channels.get(ch).map(|c| (ch, c.name.as_str()));
        // the strips scroll up and down when the window is lower than they are (a
        // small screen, or a large interface scale)
        let state = &mut self.project.mixer_state;
        let mut edits = egui::ScrollArea::vertical()
            .id_salt("mixer-scroll")
            .show(ui, |ui| mixer::draw(ui, &project, state, &peaks, selected))
            .inner;
        // the effects chain files
        let mut more = Vec::new();
        edits.retain(|e| match e {
            MixEdit::SaveChain(i) => {
                if let Some(t) = project.mixer.get(*i) {
                    let file = rfd::FileDialog::new()
                        .add_filter("Effects chain", &["json"])
                        .set_title("Save chain")
                        .set_file_name(format!("{}.json", mixer_file_name(t, *i)))
                        .save_file();
                    if let Some(f) = file {
                        if let Err(e) = std::fs::write(&f, mixer::chain_json(t)) {
                            self.shell.notice = Some(format!("{}: {e}", f.display()));
                        }
                    }
                }
                false
            }
            MixEdit::LoadChain(i) => {
                let file = rfd::FileDialog::new()
                    .add_filter("Effects chain", &["json"])
                    .set_title("Load chain")
                    .pick_file();
                if let Some(f) = file {
                    match std::fs::read_to_string(&f)
                        .map_err(|e| e.to_string())
                        .and_then(|t| mixer::parse_chain(&t))
                    {
                        Ok(slots) => {
                            more.push(MixEdit::Begin);
                            more.push(MixEdit::SetChain { insert: *i, slots });
                        }
                        Err(e) => self.shell.notice = Some(format!("{}: {e}", f.display())),
                    }
                }
                false
            }
            _ => true,
        });
        edits.extend(more);
        let edits = self.workflow_mix_edits(edits);
        drop(project);
        self.project.edit_mixer(&edits);
    }

    /// The window of a channel whose instrument has its own face (not the synth panels).
    fn bound_window(&mut self, ui: &mut Ui, b: crate::inst::Bound) {
        use crate::inst::Bound;
        match b {
            Bound::Basic(c) => self.basic_editor(ui, c),
            Bound::Hosted(c) => self.hosted_channel(ui, c),
            Bound::Nebula(c) => crate::skin::face(ui, crate::skin::skin_for("nebula"), |ui| {
                self.nebula_window(ui, c)
            }),
            Bound::Weave(c) => crate::skin::face(ui, crate::skin::skin_for("weave"), |ui| {
                self.weave_window(ui, c)
            }),
            Bound::Pads(c) => crate::skin::face(ui, crate::skin::skin_for("lumapad"), |ui| {
                self.pads_window(ui, c)
            }),
            Bound::Sampler(c) => crate::skin::face(ui, crate::skin::skin_for("sampler"), |ui| {
                self.sampler_window(ui, c)
            }),
            Bound::Synth(_) => self.synth_window(ui),
        }
    }

    fn synth_window(&mut self, ui: &mut Ui) {
        let skin = crate::skin::skin_for(&self.face_id());
        crate::skin::face(ui, skin, |ui| self.synth_window_inner(ui));
    }

    fn synth_window_inner(&mut self, ui: &mut Ui) {
        if let (Some(b), Some(l)) = (self.bound, &self.project.loaded) {
            if let Some(c) = l.project.channels.get(b.channel()) {
                ui.label(RichText::new(&c.name).strong().size(15.0));
            }
        } else {
            ui.label(RichText::new("Stand-alone synth (not part of the project)").weak());
        }
        let bound = self.bound.is_some();
        ui.horizontal(|ui| {
            // a project channel keeps its instrument; the stand-alone synth can switch
            if !bound {
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
            }
            ui.label("Preset");
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
        let footer = self.shell.synth_footer.max(60.0);
        egui::ScrollArea::both()
            .max_height((ui.available_height() - footer).max(160.0))
            .show(ui, |ui| {
                if self.view == crate::app::View::Effects {
                    self.effects_view(ui);
                    return;
                }
                match self.inst {
                    Instrument::Facet => self.synth(ui),
                    Instrument::Mist => self.mist_panel(ui),
                    Instrument::Polaris => match self.face_id().as_str() {
                        "monolith" => self.monolith_panel(ui),
                        "zenith" => self.zenith_panel(ui),
                        "triosc" => self.triosc_panel(ui),
                        _ => self.polaris_panel(ui),
                    },
                }
            });
        let top = ui.cursor().top();
        ui.separator();
        self.keyboard(ui);
        self.shell.synth_footer = ui.cursor().top() - top;
    }

    fn export_window(&mut self, ui: &mut Ui) {
        if let Some(l) = &self.project.loaded {
            let now = (l.path.clone(), l.project.name.clone());
            let e = &mut self.shell.export;
            let stale = e.made_for.as_ref().is_some_and(|m| *m != now);
            if e.path.is_empty() || stale {
                let (_, kind, _) = FORMATS[e.format];
                e.path = default_export_path(&now.0, &now.1, kind);
                e.made_for = Some(now);
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
            ui.label("Render");
            ui.selectable_value(&mut self.shell.export.stems, false, "Mix (one file)")
                .on_hover_text("Everything you hear, in one file");
            ui.selectable_value(
                &mut self.shell.export.stems,
                true,
                "Each mixer track as its own file",
            )
            .on_hover_text(
                "Stems: the master and every mixer track that plays something, each in its own file, for mixing elsewhere",
            );
        });
        if self.shell.export.stems {
            let (_, kind, _) = FORMATS[self.shell.export.format];
            let dir = stem_dir(&self.shell.export.path);
            let name = self.export_name();
            ui.label(
                RichText::new(format!(
                    "Files go in {} as \"{name} - <track name>.{kind}\".",
                    dir.display()
                ))
                .weak()
                .size(11.5),
            );
        }
        ui.horizontal(|ui| {
            ui.label("Save to");
            if ui
                .add(egui::TextEdit::singleline(&mut self.shell.export.path).desired_width(320.0))
                .changed()
            {
                self.shell.export.made_for = None;
            }
            let stems = self.shell.export.stems;
            if stems
                && ui
                    .add_enabled(!busy, egui::Button::new("Choose folder..."))
                    .on_hover_text("Pick the folder the files go in")
                    .clicked()
            {
                let (_, kind, _) = FORMATS[self.shell.export.format];
                let mut dialog = rfd::FileDialog::new().set_title("Export stems to a folder");
                let dir = stem_dir(&self.shell.export.path);
                if dir.is_dir() {
                    dialog = dialog.set_directory(&dir);
                }
                if let Some(d) = dialog.pick_folder() {
                    let file = format!("{}.{kind}", self.export_name());
                    self.shell.export.path = d.join(file).to_string_lossy().into_owned();
                    self.shell.export.made_for = None;
                }
            }
            if !stems
                && ui
                    .add_enabled(!busy, egui::Button::new("Choose..."))
                    .on_hover_text("Pick the folder and file name")
                    .clicked()
            {
                let (name, kind, _) = FORMATS[self.shell.export.format];
                let now = std::path::PathBuf::from(&self.shell.export.path);
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Export audio")
                    .add_filter(name, &[kind]);
                if let Some(dir) = now.parent().filter(|d| d.is_dir()) {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(file) = now.file_name() {
                    dialog = dialog.set_file_name(file.to_string_lossy());
                }
                if let Some(mut p) = dialog.save_file() {
                    if p.extension().is_none() {
                        p.set_extension(kind);
                    }
                    self.shell.export.path = p.to_string_lossy().into_owned();
                    self.shell.export.made_for = None;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Format");
            let f = self.shell.export.format;
            egui::ComboBox::from_id_salt("export-format")
                .selected_text(FORMATS[f].0)
                .show_ui(ui, |ui| {
                    for (i, (name, kind, _)) in FORMATS.iter().enumerate() {
                        if ui
                            .selectable_value(&mut self.shell.export.format, i, *name)
                            .clicked()
                        {
                            // the file name follows the format
                            let path = std::path::Path::new(&self.shell.export.path);
                            self.shell.export.path =
                                path.with_extension(kind).to_string_lossy().into_owned();
                        }
                    }
                });
            let label = if self.shell.export.stems {
                "Normalize each file to -0.3 dB"
            } else {
                "Normalize to -0.3 dB"
            };
            ui.checkbox(&mut self.shell.export.normalize, label)
                .on_hover_text("Make the loudest moment reach -0.3 dB");
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.shell.export.pattern_only, false, "Song");
            ui.selectable_value(&mut self.shell.export.pattern_only, true, "Chosen pattern");
            let has_loop = self.project.playlist.song_loop.is_some();
            if !has_loop {
                self.shell.export.loop_only = false;
            }
            ui.add_enabled(
                has_loop && !self.shell.export.pattern_only,
                egui::Checkbox::new(&mut self.shell.export.loop_only, "Only the loop"),
            )
            .on_hover_text(
                "Render the stretch the playlist loops (right-click the playlist to set one)",
            );
        });
        ui.add_space(4.0);
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(RichText::new("Render").strong()).min_size(vec2(120.0, 30.0)),
            )
            .clicked()
        {
            self.start_export();
        }
        if busy {
            if let Some(p) = &self.shell.export.progress {
                let e = &self.shell.export;
                let f = p.fraction();
                ui.add(
                    egui::ProgressBar::new(f as f32)
                        .text(format!("{:.0}%", f * 100.0))
                        .desired_width(ui.available_width()),
                );
                let total = p.total.load(std::sync::atomic::Ordering::Relaxed) as f64 / 44_100.0;
                let done = total * f;
                let (bpm, beats) = e.tempo;
                let bar = |s: f64| (s * bpm / 60.0 / beats).floor() as u64 + 1;
                let elapsed = e.started.map_or(0.0, |t| t.elapsed().as_secs_f64());
                let left = if f > 0.02 {
                    format!(", about {} left", clock(elapsed / f - elapsed))
                } else {
                    String::new()
                };
                ui.label(format!(
                    "Bar {} of {}   {} of {}{left}",
                    bar(done).min(bar(total)),
                    bar(total),
                    clock(done),
                    clock(total),
                ));
            }
        }
        match &self.shell.export.message {
            Some(Ok(m)) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(m).color(pal().ok));
                    if ui.button("Show in folder").clicked() {
                        if self.shell.export.stems {
                            let dir = stem_dir(&self.shell.export.path);
                            show_in_folder(&dir.join("x").to_string_lossy());
                        } else {
                            show_in_folder(&self.shell.export.path);
                        }
                    }
                });
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
        let name = self.export_name();
        let e = &self.shell.export;
        let (path, normalize, stems) = (e.path.trim().to_string(), e.normalize, e.stems);
        let (_, kind, bits) = FORMATS[e.format];
        let mode = if e.pattern_only {
            Mode::Pattern {
                id: project
                    .patterns
                    .get(self.project.pattern)
                    .map(|p| p.id.clone()),
                loops: 1,
            }
        } else if let Some((from, to)) = self.project.playlist.song_loop.filter(|_| e.loop_only) {
            Mode::Range { from, to }
        } else {
            Mode::Song
        };
        let (tx, rx) = channel();
        let progress = std::sync::Arc::new(las_engine::render::Progress::default());
        let watch = progress.clone();
        self.shell.export.progress = Some(progress);
        self.shell.export.started = Some(Instant::now());
        self.shell.export.tempo = (project.bpm.max(1.0), f64::from(project.time_sig.0.max(1)));
        std::thread::spawn(move || {
            las_engine::render::watch_progress(Some(watch));
            let run = || -> Result<String, String> {
                if let Some(dir) = std::path::Path::new(&path).parent() {
                    if !dir.as_os_str().is_empty() {
                        std::fs::create_dir_all(dir)
                            .map_err(|e| format!("{}: {e}", dir.display()))?;
                    }
                }
                let opts = RenderOptions {
                    sample_rate: 44100.0,
                    mode,
                    ..Default::default()
                };
                if stems {
                    let dir = stem_dir(&path);
                    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    let all = las_engine::render_stems(&project, &samples, &opts)?;
                    let files = stem_files(&name, &project, kind);
                    let mut secs = 0.0;
                    for (stem, file) in all.iter().zip(&files) {
                        let to = dir.join(file);
                        let bytes = encode(&stem.audio, normalize, kind, bits)?;
                        std::fs::write(&to, bytes).map_err(|e| format!("{}: {e}", to.display()))?;
                        secs = stem.audio.seconds();
                    }
                    return Ok(format!(
                        "{} files ({secs:.1} s each) in {}",
                        all.len().min(files.len()),
                        dir.display()
                    ));
                }
                let r = las_engine::render_mix_with(&project, &samples, &opts)?;
                let bytes = encode(&r, normalize, kind, bits)?;
                std::fs::write(&path, bytes).map_err(|e| format!("{path}: {e}"))?;
                Ok(format!("{path}: {:.1} s", r.seconds()))
            };
            let _ = tx.send(run());
        });
        self.shell.export.working = Some(rx);
        self.shell.export.message = None;
    }
}

impl App {
    /// The name exports are called by: the project's file name, or "Untitled".
    pub(crate) fn export_name(&self) -> String {
        let from_file = std::path::Path::new(&self.project.path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty());
        let from_name = self
            .project
            .loaded
            .as_ref()
            .map(|l| {
                l.project
                    .name
                    .trim()
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || " -_()".contains(c) {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>()
            })
            .filter(|s| !s.trim().is_empty());
        from_file.or(from_name).unwrap_or_else(|| "Untitled".into())
    }
}

/// A render as the bytes of a file of the chosen format.
fn encode(
    r: &las_engine::Rendered,
    normalize: bool,
    kind: &str,
    bits: u32,
) -> Result<Vec<u8>, String> {
    let chans = [&r.left[..], &r.right[..]];
    let scaled;
    let chans: [&[f32]; 2] = if normalize {
        scaled = las_engine::export::normalize(&chans, -0.3);
        [&scaled[0], &scaled[1]]
    } else {
        chans
    };
    let rate = r.sample_rate as u32;
    Ok(if kind == "flac" {
        las_engine::export::encode_flac(&chans, rate, bits)?
    } else if kind == "mp3" {
        las_engine::export::encode_mp3(&chans, rate, bits)?
    } else {
        let depth = las_engine::export::BitDepth::from_bits(bits).ok_or("bad depth")?;
        las_engine::export::encode_wav(&chans, rate, depth, bits == 16)
    })
}

/// The folder stems go in: the folder of the export path (or the path itself when it
/// is a folder).
pub fn stem_dir(path: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(path.trim());
    if p.is_dir() {
        return p;
    }
    p.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map_or_else(|| std::path::PathBuf::from("."), |d| d.to_path_buf())
}

/// A name that is safe as a file name on every system.
fn safe_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let t = t.trim().trim_end_matches('.').to_string();
    if t.is_empty() {
        "_".into()
    } else {
        t
    }
}

/// The file names of the stems `render_stems` makes, in its order (the master, then each
/// mixer track that plays): "<project> - <track name>.<ext>". Tracks with the same name
/// get their number added.
pub fn stem_files(project_name: &str, project: &las_project::Project, ext: &str) -> Vec<String> {
    let inserts = las_engine::used_inserts(project);
    let name_of = |i: usize| {
        project
            .mixer
            .get(i)
            .map(|t| t.name.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Insert {i}"))
    };
    let mut names = vec!["Master".to_string()];
    for &i in &inserts {
        let n = name_of(i);
        let twice = n == "Master" || inserts.iter().filter(|&&j| name_of(j) == n).count() > 1;
        names.push(if twice { format!("{n} ({i})") } else { n });
    }
    names
        .iter()
        .map(|n| format!("{} - {}.{ext}", safe_name(project_name), safe_name(n)))
        .collect()
}

/// Minutes and seconds.
fn clock(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Where an export goes when the user has not chosen: next to the project, or in the
/// Music folder for a project not yet saved (named after the song).
fn default_export_path(project: &str, name: &str, kind: &str) -> String {
    if !project.is_empty() {
        return format!("{}.{kind}", project.trim_end_matches(".las"));
    }
    let name: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = if name.trim().is_empty() {
        "Untitled".to_string()
    } else {
        name.trim().to_string()
    };
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let music = home.join("Music");
    let dir = if music.is_dir() { music } else { home };
    dir.join(format!("{name}.{kind}"))
        .to_string_lossy()
        .into_owned()
}

/// Open the file manager at a file.
fn show_in_folder(path: &str) {
    let p = std::path::Path::new(path);
    let _ = if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", p.display()))
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(p).spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(p.parent().unwrap_or(p))
            .spawn()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_opened_from_inside_another_stays_open() {
        let mut now = Shell::new().open;
        let before = now;
        // the rack's menu opened the piano roll while the windows were drawn
        now.roll = true;
        let mut after = before;
        // and the user closed the playlist with its X in the same frame
        after.playlist = false;
        now.close_from(&before, &after);
        assert!(now.roll);
        assert!(!now.playlist);
        assert!(now.rack);
    }

    #[test]
    fn export_paths_and_clocks_read_well() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(195.4), "3:15");
        assert_eq!(
            default_export_path("/a/song.las", "x", "mp3"),
            "/a/song.mp3"
        );
        assert!(default_export_path("", "", "wav").ends_with("Untitled.wav"));
        assert!(default_export_path("", "Lumen Nights (demo)", "wav")
            .ends_with("Lumen Nights (demo).wav"));
        assert!(default_export_path("", "a/b:c", "wav").ends_with("a_b_c.wav"));
    }

    #[test]
    fn stems_are_named_after_the_project_and_their_tracks() {
        let mut p = las_project::create::empty_project("x");
        for name in ["Kick", "Bass", "Kick"] {
            let i = las_project::create::add_channel(&mut p, "sampler");
            let t = p.channels[i].mixer_track;
            p.mixer[t].name = name.into();
        }
        let files = stem_files("My Song", &p, "wav");
        assert_eq!(
            files,
            vec![
                "My Song - Master.wav",
                "My Song - Kick (1).wav",
                "My Song - Bass.wav",
                "My Song - Kick (3).wav",
            ]
        );
        // one file for each stem the engine renders
        let stems = las_engine::used_inserts(&p).len() + 1;
        assert_eq!(files.len(), stems);
        assert_eq!(safe_name("a/b:c?"), "a_b_c_");
        assert_eq!(
            stem_dir("/tmp/x/song.wav"),
            std::path::PathBuf::from("/tmp/x")
        );
    }

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
    fn exports_offer_wav_flac_and_mp3() {
        for kind in ["wav", "flac", "mp3"] {
            assert!(FORMATS.iter().any(|f| f.1 == kind), "{kind}");
        }
        assert!(
            FORMATS.iter().all(|f| !(f.1 == "flac" && f.2 == 32)),
            "no 32 bit flac"
        );
    }
}
