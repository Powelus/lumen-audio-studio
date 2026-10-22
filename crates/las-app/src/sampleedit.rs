// SPDX-License-Identifier: GPL-3.0-or-later
//! The sample editor (FL Studio's Edison) and the slicer's markers (Slicex).
//!
//! The editor works on a copy of a channel's audio kept in the project: every edit
//! stores the result as a new WAV sample in the project and points the channel at
//! it, so the file the sound came from is never touched and the project's undo puts
//! the old sound back. The beat markers cut the sound into slices for a slicer
//! channel, which plays slice N on key C5 + N.

use crate::project::ProjectState;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui};
use las_engine::factory::Audio;
use las_project::slicer::{self, SliceSpec, SLICER_ID};
use std::cell::RefCell;
use std::sync::Arc;

/// What a menu asks of the sample editor, by channel id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open the channel's sound in the editor.
    Edit(String),
    /// Cut the channel's sound at its beats into a new slicer channel.
    Slice(String),
}

thread_local! {
    static REQUESTS: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
}

/// Ask for the sample editor (from a menu, which cannot reach the app itself).
pub fn request(r: Request) {
    REQUESTS.with(|q| q.borrow_mut().push(r));
}

/// The requests made since the last call.
pub fn take_requests() -> Vec<Request> {
    REQUESTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// The two menu items every sound has: open it in the editor, or slice it.
pub fn menu_items(ui: &mut Ui, channel_id: &str) {
    if ui
        .button("Edit in sample editor")
        .on_hover_text("Open this sound in the sample editor to trim, fade, normalize and more")
        .clicked()
    {
        request(Request::Edit(channel_id.to_string()));
        ui.close();
    }
    if ui
        .button("Slice to new channel")
        .on_hover_text(
            "Cut this sound at its beats into a Slicer channel that plays slice N on key C5+N, with a pattern that plays the loop",
        )
        .clicked()
    {
        request(Request::Slice(channel_id.to_string()));
        ui.close();
    }
}

/// A destructive edit of the sound (or of the selected part of it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Trim,
    Delete,
    Silence,
    FadeIn,
    FadeOut,
    Normalize,
    Reverse,
    /// dB.
    Gain(f32),
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Trim => "Trim to selection",
            Op::Delete => "Delete",
            Op::Silence => "Silence",
            Op::FadeIn => "Fade in",
            Op::FadeOut => "Fade out",
            Op::Normalize => "Normalize",
            Op::Reverse => "Reverse",
            Op::Gain(_) => "Apply gain",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Op::Trim => "Keep only the selection",
            Op::Delete => "Cut the selection out; what follows moves up",
            Op::Silence => "Make the selection (or everything) silent",
            Op::FadeIn => "Fade the selection (or everything) in from silence",
            Op::FadeOut => "Fade the selection (or everything) out to silence",
            Op::Normalize => "Bring the loudest peak of the selection (or everything) to 0 dB",
            Op::Reverse => "Play the selection (or everything) backwards",
            Op::Gain(_) => "Make the selection (or everything) louder or quieter by this many dB",
        }
    }

    /// Whether the edit only makes sense on a selection.
    pub fn needs_selection(self) -> bool {
        matches!(self, Op::Trim | Op::Delete)
    }
}

/// Apply an edit to `sel` (frames, end not included) or to everything. Returns the new
/// audio and the selection after it.
pub fn apply(a: &Audio, op: Op, sel: Option<(usize, usize)>) -> (Audio, Option<(usize, usize)>) {
    let n = a.frames();
    let (s, e) = sel
        .map(|(s, e)| (s.min(n), e.min(n)))
        .filter(|(s, e)| e > s)
        .unwrap_or((0, n));
    let mut out = Audio {
        channels: a.channels.clone(),
        sample_rate: a.sample_rate,
    };
    let len = (e - s).max(1) as f32;
    let keep = sel.map(|_| (s, e));
    match op {
        Op::Trim => {
            for c in &mut out.channels {
                *c = c[s..e].to_vec();
            }
            return (out, None);
        }
        Op::Delete => {
            for c in &mut out.channels {
                c.drain(s..e);
            }
            return (out, None);
        }
        Op::Silence => out.channels.iter_mut().for_each(|c| c[s..e].fill(0.0)),
        Op::FadeIn => out.channels.iter_mut().for_each(|c| {
            for (i, v) in c[s..e].iter_mut().enumerate() {
                *v *= i as f32 / len;
            }
        }),
        Op::FadeOut => out.channels.iter_mut().for_each(|c| {
            for (i, v) in c[s..e].iter_mut().enumerate() {
                *v *= 1.0 - (i + 1) as f32 / len;
            }
        }),
        Op::Normalize => {
            let peak = out
                .channels
                .iter()
                .flat_map(|c| c[s..e].iter())
                .fold(0f32, |m, v| m.max(v.abs()));
            if peak > 0.0 {
                let g = 1.0 / peak;
                out.channels
                    .iter_mut()
                    .for_each(|c| c[s..e].iter_mut().for_each(|v| *v *= g));
            }
        }
        Op::Reverse => out.channels.iter_mut().for_each(|c| c[s..e].reverse()),
        Op::Gain(db) => {
            let g = 10f32.powf(db / 20.0);
            out.channels
                .iter_mut()
                .for_each(|c| c[s..e].iter_mut().for_each(|v| *v *= g));
        }
    }
    (out, keep)
}

/// The outline of some audio: the lowest and highest sample of each block of frames,
/// per channel, made once so drawing at any zoom is quick.
pub struct Peaks {
    pub block: usize,
    pub channels: Vec<Vec<(f32, f32)>>,
}

impl Peaks {
    pub const BLOCK: usize = 64;

    pub fn of(a: &Audio) -> Self {
        let channels = a
            .channels
            .iter()
            .map(|c| {
                c.chunks(Self::BLOCK)
                    .map(|b| {
                        b.iter()
                            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)))
                    })
                    .collect()
            })
            .collect();
        Self {
            block: Self::BLOCK,
            channels,
        }
    }

    /// The lowest and highest sample of channel `ch` in frames `from..to`.
    pub fn span(&self, a: &Audio, ch: usize, from: usize, to: usize) -> (f32, f32) {
        let Some(c) = a.channels.get(ch) else {
            return (0.0, 0.0);
        };
        let to = to.min(c.len());
        if from >= to {
            return (0.0, 0.0);
        }
        let fold = |it: &mut dyn Iterator<Item = (f32, f32)>| {
            it.fold((f32::MAX, f32::MIN), |(lo, hi), (a, b)| {
                (lo.min(a), hi.max(b))
            })
        };
        if to - from < self.block * 2 {
            return fold(&mut c[from..to].iter().map(|v| (*v, *v)));
        }
        let blocks = &self.channels[ch];
        let (a, z) = (from / self.block, to.div_ceil(self.block).min(blocks.len()));
        fold(&mut blocks[a..z].iter().copied())
    }
}

/// Every channel of some audio mixed to one.
pub fn mono(a: &Audio) -> Vec<f32> {
    let n = a.channels.len().max(1) as f32;
    (0..a.frames())
        .map(|i| a.channels.iter().map(|c| c[i]).sum::<f32>() / n)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    /// Selecting from this frame.
    Select(usize),
    /// Moving this marker.
    Marker(usize),
}

/// The open sample editor.
pub struct SampleEditor {
    /// The channel whose sound is edited (by id).
    pub channel: String,
    /// The sample the channel plays now.
    pub sample: String,
    pub name: String,
    pub audio: Arc<Audio>,
    peaks: Peaks,
    /// Earlier and later versions of the sound: sample id and audio.
    undo: Vec<(String, Arc<Audio>)>,
    redo: Vec<(String, Arc<Audio>)>,
    /// The first frame shown and the frames per point.
    pub start: f64,
    pub fpp: f64,
    /// Show the whole sound when next drawn.
    fit: bool,
    pub sel: Option<(usize, usize)>,
    drag: Drag,
    /// Where slices after the first start, frames, ascending.
    pub markers: Vec<usize>,
    pub sensitivity: f32,
    /// The loop's tempo as detected, and as used (it can be typed in).
    pub detected: Option<f64>,
    pub tempo: f64,
    pub gain_db: f32,
    /// The slicer channel the markers cut (by id).
    pub slicer: Option<String>,
    pub status: String,
}

impl SampleEditor {
    pub fn new(channel: &str, sample: &str, name: &str, audio: Arc<Audio>, bpm: f64) -> Self {
        Self {
            channel: channel.to_string(),
            sample: sample.to_string(),
            name: name.to_string(),
            peaks: Peaks::of(&audio),
            audio,
            undo: Vec::new(),
            redo: Vec::new(),
            start: 0.0,
            fpp: 64.0,
            fit: true,
            sel: None,
            drag: Drag::None,
            markers: Vec::new(),
            sensitivity: 0.5,
            detected: None,
            tempo: bpm,
            gain_db: 3.0,
            slicer: None,
            status: String::new(),
        }
    }

    /// Show another version of the sound.
    pub fn set_audio(&mut self, sample: &str, audio: Arc<Audio>) {
        self.sample = sample.to_string();
        self.peaks = Peaks::of(&audio);
        let n = audio.frames();
        self.markers.retain(|&m| m > 0 && m < n);
        self.sel = self
            .sel
            .map(|(s, e)| (s.min(n), e.min(n)))
            .filter(|(s, e)| e > s);
        if self.start >= n as f64 {
            self.start = 0.0;
        }
        self.audio = audio;
    }

    pub fn seconds(&self) -> f64 {
        self.audio.frames() as f64 / self.audio.sample_rate.max(1.0)
    }

    /// Find the beats (with the editor's sensitivity) and the loop's tempo.
    pub fn detect(&mut self) {
        let m = mono(&self.audio);
        self.markers = las_dsp::onset::onsets(&m, self.audio.sample_rate, self.sensitivity);
        self.detect_tempo();
    }

    /// The loop's tempo from its beats.
    pub fn detect_tempo(&mut self) {
        let onsets = if self.markers.is_empty() {
            las_dsp::onset::onsets(&mono(&self.audio), self.audio.sample_rate, self.sensitivity)
        } else {
            self.markers.clone()
        };
        self.detected =
            las_dsp::onset::detect_tempo(&onsets, self.audio.sample_rate, self.audio.frames());
        if let Some(t) = self.detected {
            self.tempo = t;
        }
    }

    /// How the markers cut the sound.
    pub fn spec(&self) -> SliceSpec {
        SliceSpec {
            starts: self.markers.clone(),
            frames: self.audio.frames(),
            sample_rate: self.audio.sample_rate,
            bpm: self.tempo,
        }
    }

    /// Zoom by `factor` (above 1 zooms in) keeping the frame at `about` where it is.
    pub fn zoom(&mut self, factor: f64, about: f64, width: f32) {
        let x = (about - self.start) / self.fpp;
        let most = (self.audio.frames() as f64 / f64::from(width.max(1.0))).max(0.125);
        self.fpp = (self.fpp / factor).clamp(0.125, most);
        self.start = about - x * self.fpp;
        self.clamp_view(width);
    }

    fn clamp_view(&mut self, width: f32) {
        let shown = self.fpp * f64::from(width);
        let n = self.audio.frames() as f64;
        self.start = self.start.clamp(0.0, (n - shown).max(0.0));
    }

    /// Show the whole sound in `width` points.
    pub fn show_all(&mut self, width: f32) {
        self.fpp = (self.audio.frames() as f64 / f64::from(width.max(1.0))).max(0.125);
        self.start = 0.0;
    }

    fn sorted(&mut self) {
        let n = self.audio.frames();
        self.markers.retain(|&m| m > 0 && m < n);
        self.markers.sort_unstable();
        self.markers.dedup();
    }
}

/// The audio a sample id names in the open project, at its own rate.
pub fn load_audio(ps: &ProjectState, sample: &str) -> Option<Audio> {
    let l = ps.loaded.as_ref()?;
    if las_engine::factory::is_factory_id(sample) {
        return las_engine::factory::render_factory(sample, 44_100.0);
    }
    las_engine::library::decode_audio(&l.samples.get(sample)?.data)
}

/// Open the editor on a channel's sound. `None` when it has none that can be read.
pub fn open(ps: &ProjectState, channel_id: &str) -> Option<SampleEditor> {
    let l = ps.loaded.as_ref()?;
    let c = l.project.channel(channel_id)?;
    let sample = c.sample_id.clone()?;
    let audio = Arc::new(load_audio(ps, &sample)?);
    let mut ed = SampleEditor::new(&c.id, &sample, &c.name, audio, l.project.bpm);
    if c.plugin_id == SLICER_ID {
        ed.markers = slicer::slice_starts(c, ed.audio.frames());
        ed.slicer = Some(c.id.clone());
        if let Some(b) = c.state.get("sliceBpm").and_then(serde_json::Value::as_f64) {
            ed.tempo = b;
        }
    }
    Some(ed)
}

/// Ticks an audio clip of `seconds` lasts at `bpm`.
fn ticks(seconds: f64, bpm: f64) -> f64 {
    seconds * bpm / 60.0 * f64::from(las_project::PPQ)
}

/// Point the editor's channel at another version of its sound (an undo step of the
/// project). Audio clips as long as the old sound follow the new one's length.
fn point_at(ed: &mut SampleEditor, ps: &mut ProjectState, id: &str, audio: Arc<Audio>) {
    let old = ed.seconds();
    let new = audio.frames() as f64 / audio.sample_rate.max(1.0);
    let channel = ed.channel.clone();
    let sid = id.to_string();
    ps.modify(true, move |p| {
        let bpm = p.bpm;
        if let Some(c) = p.channels.iter_mut().find(|c| c.id == channel) {
            c.sample_id = Some(sid);
        }
        for clip in p.playlist.clips.iter_mut().filter(|c| {
            c.kind == las_project::ClipKind::Audio && c.ref_id == channel && c.offset == 0.0
        }) {
            let plain = clip.audio.as_ref().is_none_or(|e| e.stretch.is_none());
            if plain && (clip.length - ticks(old, bpm)).abs() < 1.0 {
                clip.length = ticks(new, bpm).max(1.0);
            }
        }
    });
    ed.set_audio(id, audio);
}

/// Store an edited sound in the project (a new WAV sample; the old one is left as it
/// was) and play it from the editor's channel.
pub fn commit(ed: &mut SampleEditor, ps: &mut ProjectState, new: Audio) {
    let id = las_project::create::uid("smp_");
    let wav = las_engine::library::encode_wav(&new);
    let name = format!("{} (edited)", ed.name);
    let secs = new.frames() as f64 / new.sample_rate.max(1.0);
    let sid = id.clone();
    ps.modify_with_samples(false, move |p, samples| {
        samples.insert(
            sid.clone(),
            las_project::Sample {
                name: name.clone(),
                mime: "audio/wav".into(),
                data: wav,
            },
        );
        p.samples.push(las_project::SampleRef {
            id: sid,
            name,
            duration: Some(secs),
            ..Default::default()
        });
    });
    ed.undo.push((ed.sample.clone(), ed.audio.clone()));
    ed.redo.clear();
    point_at(ed, ps, &id, Arc::new(new));
}

/// Make an edit and store it.
pub fn edit(ed: &mut SampleEditor, ps: &mut ProjectState, op: Op) {
    if op.needs_selection() && ed.sel.is_none() {
        ed.status = "Select part of the sound first (drag across the waveform).".into();
        return;
    }
    let (out, sel) = apply(&ed.audio, op, ed.sel);
    if out.frames() == 0 {
        ed.status = "That would leave no sound.".into();
        return;
    }
    commit(ed, ps, out);
    ed.sel = sel;
    if matches!(op, Op::Trim | Op::Delete) {
        ed.markers.clear();
        ed.fit = true;
    }
    ed.status = format!("{} done.", op.label());
    reslice(ed, ps);
}

/// Go back to the version of the sound before the last edit.
pub fn undo(ed: &mut SampleEditor, ps: &mut ProjectState) {
    if let Some((id, a)) = ed.undo.pop() {
        ed.redo.push((ed.sample.clone(), ed.audio.clone()));
        point_at(ed, ps, &id, a);
        ed.fit = true;
    }
}

pub fn redo(ed: &mut SampleEditor, ps: &mut ProjectState) {
    if let Some((id, a)) = ed.redo.pop() {
        ed.undo.push((ed.sample.clone(), ed.audio.clone()));
        point_at(ed, ps, &id, a);
        ed.fit = true;
    }
}

/// Make a slicer channel from the editor's sound and markers. Returns its pattern's
/// index.
pub fn make_slicer(ed: &mut SampleEditor, ps: &mut ProjectState) -> Option<usize> {
    let spec = ed.spec();
    let (sample, name) = (ed.sample.clone(), ed.name.clone());
    let mut made = None;
    ps.modify(true, |p| {
        made = Some(slicer::add_slicer(p, &sample, &name, &spec));
    });
    let (ch, pat) = made?;
    let l = ps.loaded.as_ref()?;
    let c = &l.project.channels[ch];
    ed.slicer = Some(c.id.clone());
    ed.status = format!(
        "Made \"{}\" with {} slices (C5 up) and the pattern \"{}\" that plays them at {:.2} BPM.",
        c.name,
        spec.fractions().len(),
        l.project.patterns[pat].name,
        spec.bpm
    );
    Some(pat)
}

/// Cut the linked slicer channel again at the markers.
pub fn reslice(ed: &SampleEditor, ps: &mut ProjectState) {
    let Some(id) = ed.slicer.clone() else {
        return;
    };
    let spec = ed.spec();
    let Some(i) = ps
        .loaded
        .as_ref()
        .and_then(|l| l.project.channels.iter().position(|c| c.id == id))
    else {
        return;
    };
    ps.modify_coalesced("slices", |p| slicer::reslice(p, i, &spec));
}

/// What the editor's panel asks for.
#[derive(Default)]
pub struct Out {
    pub op: Option<Op>,
    pub undo: bool,
    pub redo: bool,
    pub play: bool,
    pub stop: bool,
    pub save: bool,
    pub slice: bool,
    /// The markers changed.
    pub markers: bool,
}

/// The waveform colour.
fn wave_colour() -> Color32 {
    crate::theme::accent()
}

/// Draw the editor: the buttons, the waveform and the markers.
pub fn panel(ui: &mut Ui, ed: &mut SampleEditor, linked: Option<&str>) -> Out {
    let mut out = Out::default();
    let has_sel = ed.sel.is_some();
    ui.horizontal_wrapped(|ui| {
        for op in [
            Op::Trim,
            Op::Delete,
            Op::Silence,
            Op::FadeIn,
            Op::FadeOut,
            Op::Normalize,
            Op::Reverse,
        ] {
            let ok = has_sel || !op.needs_selection();
            let tip = if ok {
                op.tip()
            } else {
                "Select part of the sound first: drag across the waveform"
            };
            if ui
                .add_enabled(ok, egui::Button::new(op.label()))
                .on_hover_text(tip)
                .on_disabled_hover_text(tip)
                .clicked()
            {
                out.op = Some(op);
            }
        }
        ui.separator();
        ui.label("Gain");
        ui.add(
            egui::DragValue::new(&mut ed.gain_db)
                .range(-24.0..=24.0)
                .speed(0.1)
                .suffix(" dB"),
        );
        if ui
            .button(Op::Gain(0.0).label())
            .on_hover_text(Op::Gain(0.0).tip())
            .clicked()
        {
            out.op = Some(Op::Gain(ed.gain_db));
        }
        ui.separator();
        out.undo = ui
            .add_enabled(!ed.undo.is_empty(), egui::Button::new("Undo"))
            .on_hover_text("Take back the last edit")
            .clicked();
        out.redo = ui
            .add_enabled(!ed.redo.is_empty(), egui::Button::new("Redo"))
            .on_hover_text("Make the edit taken back again")
            .clicked();
        ui.separator();
        let what = if has_sel { "Play selection" } else { "Play" };
        out.play = ui
            .button(what)
            .on_hover_text("Listen to the selection (or the whole sound)")
            .clicked();
        out.stop = ui.button("Stop").on_hover_text("Stop listening").clicked();
        out.save = ui
            .button("Save as WAV...")
            .on_hover_text("Write the edited sound to a WAV file")
            .clicked();
    });
    // what is shown, and the zoom
    let width = ui.available_width().max(100.0);
    ui.horizontal_wrapped(|ui| {
        let rate = ed.audio.sample_rate;
        let chans = match ed.audio.channels.len() {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} channels"),
        };
        ui.label(RichText::new(format!("{:.3} s, {:.0} Hz, {chans}", ed.seconds(), rate)).weak());
        if let Some((s, e)) = ed.sel {
            ui.label(format!(
                "Selection {:.3} to {:.3} s ({:.3} s)",
                s as f64 / rate,
                e as f64 / rate,
                (e - s) as f64 / rate
            ));
        } else {
            ui.label(RichText::new("No selection: edits apply to the whole sound").weak());
        }
        ui.separator();
        let mid = ed.start + ed.fpp * f64::from(width) / 2.0;
        if ui.button("Zoom in").clicked() {
            ed.zoom(2.0, mid, width);
        }
        if ui.button("Zoom out").clicked() {
            ed.zoom(0.5, mid, width);
        }
        if ui
            .button("Show all")
            .on_hover_text("Fit the whole sound in the window")
            .clicked()
        {
            ed.fit = true;
        }
        if let Some((s, e)) = ed.sel {
            if ui.button("Zoom to selection").clicked() {
                ed.fpp = ((e - s) as f64 / f64::from(width)).max(0.125);
                ed.start = s as f64;
            }
        }
    });
    // the slicer
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Slices").strong());
        if ui
            .button("Detect beats")
            .on_hover_text("Put a slice marker at each beat or hit")
            .clicked()
        {
            ed.detect();
            out.markers = true;
        }
        ui.label("Sensitivity");
        if ui
            .add(egui::Slider::new(&mut ed.sensitivity, 0.0..=1.0).show_value(false))
            .on_hover_text("Higher finds quieter hits (more slices)")
            .changed()
        {
            ed.detect();
            out.markers = true;
        }
        if ui
            .add_enabled(!ed.markers.is_empty(), egui::Button::new("Clear markers"))
            .clicked()
        {
            ed.markers.clear();
            out.markers = true;
        }
        ui.label(format!("{} slices", ed.markers.len() + 1));
        ui.separator();
        ui.label("Tempo");
        ui.add(
            egui::DragValue::new(&mut ed.tempo)
                .range(20.0..=400.0)
                .speed(0.1)
                .max_decimals(2)
                .suffix(" BPM"),
        )
        .on_hover_text("The loop's own tempo: it places the slices in the pattern");
        if ui
            .button("Detect tempo")
            .on_hover_text("Work out the loop's tempo from its beats and length")
            .clicked()
        {
            ed.detect_tempo();
            out.markers = true;
        }
        match ed.detected {
            Some(t) => ui.label(RichText::new(format!("detected {t:.2} BPM")).weak()),
            None => ui.label(RichText::new("not detected yet").weak()),
        };
        ui.separator();
        match linked {
            Some(name) => {
                ui.label(RichText::new(format!("Markers cut \"{name}\"")).weak());
            }
            None => {
                out.slice = ui
                    .button("Slice to new channel")
                    .on_hover_text("Make a Slicer channel (slice N on key C5+N) and a pattern that plays the loop")
                    .clicked();
            }
        }
    });
    if !ed.status.is_empty() {
        ui.label(RichText::new(&ed.status).weak().size(11.0));
    }
    out.markers |= waveform(ui, ed);
    out
}

/// The waveform: drag to select, Ctrl+wheel to zoom, wheel to scroll; in the strip
/// above it markers are dragged, added (double click) and deleted (right click).
/// Returns whether the markers changed.
fn waveform(ui: &mut Ui, ed: &mut SampleEditor) -> bool {
    const STRIP: f32 = 18.0;
    const BAR: f32 = 12.0;
    let size = vec2(
        ui.available_width().max(100.0),
        (ui.available_height() - BAR - 4.0).max(120.0),
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let width = rect.width();
    if std::mem::take(&mut ed.fit) {
        ed.show_all(width);
    }
    ed.clamp_view(width);
    let n = ed.audio.frames();
    let frame_at = |ed: &SampleEditor, x: f32| -> usize {
        (ed.start + f64::from(x - rect.left()) * ed.fpp)
            .round()
            .clamp(0.0, n as f64) as usize
    };
    let x_of = |ed: &SampleEditor, f: usize| -> f32 {
        rect.left() + ((f as f64 - ed.start) / ed.fpp) as f32
    };
    let mut changed = false;
    let strip = Rect::from_min_size(rect.min, vec2(width, STRIP));
    // zoom and scroll
    if resp.hovered() {
        let (z, m, scroll, pos) = ui.input(|i| {
            (
                i.zoom_delta(),
                i.modifiers,
                i.smooth_scroll_delta,
                i.pointer.hover_pos(),
            )
        });
        if m.command && (z - 1.0).abs() > 1e-4 {
            if let Some(p) = pos {
                let about = ed.start + f64::from(p.x - rect.left()) * ed.fpp;
                ed.zoom(f64::from(z), about, width);
            }
        } else {
            let d = if scroll.x.abs() > scroll.y.abs() {
                scroll.x
            } else {
                scroll.y
            };
            if d != 0.0 {
                ed.start -= f64::from(d) * ed.fpp;
                ed.clamp_view(width);
            }
        }
    }
    let near_marker = |ed: &SampleEditor, x: f32| {
        ed.markers
            .iter()
            .enumerate()
            .map(|(i, &m)| (i, (x_of(ed, m) - x).abs()))
            .filter(|(_, d)| *d < 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started() {
        if let Some(p) = pointer {
            ed.drag = match near_marker(ed, p.x).filter(|_| p.y < strip.bottom() + 4.0) {
                Some(i) => Drag::Marker(i),
                None if p.y >= strip.bottom() => {
                    let f = frame_at(ed, p.x);
                    ed.sel = None;
                    Drag::Select(f)
                }
                None => Drag::None,
            };
        }
    }
    if resp.dragged() {
        if let Some(p) = pointer {
            let f = frame_at(ed, p.x);
            match ed.drag {
                Drag::Select(a) => {
                    ed.sel = Some((a.min(f), a.max(f))).filter(|(s, e)| e > s);
                }
                Drag::Marker(i) => {
                    if let Some(m) = ed.markers.get_mut(i) {
                        *m = f.clamp(1, n.saturating_sub(1));
                    }
                }
                Drag::None => {}
            }
        }
    }
    if resp.drag_stopped() {
        if let Drag::Marker(_) = ed.drag {
            ed.sorted();
            changed = true;
        }
        ed.drag = Drag::None;
    }
    if let Some(p) = pointer {
        let in_strip = p.y < strip.bottom();
        if resp.double_clicked() && in_strip {
            ed.markers.push(frame_at(ed, p.x));
            ed.sorted();
            changed = true;
        } else if resp.secondary_clicked() && in_strip {
            if let Some(i) = near_marker(ed, p.x) {
                ed.markers.remove(i);
                changed = true;
            }
        } else if resp.clicked() && !in_strip {
            ed.sel = None;
        }
    }
    resp.on_hover_text(
        "Drag across the waveform to select. Ctrl+wheel zooms, the wheel scrolls.\nMarker strip (top): drag a marker to move it, double click to add one, right click one to delete it.",
    );

    // drawing
    let painter = ui.painter_at(rect);
    let vis = ui.visuals().clone();
    painter.rect_filled(rect, 3.0, vis.extreme_bg_color);
    painter.rect_filled(strip, 0.0, vis.faint_bg_color);
    if let Some((s, e)) = ed.sel {
        let r = Rect::from_x_y_ranges(
            x_of(ed, s).max(rect.left())..=x_of(ed, e).min(rect.right()),
            strip.bottom()..=rect.bottom(),
        );
        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 34));
    }
    let lanes = ed.audio.channels.len().max(1);
    let body = Rect::from_min_max(pos2(rect.left(), strip.bottom()), rect.max);
    let lane_h = body.height() / lanes as f32;
    let col = wave_colour();
    for ch in 0..lanes {
        let top = body.top() + lane_h * ch as f32;
        let mid = top + lane_h / 2.0;
        let half = lane_h / 2.0 - 3.0;
        painter.line_segment(
            [pos2(rect.left(), mid), pos2(rect.right(), mid)],
            Stroke::new(1.0, vis.weak_text_color().gamma_multiply(0.4)),
        );
        if ed.fpp < 1.0 {
            // close in: the samples themselves
            let first = ed.start.floor() as usize;
            let last = ((ed.start + ed.fpp * f64::from(width)).ceil() as usize + 1).min(n);
            let pts: Vec<egui::Pos2> = (first..last)
                .map(|i| {
                    let v = ed.audio.channels[ch][i];
                    pos2(x_of(ed, i), mid - v.clamp(-1.0, 1.0) * half)
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.2, col)));
        } else {
            for x in 0..width as usize {
                let a = ed.start + x as f64 * ed.fpp;
                if a >= n as f64 {
                    break;
                }
                let (lo, hi) =
                    ed.peaks
                        .span(&ed.audio, ch, a as usize, (a + ed.fpp).ceil() as usize);
                let px = rect.left() + x as f32 + 0.5;
                painter.line_segment(
                    [
                        pos2(px, mid - hi.clamp(-1.0, 1.0) * half),
                        pos2(px, mid - lo.clamp(-1.0, 1.0) * half + 0.5),
                    ],
                    Stroke::new(1.0, col),
                );
            }
        }
    }
    // the markers, numbered by the key that plays them
    let marker_col = Color32::from_rgb(255, 196, 64);
    for (i, &m) in std::iter::once(&0).chain(ed.markers.iter()).enumerate() {
        let x = x_of(ed, m);
        if x < rect.left() - 1.0 || x > rect.right() + 1.0 {
            continue;
        }
        painter.line_segment(
            [pos2(x, strip.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, marker_col.gamma_multiply(0.7)),
        );
        let flag = Rect::from_min_size(pos2(x, strip.top() + 1.0), vec2(22.0, STRIP - 2.0));
        painter.rect_filled(flag, 2.0, marker_col);
        painter.text(
            flag.center(),
            Align2::CENTER_CENTER,
            crate::widgets::note_name((60 + i).min(127) as u8),
            FontId::proportional(9.5),
            Color32::BLACK,
        );
    }
    // the time ruler in the strip
    let secs = ed.fpp * f64::from(width) / ed.audio.sample_rate.max(1.0);
    let step = [
        0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0,
    ]
    .into_iter()
    .find(|s| secs / s <= 12.0)
    .unwrap_or(60.0);
    let t0 = ed.start / ed.audio.sample_rate;
    let mut t = (t0 / step).ceil() * step;
    while t < t0 + secs {
        let x = x_of(ed, (t * ed.audio.sample_rate) as usize);
        painter.text(
            pos2(x + 2.0, rect.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            format!("{t:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
                + " s",
            FontId::proportional(9.5),
            vis.weak_text_color(),
        );
        t += step;
    }
    // the scroll bar
    let (bar, bresp) = ui.allocate_exact_size(vec2(width, BAR), Sense::click_and_drag());
    let total = n.max(1) as f64;
    let shown = (ed.fpp * f64::from(width) / total).min(1.0) as f32;
    let left = (ed.start / total) as f32;
    let thumb = Rect::from_x_y_ranges(
        bar.left() + left * width..=bar.left() + (left + shown) * width,
        bar.y_range(),
    );
    ui.painter().rect_filled(bar, 3.0, vis.faint_bg_color);
    ui.painter()
        .rect_filled(thumb, 3.0, vis.widgets.inactive.bg_fill);
    if bresp.dragged() {
        ed.start += f64::from(bresp.drag_delta().x / width) * total;
        ed.clamp_view(width);
    } else if let Some(p) = bresp.interact_pointer_pos().filter(|_| bresp.clicked()) {
        ed.start = f64::from((p.x - bar.left()) / width) * total - ed.fpp * f64::from(width) / 2.0;
        ed.clamp_view(width);
    }
    changed
}

/// A small outline of a sound (the audio clip window's).
pub fn wave_strip(ui: &mut Ui, wave: &crate::playlist::Wave, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width().max(100.0), height),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let n = wave.peaks.len();
    if n > 0 {
        let mid = rect.center().y;
        let half = rect.height() / 2.0 - 2.0;
        for x in 0..rect.width() as usize {
            let a = x * n / rect.width() as usize;
            let z = ((x + 1) * n / rect.width() as usize).max(a + 1).min(n);
            let v = wave.peaks[a..z]
                .iter()
                .fold(0f32, |m, v| m.max(*v))
                .min(1.0);
            let px = rect.left() + x as f32 + 0.5;
            painter.line_segment(
                [pos2(px, mid - v * half), pos2(px, mid + v * half + 0.5)],
                Stroke::new(1.0, wave_colour()),
            );
        }
    }
    resp
}

impl crate::app::App {
    /// Open the sample editor on a channel's sound (and slice it, when asked).
    pub(crate) fn open_sample_editor(&mut self, req: &Request) {
        let (Request::Edit(id) | Request::Slice(id)) = req;
        let Some(mut ed) = open(&self.project, id) else {
            self.status = "This channel has no sound the sample editor can open.".into();
            return;
        };
        if let Request::Slice(_) = req {
            ed.detect();
            if let Some(pat) = make_slicer(&mut ed, &mut self.project) {
                self.project.pattern = pat;
            }
        }
        self.sample_editor = Some(ed);
    }

    /// The sample editor's window, while it is open.
    pub(crate) fn sample_editor_window(&mut self, ctx: &egui::Context, area: Rect) {
        for r in take_requests() {
            self.open_sample_editor(&r);
        }
        let Some(mut ed) = self.sample_editor.take() else {
            return;
        };
        // an undo of the project can have changed the sound
        let now = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channel(&ed.channel).map(|c| c.sample_id.clone()));
        match now {
            None => return,
            Some(Some(id)) if id != ed.sample => {
                if let Some(a) = load_audio(&self.project, &id) {
                    ed.set_audio(&id, Arc::new(a));
                    ed.fit = true;
                }
            }
            _ => {}
        }
        let linked = ed.slicer.as_ref().and_then(|id| {
            let l = self.project.loaded.as_ref()?;
            l.project.channel(id).map(|c| c.name.clone())
        });
        let mut keep = true;
        let mut out = Out::default();
        egui::Window::new(format!("Sample editor: {}", ed.name))
            .id(egui::Id::unique("sample-editor"))
            .constrain_to(area)
            .resizable(true)
            .default_pos(area.min + vec2(120.0, 90.0))
            .default_size(vec2((area.width() - 240.0).clamp(600.0, 1400.0), 520.0))
            .open(&mut keep)
            .show(ctx, |ui| {
                out = panel(ui, &mut ed, linked.as_deref());
            });
        if let Some(op) = out.op {
            edit(&mut ed, &mut self.project, op);
        }
        if out.undo {
            undo(&mut ed, &mut self.project);
        }
        if out.redo {
            redo(&mut ed, &mut self.project);
        }
        if out.markers {
            reslice(&ed, &mut self.project);
        }
        if out.slice {
            if let Some(pat) = make_slicer(&mut ed, &mut self.project) {
                self.project.pattern = pat;
            }
        }
        if out.play {
            let n = ed.audio.frames();
            let (s, e) = ed.sel.unwrap_or((0, n));
            let ch = |i: usize| ed.audio.channels.get(i).unwrap_or(&ed.audio.channels[0]);
            let left: Arc<[f32]> = Arc::from(&ch(0)[s..e]);
            let right: Arc<[f32]> = Arc::from(&ch(1)[s..e]);
            self.send(crate::audio::Command::Preview(Box::new(
                crate::audio::Preview {
                    left,
                    right,
                    rate: ed.audio.sample_rate,
                },
            )));
        }
        if out.stop {
            self.send(crate::audio::Command::PreviewStop);
        }
        if out.save {
            let file = rfd::FileDialog::new()
                .add_filter("WAV", &["wav"])
                .set_file_name(format!("{}.wav", ed.name))
                .set_title("Save the edited sound")
                .save_file();
            if let Some(path) = file {
                let bytes = las_engine::library::encode_wav(&ed.audio);
                ed.status = match std::fs::write(&path, bytes) {
                    Ok(()) => format!("Saved {}", path.display()),
                    Err(e) => format!("{}: {e}", path.display()),
                };
            }
        }
        if keep {
            self.sample_editor = Some(ed);
        }
    }

    /// A slicer channel's window: what it plays, and a way to its markers.
    pub(crate) fn slicer_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            return;
        };
        let slices = c
            .state
            .get("slices")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        let bpm = c.state.get("sliceBpm").and_then(serde_json::Value::as_f64);
        ui.label(RichText::new(&c.name).size(16.0).strong());
        ui.label(format!(
            "Slicer: {slices} slices, on keys {} up{}.",
            crate::widgets::note_name(60),
            bpm.map_or(String::new(), |b| format!(", cut at {b:.2} BPM"))
        ));
        ui.label(
            RichText::new(
                "Each key from C5 plays one slice of the loop. Play the slices pattern to hear the loop at the project tempo, or move its notes in the piano roll to make a new groove.",
            )
            .weak()
            .size(11.0),
        );
        ui.add_space(6.0);
        if let Some(wave) = c
            .sample_id
            .as_ref()
            .and_then(|s| self.project.playlist.waves.get(s))
            .cloned()
        {
            wave_strip(ui, &wave, 70.0);
        }
        if ui
            .button("Edit slices in sample editor")
            .on_hover_text("Move, add or delete slice markers and detect the tempo")
            .clicked()
        {
            request(Request::Edit(c.id.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Audio {
        Audio {
            channels: vec![(0..n).map(|i| i as f32 / n as f32).collect()],
            sample_rate: 1000.0,
        }
    }

    fn state_with(a: &Audio) -> (ProjectState, String) {
        let mut ps = ProjectState::default();
        let mut p = las_project::create::empty_project("x");
        p.bpm = 120.0;
        let mut samples = std::collections::BTreeMap::new();
        let ch = crate::menus::import_audio_clip(
            &mut p,
            &mut samples,
            "Loop",
            las_engine::library::encode_wav(a),
            0.0,
            0,
        )
        .unwrap();
        let id = p.channels[ch].id.clone();
        ps.install(p, samples, "", Vec::new());
        (ps, id)
    }

    #[test]
    fn edits_change_the_selection_or_everything() {
        let a = ramp(100);
        let (t, sel) = apply(&a, Op::Trim, Some((10, 30)));
        assert_eq!((t.frames(), sel), (20, None));
        assert_eq!(t.channels[0][0], 0.1);
        let (d, _) = apply(&a, Op::Delete, Some((10, 30)));
        assert_eq!(d.frames(), 80);
        assert_eq!(d.channels[0][10], 0.3);
        let (s, sel) = apply(&a, Op::Silence, Some((10, 30)));
        assert_eq!(sel, Some((10, 30)));
        assert!(s.channels[0][10..30].iter().all(|v| *v == 0.0));
        assert_eq!(s.channels[0][30], 0.3);
        let (f, _) = apply(&a, Op::FadeIn, None);
        assert_eq!(f.channels[0][0], 0.0);
        let (f, _) = apply(&a, Op::FadeOut, None);
        assert_eq!(f.channels[0][99], 0.0);
        let (n, _) = apply(&a, Op::Normalize, Some((0, 50)));
        let peak = n.channels[0][..50].iter().fold(0f32, |m, v| m.max(*v));
        assert!((peak - 1.0).abs() < 1e-6);
        let (r, _) = apply(&a, Op::Reverse, None);
        assert_eq!(r.channels[0][0], a.channels[0][99]);
        let (g, _) = apply(&a, Op::Gain(-6.0206), None);
        assert!((g.channels[0][50] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn peaks_give_the_range_at_any_zoom() {
        let a = Audio {
            channels: vec![(0..10_000)
                .map(|i| ((i as f32) * 0.01).sin() * if i > 5000 { 1.0 } else { 0.5 })
                .collect()],
            sample_rate: 44_100.0,
        };
        let p = Peaks::of(&a);
        let (lo, hi) = p.span(&a, 0, 0, 5000);
        assert!(hi <= 0.5 + 1e-6 && hi > 0.45 && lo < -0.45);
        let (_, hi) = p.span(&a, 0, 4000, 10_000);
        assert!(hi > 0.99);
        // close in it reads the samples
        let (lo, hi) = p.span(&a, 0, 100, 101);
        assert_eq!(lo, hi);
        assert_eq!(p.span(&a, 0, 20_000, 30_000), (0.0, 0.0));
    }

    #[test]
    fn an_edit_is_stored_as_a_new_sample_and_undo_brings_the_old_back() {
        let a = Audio {
            channels: vec![vec![0.5; 2000]],
            sample_rate: 1000.0,
        };
        let (mut ps, ch) = state_with(&a);
        let mut ed = open(&ps, &ch).unwrap();
        let first = ed.sample.clone();
        let old_bytes = ps.loaded.as_ref().unwrap().samples[&first].data.clone();
        ed.sel = Some((0, 1000));
        edit(&mut ed, &mut ps, Op::Trim);
        let l = ps.loaded.as_ref().unwrap();
        let now = l.project.channel(&ch).unwrap().sample_id.clone().unwrap();
        assert_ne!(now, first, "the channel plays a new sample");
        assert_eq!(
            l.samples[&first].data, old_bytes,
            "the original is untouched"
        );
        // the audio clip follows the new length: one second at 120 BPM is two beats
        let clip = l
            .project
            .playlist
            .clips
            .iter()
            .find(|c| c.ref_id == ch)
            .unwrap();
        assert!((clip.length - 192.0).abs() < 1e-6, "{}", clip.length);
        // what plays is the new audio (the decode cache is keyed by the bytes)
        let mut lib = las_engine::library::Library::new(1000.0, &l.samples);
        assert_eq!(lib.get(&now).unwrap().frames(), 1000);
        assert_eq!(lib.get(&first).unwrap().frames(), 2000);
        undo(&mut ed, &mut ps);
        let l = ps.loaded.as_ref().unwrap();
        assert_eq!(
            l.project.channel(&ch).unwrap().sample_id.as_deref(),
            Some(first.as_str())
        );
        assert_eq!(ed.audio.frames(), 2000);
        redo(&mut ed, &mut ps);
        assert_eq!(ed.audio.frames(), 1000);
        // the project's own undo goes back too
        ps.undo();
        let l = ps.loaded.as_ref().unwrap();
        assert_eq!(
            l.project.channel(&ch).unwrap().sample_id.as_deref(),
            Some(first.as_str())
        );
    }

    #[test]
    fn slicing_makes_a_slicer_channel_and_its_pattern_and_markers_recut_it() {
        // a hit every half second for four seconds (120 BPM)
        let sr = 22_050.0;
        let x: Vec<f32> = (0..(sr * 4.0) as usize)
            .map(|i| {
                let t = (i as f64 / sr) % 0.5;
                ((t * 200.0 * std::f64::consts::TAU).sin() * (-t * 12.0).exp() * 0.8) as f32
            })
            .collect();
        let a = Audio {
            channels: vec![x],
            sample_rate: sr,
        };
        let (mut ps, ch) = state_with(&a);
        request(Request::Slice(ch.clone()));
        let reqs = take_requests();
        assert_eq!(reqs, vec![Request::Slice(ch.clone())]);
        let mut ed = open(&ps, &ch).unwrap();
        ed.detect();
        assert_eq!(ed.markers.len(), 7);
        assert_eq!(ed.detected, Some(120.0));
        let pat = make_slicer(&mut ed, &mut ps).unwrap();
        let l = ps.loaded.as_ref().unwrap();
        let s = l.project.channels.last().unwrap().clone();
        assert_eq!(s.plugin_id, SLICER_ID);
        assert_eq!(ed.slicer.as_deref(), Some(s.id.as_str()));
        let notes = &l.project.patterns[pat].notes[&s.id];
        assert_eq!(notes.len(), 8);
        assert!((notes[3].pos - 3.0 * 96.0).abs() < 2.0);
        // deleting a marker cuts the slicer again
        ed.markers.remove(0);
        reslice(&ed, &mut ps);
        let l = ps.loaded.as_ref().unwrap();
        assert_eq!(l.project.patterns[pat].notes[&s.id].len(), 7);
        // and the editor opened on the slicer shows its markers
        let again = open(&ps, &s.id).unwrap();
        assert_eq!(again.markers.len(), 6);
        assert_eq!(again.slicer.as_deref(), Some(s.id.as_str()));
    }

    #[test]
    fn the_editor_draws_and_a_drag_selects() {
        let a = ramp(44_100);
        let mut ed = SampleEditor::new("c", "s", "Ramp", Arc::new(a), 120.0);
        let ctx = egui::Context::default();
        let frame = |ctx: &egui::Context, ed: &mut SampleEditor, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0))),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(ctx, input, |ui| {
                panel(ui, ed, None);
            });
        };
        frame(&ctx, &mut ed, Vec::new());
        assert!(ed.fpp > 40.0, "the whole sound fits: {}", ed.fpp);
        let press = |x: f32, down: bool| egui::Event::PointerButton {
            pos: pos2(x, 400.0),
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: Default::default(),
        };
        frame(
            &ctx,
            &mut ed,
            vec![
                egui::Event::PointerMoved(pos2(200.0, 400.0)),
                press(200.0, true),
            ],
        );
        frame(
            &ctx,
            &mut ed,
            vec![egui::Event::PointerMoved(pos2(260.0, 400.0))],
        );
        frame(
            &ctx,
            &mut ed,
            vec![egui::Event::PointerMoved(pos2(400.0, 400.0))],
        );
        frame(&ctx, &mut ed, vec![press(400.0, false)]);
        let (s, e) = ed.sel.expect("a selection");
        assert!(e > s + 5000, "{s}..{e}");
    }
}
