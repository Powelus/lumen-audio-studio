// SPDX-License-Identifier: GPL-3.0-or-later
//! The playlist: the song laid out in tracks of clips (patterns, audio and
//! automation). Like the piano roll, drawing only reports what the user asked for
//! ([`PlEdit`]) and [`apply`] changes the project.

use crate::rack::channel_colour;
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, song_length, ticks_per_bar};
use las_project::create::uid;
use las_project::{AudioClipEdit, Clip, ClipKind, Project, PPQ};
use std::collections::HashSet;

const RULER: f32 = 24.0;
const HEAD: f32 = 110.0;
const EDGE: f32 = 7.0;
/// The height of a clip's title bar, where a right click opens the clip's menu.
const TITLE: f32 = 16.0;
/// Rows shown when the project has fewer tracks.
const MIN_TRACKS: usize = 6;

/// A change the user asked for.
#[derive(Clone, Debug)]
pub enum PlEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    Add(Box<Clip>),
    Move {
        id: String,
        start: f64,
        track: usize,
    },
    Resize {
        id: String,
        length: f64,
    },
    Delete {
        id: String,
    },
    /// Switch a clip's mute on or off.
    Mute {
        id: String,
    },
    /// Cut a clip in two at a tick.
    Split {
        id: String,
        at: f64,
    },
    /// Switch a track's mute on or off.
    TrackMute(usize),
    /// Play a stretch of the song (ticks) over and over, or the whole song again.
    SongLoop(Option<(f64, f64)>),
    /// Give the pattern or automation clip that a clip plays a colour ("#rrggbb").
    Colour {
        clip: String,
        color: String,
    },
    /// Give pattern clips (by id) each its own copy of their pattern, so editing one
    /// leaves the others.
    MakeUnique(Vec<String>),
    /// Turn the song loop on or off, keeping its range.
    LoopOn(bool),
    /// Move a marker (by id) to a tick.
    MoveMarker {
        id: String,
        tick: f64,
    },
    /// Ask for a new name for a pattern (by id).
    AskRenamePattern(String),
    /// Replace a pattern clip with one clip per channel of its pattern, each playing a
    /// new pattern of that channel's notes, stacked on the tracks below (FL's "Split by
    /// channel").
    SplitByChannel(String),
    /// Put a copy of a pattern (by id) after it.
    ClonePattern(String),
    /// Remove a pattern (by id) and its clips; the last pattern stays.
    DeletePattern(String),
    /// Put a song marker at a tick.
    AddMarker(f64),
    DeleteMarker(String),
    /// Ask for a new name for a marker (by id).
    AskRenameMarker(String),
    /// Ask for a new name for a track.
    AskRenameTrack(usize),
    /// Open the editor of an automation clip (by the clip's own id).
    OpenAuto(String),
    /// Open a pattern in the piano roll (by the pattern's id).
    OpenPattern(String),
    /// Move several clips together in time and between tracks.
    MoveMany {
        ids: Vec<String>,
        dstart: f64,
        dtrack: i32,
    },
    DeleteMany {
        ids: Vec<String>,
    },
    /// Add clips that already have their places and new ids.
    Paste {
        clips: Vec<Clip>,
    },
    /// Start playing from here.
    Seek(f64),
    /// Replace the edits of an audio clip (fades, gain, reverse, pitch, stretch).
    AudioEdit {
        id: String,
        edit: Box<AudioClipEdit>,
    },
    /// Move a clip's left edge to `start`, keeping the rest of it where it is in time
    /// (the clip then starts further into its pattern or sound).
    Trim {
        id: String,
        start: f64,
    },
    /// Switch a track's solo on or off.
    TrackSolo(usize),
    /// Give a track a colour ("#rrggbb").
    TrackColour(usize, String),
    /// Put a new empty track before this one; the clips below move down.
    InsertTrack(usize),
    /// Remove a track and its clips; the clips below move up.
    DeleteTrack(usize),
    /// Make a pattern (by id) the current one, as a click in the picker does.
    ChoosePattern(String),
}

/// What the draw and paint tools place: the current pattern, an audio clip's sound or an
/// automation clip. The picker on the left chooses it, as in FL Studio.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// The pattern chosen in the pattern list.
    #[default]
    Pattern,
    /// An audio clip channel, by id.
    Audio(String),
    /// An automation clip, by id.
    Automation(String),
}

/// A picker item being dragged into the playlist.
#[derive(Clone, Debug, PartialEq)]
pub struct PickDrag {
    pub source: Source,
    /// The pattern (by id) when the source is a pattern.
    pub pattern: Option<String>,
    pub name: String,
}

/// The part of a clip under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Body,
    /// The left edge: a drag trims the start.
    Left,
    /// The right edge: a drag sets the length.
    Right,
}

/// What a click on the playlist does, as in FL Studio's tool row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    /// Place pattern clips, move and resize clips.
    #[default]
    Draw,
    /// Move and resize clips without placing new ones.
    Select,
    /// Cut the clip that is clicked at the grid line.
    Slice,
    /// Switch a clip's mute on or off.
    Mute,
    /// Delete the clip that is clicked.
    Delete,
    /// FL's brush: drag along a track to place one clip after another.
    Paint,
}

#[derive(Clone, Debug)]
enum Drag {
    Move {
        id: String,
        grab: f64,
    },
    Resize {
        id: String,
    },
    /// The left edge of a clip.
    Trim {
        id: String,
    },
    /// The fade in (or out) handle of an audio clip.
    Fade {
        id: String,
        out: bool,
    },
    /// The paint tool placing clips.
    Paint,
    /// The selected clips together.
    Group,
    /// A song marker along the ruler.
    Marker {
        id: String,
    },
    /// The song loop, dragged out on the ruler from a tick.
    LoopRange {
        from: f64,
    },
    /// A rectangle that selects what it touches.
    Marquee,
}

pub struct PlaylistState {
    /// Where the last right click landed (tick) and the marker under it, for its menu.
    pub menu_at: Option<(f64, Option<String>)>,
    /// The clip under the last right click, for its colour menu.
    pub menu_clip: Option<String>,
    /// The loop range of the song (ticks), if one is set; it plays over and over while
    /// `loop_on`.
    pub song_loop: Option<(f64, f64)>,
    pub loop_on: bool,
    /// Points per tick.
    pub zoom: f32,
    pub row: f32,
    /// Snap in ticks.
    pub snap: f64,
    pub tool: Tool,
    /// The clips selected (by id) and the ones copied (starts relative to the first).
    pub selected: HashSet<String>,
    pub clipboard: Vec<Clip>,
    /// The waveforms of the samples audio clips play, by sample id.
    pub waves: std::collections::HashMap<String, std::sync::Arc<Wave>>,
    /// Where the clips were drawn last (to place files dropped on the playlist).
    pub area: Option<Area>,
    marquee: Option<(egui::Pos2, egui::Pos2)>,
    group_last: Option<(f64, usize)>,
    drag: Option<Drag>,
    /// The clip the last click placed (a double click on it does not open it).
    just_added: Option<String>,
    /// What the draw and paint tools place.
    pub pick: Source,
    /// Show the picker (patterns, audio and automation clips) on the left.
    pub show_picker: bool,
    /// Where the paint tool places its next clip, and on which track.
    paint_next: Option<(f64, usize)>,
    scroll_x: f32,
    want_x: Option<f32>,
}

/// A sample's outline: the loudest level in each stretch of `step` seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Wave {
    pub peaks: Vec<f32>,
    pub step: f64,
}

impl Wave {
    /// The outline of some audio (any number of channels).
    pub fn of(channels: &[Vec<f32>], sample_rate: f64) -> Self {
        const FRAMES: usize = 256;
        let n = channels.iter().map(Vec::len).max().unwrap_or(0);
        let peaks = (0..n.div_ceil(FRAMES))
            .map(|b| {
                let (a, z) = (b * FRAMES, ((b + 1) * FRAMES).min(n));
                channels
                    .iter()
                    .flat_map(|c| c.get(a..z.min(c.len())).unwrap_or(&[]))
                    .fold(0f32, |m, v| m.max(v.abs()))
            })
            .collect();
        Self {
            peaks,
            step: FRAMES as f64 / sample_rate.max(1.0),
        }
    }

    /// The level at `t` seconds into the sample.
    pub fn at(&self, t: f64) -> f32 {
        if t < 0.0 {
            return 0.0;
        }
        self.peaks
            .get((t / self.step) as usize)
            .copied()
            .unwrap_or(0.0)
    }
}

/// The geometry of the clip area as last drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub rect: Rect,
    pub left: f32,
    pub top: f32,
    pub zoom: f32,
    pub row: f32,
    pub rows: usize,
    pub snap: f64,
}

impl Area {
    /// The tick (on the snap) and the track a point of the screen falls on.
    pub fn place(&self, pos: egui::Pos2) -> Option<(f64, usize)> {
        if !self.rect.contains(pos) || pos.x < self.left || pos.y < self.top {
            return None;
        }
        let tick = f64::from((pos.x - self.left) / self.zoom).max(0.0);
        let tick = if self.snap > 1.0 {
            (tick / self.snap).floor() * self.snap
        } else {
            tick
        };
        let track = (((pos.y - self.top) / self.row) as usize).min(self.rows.saturating_sub(1));
        Some((tick, track))
    }
}

/// Where the files of one drop land: from `first` (the tick and track under the
/// pointer), one file per track down. None when the drop is not on the playlist.
pub fn drop_spots(first: Option<(f64, usize)>, n: usize) -> Vec<Option<(f64, usize)>> {
    (0..n)
        .map(|i| first.map(|(tick, track)| (tick, track + i)))
        .collect()
}

/// The first track with no clips, at tick `at`: where a drop with no pointer
/// position goes.
pub fn free_track(p: &las_project::Project, at: f64) -> (f64, usize) {
    let used: HashSet<usize> = p.playlist.clips.iter().map(|c| c.track).collect();
    let track = (0..).find(|t| !used.contains(t)).unwrap_or(0);
    (at, track)
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            zoom: 0.12,
            row: 44.0,
            snap: f64::from(PPQ) * 4.0,
            tool: Tool::Draw,
            selected: HashSet::new(),
            clipboard: Vec::new(),
            waves: Default::default(),
            area: None,
            marquee: None,
            group_last: None,
            drag: None,
            menu_at: None,
            menu_clip: None,
            song_loop: None,
            loop_on: false,
            pick: Source::Pattern,
            just_added: None,
            show_picker: true,
            paint_next: None,
            scroll_x: 0.0,
            want_x: None,
        }
    }
}

impl PlaylistState {
    /// Another project was opened: its loop, selection, waveforms and what was being
    /// done go; the way the playlist is set up (zoom, snap, tool, the clipboard) stays.
    pub fn forget_project(&mut self) {
        let old = std::mem::take(self);
        *self = Self {
            zoom: old.zoom,
            row: old.row,
            snap: old.snap,
            tool: old.tool,
            clipboard: old.clipboard,
            show_picker: old.show_picker,
            ..Self::default()
        };
    }

    /// The stretch the song loops over now, if any.
    pub fn active_loop(&self) -> Option<(f64, f64)> {
        self.song_loop.filter(|_| self.loop_on)
    }
}

/// The marker before (or after, with `forward`) a tick: where Alt+Left and Alt+Right
/// jump.
pub fn marker_jump(p: &Project, from: f64, forward: bool) -> Option<f64> {
    let marks = p.markers.iter().flatten().map(|m| m.tick);
    if forward {
        marks.filter(|&m| m > from + 0.5).min_by(f64::total_cmp)
    } else {
        marks.filter(|&m| m < from - 0.5).max_by(f64::total_cmp)
    }
}

/// The ids of the clips that play the same thing as the clip `id`: the same pattern,
/// sound or automation clip.
pub fn clips_like(p: &Project, id: &str) -> HashSet<String> {
    let Some(c) = p.playlist.clips.iter().find(|c| c.id == id) else {
        return HashSet::new();
    };
    p.playlist
        .clips
        .iter()
        .filter(|x| x.kind == c.kind && x.ref_id == c.ref_id)
        .map(|x| x.id.clone())
        .collect()
}

/// Split the pattern clip `clip` by channel; see [`PlEdit::SplitByChannel`]. Returns the
/// new clips' ids.
pub fn split_by_channel(project: &mut Project, clip: &str) -> Vec<String> {
    let Some(ci) = project.playlist.clips.iter().position(|c| c.id == clip) else {
        return Vec::new();
    };
    let c = project.playlist.clips[ci].clone();
    if c.kind != ClipKind::Pattern {
        return Vec::new();
    }
    let Some(pi) = project.patterns.iter().position(|x| x.id == c.ref_id) else {
        return Vec::new();
    };
    let pat = project.patterns[pi].clone();
    let length = pattern_length(Some(&pat), project.time_sig);
    // in the order of the channel rack
    let chans: Vec<(String, String, Color32)> = project
        .channels
        .iter()
        .filter(|ch| pat.notes.get(&ch.id).is_some_and(|n| !n.is_empty()))
        .map(|ch| (ch.id.clone(), ch.name.clone(), channel_colour(ch)))
        .collect();
    if chans.len() < 2 {
        return Vec::new();
    }
    let mut ids = Vec::new();
    project.playlist.clips.remove(ci);
    for (k, (ch, name, col)) in chans.into_iter().enumerate() {
        let mut np = pat.clone();
        np.id = uid("pat_");
        np.name = format!("{} - {name}", pat.name);
        np.color = format!("#{:02x}{:02x}{:02x}", col.r(), col.g(), col.b());
        np.length = length;
        np.notes.retain(|id, _| *id == ch);
        for n in np.notes.values_mut().flatten() {
            n.id = uid("n");
        }
        let track = c.track + k;
        // stacked below: a track with a clip in the way makes room with a new track
        let busy = project.playlist.clips.iter().any(|x| {
            x.track == track && x.start < c.start + c.length && x.start + x.length > c.start
        });
        if k > 0 && busy {
            apply(project, &PlEdit::InsertTrack(track));
        }
        let id = uid("c");
        project.playlist.clips.push(Clip {
            id: id.clone(),
            ref_id: np.id.clone(),
            track,
            ..c.clone()
        });
        ids.push(id);
        project.patterns.insert(pi + 1 + k, np);
    }
    ids
}

fn snapped(t: f64, snap: f64) -> f64 {
    if snap <= 1.0 {
        t.round()
    } else {
        (t / snap).round() * snap
    }
}

/// How many rows the playlist shows.
pub fn track_count(p: &Project) -> usize {
    let used = p
        .playlist
        .clips
        .iter()
        .map(|c| c.track + 1)
        .max()
        .unwrap_or(0);
    p.playlist.tracks.len().max(used).max(MIN_TRACKS)
}

/// The clip under a tick and track, and the part of it there (its edges resize it).
fn hit(p: &Project, tick: f64, track: usize, edge_ticks: f64) -> Option<(usize, Part)> {
    p.playlist
        .clips
        .iter()
        .enumerate()
        .rev()
        .find(|(_, c)| c.track == track && tick >= c.start && tick < c.start + c.length.max(1.0))
        .map(|(i, c)| {
            let part = if tick >= c.start + c.length - edge_ticks && c.length > edge_ticks * 1.5 {
                Part::Right
            } else if tick < c.start + edge_ticks && c.length > edge_ticks * 3.0 {
                Part::Left
            } else {
                Part::Body
            };
            (i, part)
        })
}

/// A clip of `source` (the pattern at `pattern` for [`Source::Pattern`]) at `start` on
/// `track`, as long as what it plays.
pub fn source_clip(
    p: &Project,
    source: &Source,
    pattern: usize,
    start: f64,
    track: usize,
) -> Option<Clip> {
    match source {
        Source::Pattern => new_pattern_clip(p, pattern, start, track),
        Source::Audio(id) => {
            let ch = p.channels.iter().find(|c| &c.id == id)?;
            let secs = ch
                .sample_id
                .as_ref()
                .and_then(|s| p.samples.iter().find(|x| &x.id == s))
                .and_then(|s| s.duration);
            let length = match secs {
                Some(s) => s * p.bpm / 60.0 * f64::from(PPQ),
                None => p
                    .playlist
                    .clips
                    .iter()
                    .find(|c| c.kind == ClipKind::Audio && &c.ref_id == id)
                    .map_or(ticks_per_bar(p.time_sig), |c| c.length + c.offset),
            };
            Some(Clip {
                id: uid("c"),
                kind: ClipKind::Audio,
                ref_id: id.clone(),
                track,
                start,
                length: length.max(1.0),
                ..Default::default()
            })
        }
        Source::Automation(id) => {
            let a = p.automation.iter().find(|x| &x.id == id)?;
            Some(Clip {
                id: uid("c"),
                kind: ClipKind::Automation,
                ref_id: id.clone(),
                track,
                start,
                length: a.length.max(1.0),
                ..Default::default()
            })
        }
    }
}

/// The clips the paint tool places as the pointer reaches `tick` on `track`: copies of
/// `template` one after another from `next` (where the next one goes), skipping places
/// where a clip already lies. Returns them and where the one after them would go.
pub fn paint_clips(
    clips: &[Clip],
    template: &Clip,
    next: Option<(f64, usize)>,
    tick: f64,
    track: usize,
    snap: f64,
) -> (Vec<Clip>, (f64, usize)) {
    let len = template.length.max(1.0);
    let g = snap.max(1.0);
    let mut at = match next {
        Some((t, tr)) if tr == track => t,
        _ => (tick.max(0.0) / g).floor() * g,
    };
    let mut out: Vec<Clip> = Vec::new();
    // at most a few hundred clips in one frame, whatever the pointer does
    for _ in 0..512 {
        if at > tick {
            break;
        }
        let taken = clips
            .iter()
            .chain(out.iter())
            .any(|c| c.track == track && at < c.start + c.length - 0.5 && at + len > c.start + 0.5);
        if !taken {
            out.push(Clip {
                id: uid("c"),
                start: at,
                track,
                ..template.clone()
            });
        }
        at += len;
    }
    (out, (at, track))
}

/// Copies of the clips picked by `ids` (every clip when it is empty), placed right after
/// the last of them ends, with new ids: FL's Ctrl+B.
pub fn duplicate_clips(clips: &[Clip], ids: &HashSet<String>) -> Vec<Clip> {
    let sel: Vec<&Clip> = clips
        .iter()
        .filter(|c| ids.is_empty() || ids.contains(&c.id))
        .collect();
    if sel.is_empty() {
        return Vec::new();
    }
    let first = sel.iter().map(|c| c.start).fold(f64::MAX, f64::min);
    let end = sel.iter().map(|c| c.start + c.length).fold(0.0, f64::max);
    sel.iter()
        .map(|c| Clip {
            id: uid("c"),
            start: c.start + (end - first),
            ..(*c).clone()
        })
        .collect()
}

/// Make sure the project lists tracks up to `t`.
fn ensure_track(project: &mut Project, t: usize) {
    while project.playlist.tracks.len() <= t {
        let n = project.playlist.tracks.len();
        project
            .playlist
            .tracks
            .push(las_project::create::playlist_track(n));
    }
}

/// Where the fade handles of an audio clip are: the top corners, moved in by the fades.
/// `x_of` turns ticks into x; returns the (fade in, fade out) handle centres' x.
fn fade_handles(c: &Clip, x_of: impl Fn(f64) -> f32) -> (f32, f32) {
    let e = c.audio.as_ref();
    let fi = e
        .and_then(|a| a.fade_in)
        .unwrap_or(0.0)
        .clamp(0.0, c.length);
    let fo = e
        .and_then(|a| a.fade_out)
        .unwrap_or(0.0)
        .clamp(0.0, c.length);
    (
        x_of(c.start + fi) + HANDLE / 2.0,
        x_of(c.start + c.length - fo) - HANDLE / 2.0 - 1.0,
    )
}

/// The size of a fade handle, points.
const HANDLE: f32 = 8.0;

/// The audio clip whose fade handle is under `p`: its id and whether it is the fade out.
fn fade_handle_at(
    p: &Project,
    pos: egui::Pos2,
    x_of: impl Fn(f64) -> f32 + Copy,
    top: f32,
    row: f32,
) -> Option<(String, bool)> {
    p.playlist.clips.iter().rev().find_map(|c| {
        if c.kind != ClipKind::Audio {
            return None;
        }
        let y = top + c.track as f32 * row + 2.0 + HANDLE / 2.0;
        if (pos.y - y).abs() > HANDLE {
            return None;
        }
        let (a, b) = fade_handles(c, x_of);
        if (pos.x - b).abs() <= HANDLE {
            Some((c.id.clone(), true))
        } else if (pos.x - a).abs() <= HANDLE {
            Some((c.id.clone(), false))
        } else {
            None
        }
    })
}

/// The audio clip part of the playlist's right click menu: the clip's edits.
fn audio_clip_menu(ui: &mut Ui, project: &Project, c: &Clip, edits: &mut Vec<PlEdit>) {
    ui.label(egui::RichText::new("Audio clip").strong());
    crate::sampleedit::menu_items(ui, &c.ref_id);
    let seconds = sample_seconds(project, c);
    let before = c.audio.clone().unwrap_or_default();
    let out = crate::clipedit::panel(ui, &before, seconds, project.bpm, c.length);
    if out.begin {
        edits.push(PlEdit::Begin);
    }
    if let Some(e) = out.edit {
        // stretching to the tempo makes the clip as long as the sample then plays
        let tempo = |x: &AudioClipEdit| x.stretch.as_ref().map(|s| s.bpm);
        if let Some(sec) = seconds.filter(|_| tempo(&e) != tempo(&before)) {
            let own = tempo(&e).unwrap_or(project.bpm);
            let len = crate::clipedit::stretched_length(sec, own);
            edits.push(PlEdit::Resize {
                id: c.id.clone(),
                length: (len - c.offset).max(1.0),
            });
        }
        edits.push(PlEdit::AudioEdit {
            id: c.id.clone(),
            edit: Box::new(e),
        });
    }
}

/// How long the sample an audio clip plays is, seconds.
pub fn sample_seconds(project: &Project, c: &Clip) -> Option<f64> {
    let ch = project.channels.iter().find(|x| x.id == c.ref_id)?;
    let id = ch.sample_id.as_ref()?;
    project.samples.iter().find(|s| &s.id == id)?.duration
}

/// The name and colour a clip is drawn with.
pub fn describe(p: &Project, c: &Clip) -> (String, Color32) {
    match c.kind {
        ClipKind::Pattern => match p.patterns.iter().find(|x| x.id == c.ref_id) {
            Some(x) => (x.name.clone(), hex(&x.color)),
            None => ("(missing pattern)".into(), Color32::from_gray(90)),
        },
        ClipKind::Audio => match p.channels.iter().find(|x| x.id == c.ref_id) {
            Some(x) => (x.name.clone(), channel_colour(x)),
            None => ("(missing audio)".into(), Color32::from_gray(90)),
        },
        ClipKind::Automation => match p.automation.iter().find(|x| x.id == c.ref_id) {
            Some(x) => (x.name.clone(), hex(&x.color)),
            None => ("(missing automation)".into(), Color32::from_gray(90)),
        },
    }
}

fn hex(s: &str) -> Color32 {
    crate::rack::channel_colour_of(s)
}

/// A pattern clip for the pattern at `index`, starting at `start` on `track`.
pub fn new_pattern_clip(p: &Project, index: usize, start: f64, track: usize) -> Option<Clip> {
    let pat = p.patterns.get(index)?;
    Some(Clip {
        id: uid("c"),
        kind: ClipKind::Pattern,
        ref_id: pat.id.clone(),
        track,
        start,
        length: pattern_length(Some(pat), p.time_sig),
        ..Default::default()
    })
}

/// The zoom (points per tick) that shows the whole song, and a bar after it, in a view
/// `width` points wide; held to the zoom slider's range.
pub fn fit_zoom(p: &Project, width: f32) -> f32 {
    let bar = ticks_per_bar(p.time_sig);
    let ticks = song_length(p).max(bar * 4.0) + bar;
    ((width - HEAD - 24.0).max(100.0) / ticks as f32).clamp(0.03, 0.6)
}

/// The colours a clip's menu offers, after FL's playlist palette.
pub const CLIP_COLOURS: [&str; 12] = [
    "#e05a47", "#e8953a", "#e3c94a", "#8ccf4d", "#3fbf7f", "#3fb7c9", "#4a8fe0", "#6f6fe8",
    "#a463e0", "#d65ab8", "#8a8f99", "#c9cdd4",
];

/// The stretch between the markers on either side of `t`: from the last marker at or
/// before it to the next one after it.
pub fn loop_around(marks: &[f64], t: f64) -> Option<(f64, f64)> {
    let a = marks
        .iter()
        .copied()
        .filter(|&m| m <= t + 1e-6)
        .fold(None, |b: Option<f64>, m| Some(b.map_or(m, |b| b.max(m))))?;
    let b = marks
        .iter()
        .copied()
        .filter(|&m| m > a + 1e-6)
        .fold(None, |b: Option<f64>, m| Some(b.map_or(m, |b| b.min(m))))?;
    Some((a, b))
}

/// Carry out one edit on a project.
pub fn apply(project: &mut Project, edit: &PlEdit) {
    let rows = track_count(project);
    let clips = &mut project.playlist.clips;
    match edit {
        PlEdit::Begin
        | PlEdit::Seek(_)
        | PlEdit::OpenAuto(_)
        | PlEdit::OpenPattern(_)
        | PlEdit::AskRenameTrack(_)
        | PlEdit::AskRenameMarker(_)
        | PlEdit::ChoosePattern(_)
        | PlEdit::AskRenamePattern(_)
        | PlEdit::LoopOn(_)
        | PlEdit::SongLoop(_) => {}
        PlEdit::ClonePattern(id) => {
            if let Some(i) = project.patterns.iter().position(|p| &p.id == id) {
                crate::menus::clone_pattern(project, i);
            }
        }
        PlEdit::DeletePattern(id) => {
            if let Some(i) = project.patterns.iter().position(|p| &p.id == id) {
                crate::menus::delete_pattern(project, i);
            }
        }
        PlEdit::SplitByChannel(id) => {
            split_by_channel(project, id);
        }
        PlEdit::MoveMarker { id, tick } => {
            if let Some(list) = &mut project.markers {
                if let Some(m) = list.iter_mut().find(|m| &m.id == id) {
                    m.tick = tick.max(0.0);
                }
                list.sort_by(|a, b| a.tick.total_cmp(&b.tick));
            }
        }
        PlEdit::Trim { id, start } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                let end = c.start + c.length;
                // never past the right edge, never before the start of what it plays
                let start = start.max(c.start - c.offset).max(0.0).min(end - 1.0);
                let d = start - c.start;
                c.start = start;
                c.length = end - start;
                c.offset = (c.offset + d).max(0.0);
            }
        }
        PlEdit::TrackSolo(t) => {
            ensure_track(project, *t);
            let tr = &mut project.playlist.tracks[*t];
            tr.solo = !tr.solo;
        }
        PlEdit::TrackColour(t, col) => {
            ensure_track(project, *t);
            project.playlist.tracks[*t].color = col.clone();
        }
        PlEdit::InsertTrack(t) => {
            ensure_track(project, t.saturating_sub(1));
            let at = (*t).min(project.playlist.tracks.len());
            project
                .playlist
                .tracks
                .insert(at, las_project::create::playlist_track(at));
            for c in &mut project.playlist.clips {
                if c.track >= at {
                    c.track += 1;
                }
            }
        }
        PlEdit::DeleteTrack(t) => {
            if *t < project.playlist.tracks.len() {
                project.playlist.tracks.remove(*t);
            }
            project.playlist.clips.retain(|c| c.track != *t);
            for c in &mut project.playlist.clips {
                if c.track > *t {
                    c.track -= 1;
                }
            }
        }
        PlEdit::Colour { clip, color } => {
            let Some(c) = project.playlist.clips.iter().find(|c| &c.id == clip) else {
                return;
            };
            let (kind, of) = (c.kind, c.ref_id.clone());
            match kind {
                ClipKind::Pattern => {
                    if let Some(x) = project.patterns.iter_mut().find(|x| x.id == of) {
                        x.color = color.clone();
                    }
                }
                ClipKind::Automation => {
                    if let Some(x) = project.automation.iter_mut().find(|x| x.id == of) {
                        x.color = color.clone();
                    }
                }
                ClipKind::Audio => {}
            }
        }
        PlEdit::MakeUnique(ids) => {
            for clip in ids {
                let Some(ci) = project.playlist.clips.iter().position(|c| &c.id == clip) else {
                    continue;
                };
                if project.playlist.clips[ci].kind != ClipKind::Pattern {
                    continue;
                }
                let of = project.playlist.clips[ci].ref_id.clone();
                let Some(pi) = project.patterns.iter().position(|x| x.id == of) else {
                    continue;
                };
                if let Some(ni) = crate::menus::clone_pattern(project, pi) {
                    let id = project.patterns[ni].id.clone();
                    project.playlist.clips[ci].ref_id = id;
                }
            }
        }
        PlEdit::AddMarker(t) => {
            let list = project.markers.get_or_insert_with(Vec::new);
            let name = format!("Marker {}", list.len() + 1);
            list.push(las_project::Marker {
                id: las_project::create::uid("mk_"),
                tick: t.max(0.0),
                name,
                color: None,
                extra: Default::default(),
            });
            list.sort_by(|a, b| a.tick.total_cmp(&b.tick));
        }
        PlEdit::DeleteMarker(id) => {
            if let Some(list) = &mut project.markers {
                list.retain(|m| &m.id != id);
                if list.is_empty() {
                    project.markers = None;
                }
            }
        }
        PlEdit::TrackMute(t) => {
            // tracks that the project does not list yet are made as they are needed
            ensure_track(project, *t);
            let tr = &mut project.playlist.tracks[*t];
            tr.mute = !tr.mute;
        }
        PlEdit::MoveMany {
            ids,
            dstart,
            dtrack,
        } => {
            let first = clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.start)
                .fold(f64::MAX, f64::min);
            let dstart = if first == f64::MAX {
                0.0
            } else {
                dstart.max(-first)
            };
            let lowest = clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.track as i32)
                .min()
                .unwrap_or(0);
            let highest = clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.track as i32)
                .max()
                .unwrap_or(0);
            // the group keeps its shape between the first and the last track
            let dtrack = (*dtrack).clamp(-lowest, rows as i32 - 1 - highest);
            for c in clips.iter_mut().filter(|c| ids.contains(&c.id)) {
                c.start += dstart;
                c.track = (c.track as i32 + dtrack) as usize;
            }
        }
        PlEdit::DeleteMany { ids } => clips.retain(|c| !ids.contains(&c.id)),
        PlEdit::Paste { clips: add } => clips.extend(add.iter().cloned()),
        PlEdit::Add(c) => clips.push((**c).clone()),
        PlEdit::Move { id, start, track } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.start = start.max(0.0);
                c.track = (*track).min(rows - 1);
            }
        }
        PlEdit::Resize { id, length } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.length = length.max(1.0);
            }
        }
        PlEdit::Delete { id } => clips.retain(|c| &c.id != id),
        PlEdit::AudioEdit { id, edit } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.audio = (!crate::clipedit::is_plain(edit)).then(|| (**edit).clone());
            }
        }
        PlEdit::Mute { id } => {
            if let Some(c) = clips.iter_mut().find(|c| &c.id == id) {
                c.muted = Some(c.muted != Some(true));
            }
        }
        PlEdit::Split { id, at } => {
            let Some(i) = clips.iter().position(|c| &c.id == id) else {
                return;
            };
            let c = &clips[i];
            if *at <= c.start || *at >= c.start + c.length {
                return;
            }
            let mut right = c.clone();
            right.id = uid("c");
            right.start = *at;
            right.length = c.start + c.length - at;
            right.offset = c.offset + (at - c.start);
            clips[i].length = at - clips[i].start;
            clips.insert(i + 1, right);
        }
    }
}

/// Draw the playlist (and the picker on its left when shown). `pattern` is the current
/// pattern, which a click places when the picker has a pattern chosen; `start` is where
/// "play song" starts; `playhead` is shown while the song plays.
pub fn draw(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    start: f64,
    state: &mut PlaylistState,
    playhead: Option<f64>,
) -> Vec<PlEdit> {
    let mut edits = Vec::new();
    let pl = crate::theme::pal();
    if state.show_picker {
        picker(ui, project, pattern, state, &mut edits);
    }
    drag_label(ui.ctx());
    wheel_zoom(ui, ui.available_rect_before_wrap(), state);
    let rows = track_count(project);
    let silent = project.playlist.silent_tracks();
    let bar = ticks_per_bar(project.time_sig);
    // like FL Studio's, the playlist runs well past the song: at least 128 bars, and
    // always eight bars beyond the last clip
    let ticks = (song_length(project) + bar * 8.0).max(bar * 128.0);
    let width = HEAD + ticks as f32 * state.zoom;
    let height = RULER + rows as f32 * state.row;
    let mut drawn = None;
    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt("playlist");
    if let Some(x) = state.want_x.take() {
        area = area.horizontal_scroll_offset(x);
    }
    let out = area.show_viewport(ui, |ui, viewport| {
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
        let o = resp.rect.min;
        let left = o.x + HEAD;
        let x_of = |t: f64| left + t as f32 * state.zoom;
        let tick_at = |x: f32| f64::from((x - left) / state.zoom);
        let top = o.y + RULER;
        let track_at = |y: f32| (((y - top) / state.row).floor().max(0.0) as usize).min(rows - 1);
        let (vl, vr) = (o.x + viewport.min.x, o.x + viewport.max.x);
        let (vt, vb) = (o.y + viewport.min.y, o.y + viewport.max.y);
        drawn = Some(Area {
            rect: Rect::from_min_max(egui::pos2(vl, vt), egui::pos2(vr, vb)),
            left,
            top,
            zoom: state.zoom,
            row: state.row,
            rows,
            snap: state.snap,
        });

        // rows
        for r in 0..rows {
            let y = top + r as f32 * state.row;
            let shade = if r % 2 == 0 { pl.bg2 } else { pl.bg };
            painter.rect_filled(
                Rect::from_min_size(egui::pos2(left, y), egui::vec2(width - HEAD, state.row)),
                0.0,
                shade,
            );
        }
        // bar lines and numbers
        let mut t = 0.0;
        let mut n = 1;
        while t <= ticks {
            let x = x_of(t);
            if x >= vl - 40.0 && x <= vr {
                painter.line_segment(
                    [egui::pos2(x, o.y + RULER - 6.0), egui::pos2(x, vb)],
                    Stroke::new(1.0, pl.grid_bar),
                );
                painter.text(
                    egui::pos2(x + 3.0, o.y + 4.0),
                    egui::Align2::LEFT_TOP,
                    n.to_string(),
                    egui::FontId::proportional(10.5),
                    pl.muted,
                );
            }
            t += bar;
            n += 1;
        }
        // clips
        for c in &project.playlist.clips {
            let (name, col) = describe(project, c);
            let r = Rect::from_min_max(
                egui::pos2(x_of(c.start), top + c.track as f32 * state.row + 2.0),
                egui::pos2(
                    (x_of(c.start + c.length) - 1.0).max(x_of(c.start) + 3.0),
                    top + (c.track + 1) as f32 * state.row - 2.0,
                ),
            );
            if r.right() < vl || r.left() > vr {
                continue;
            }
            let muted = c.muted == Some(true);
            let fill = col.gamma_multiply(if muted { 0.25 } else { 0.8 });
            painter.rect_filled(r, 3.0, fill);
            painter.rect_stroke(
                r,
                3.0,
                if state.selected.contains(&c.id) {
                    Stroke::new(2.0, Color32::WHITE)
                } else {
                    Stroke::new(1.0, Color32::from_black_alpha(150))
                },
                egui::StrokeKind::Inside,
            );
            if c.kind == ClipKind::Pattern {
                if let Some(pat) = project.patterns.iter().find(|x| x.id == c.ref_id) {
                    // the notes of the pattern in small, repeated as often as the clip is long
                    let plen = pattern_length(Some(pat), project.time_sig).max(1.0);
                    let keys = pat.notes.values().flatten().map(|n| n.key);
                    let (lo, hi) =
                        keys.fold((i32::MAX, i32::MIN), |(a, b), k| (a.min(k), b.max(k)));
                    if lo <= hi {
                        let span = (hi - lo).max(7) as f32 + 1.0;
                        let h = r.height() - 20.0;
                        let inner = painter.with_clip_rect(r);
                        let mut base = -c.offset.rem_euclid(plen);
                        while base < c.length {
                            for n in pat.notes.values().flatten() {
                                let (a, b) = (base + n.pos, base + n.pos + n.len.min(plen));
                                if b < 0.0 || a > c.length {
                                    continue;
                                }
                                let y = r.bottom() - 4.0 - (n.key - lo) as f32 / span * h;
                                inner.rect_filled(
                                    Rect::from_min_max(
                                        egui::pos2(x_of(c.start + a.max(0.0)), y - 1.5),
                                        egui::pos2(
                                            x_of(c.start + b.min(c.length))
                                                .max(x_of(c.start + a.max(0.0)) + 1.5),
                                            y + 1.0,
                                        ),
                                    ),
                                    0.0,
                                    Color32::from_black_alpha(150),
                                );
                            }
                            base += plen;
                        }
                    }
                }
            }
            if c.kind == ClipKind::Audio {
                let wave = project
                    .channels
                    .iter()
                    .find(|x| x.id == c.ref_id)
                    .and_then(|x| x.sample_id.as_ref())
                    .and_then(|id| state.waves.get(id));
                if let Some(w) = wave {
                    // the outline of the sound, at the project's tempo
                    let sec_per_tick = 60.0 / project.bpm.max(1.0) / f64::from(PPQ);
                    let mid = r.top() + 14.0 + (r.height() - 16.0) / 2.0;
                    let half = (r.height() - 18.0) / 2.0;
                    let inner = painter
                        .with_clip_rect(r.intersect(Rect::from_x_y_ranges(vl..=vr, r.y_range())));
                    let (a, b) = (r.left().max(vl), r.right().min(vr));
                    let mut x = a;
                    while x < b {
                        let t = (c.offset + tick_at(x) - c.start) * sec_per_tick;
                        let v = w.at(t).min(1.0) * half;
                        if v > 0.3 {
                            inner.line_segment(
                                [egui::pos2(x, mid - v), egui::pos2(x, mid + v)],
                                Stroke::new(1.0, Color32::from_black_alpha(170)),
                            );
                        }
                        x += 1.0;
                    }
                }
                // the fades as lines, and their handles on the top corners
                let (ha, hb) = fade_handles(c, x_of);
                let inner = painter.with_clip_rect(r);
                let line = Stroke::new(1.5, Color32::from_black_alpha(200));
                let e = c.audio.as_ref();
                if e.and_then(|a| a.fade_in).unwrap_or(0.0) > 0.0 {
                    inner.line_segment([r.left_bottom(), egui::pos2(ha, r.top())], line);
                }
                if e.and_then(|a| a.fade_out).unwrap_or(0.0) > 0.0 {
                    inner.line_segment([egui::pos2(hb, r.top()), r.right_bottom()], line);
                }
                if r.width() > HANDLE * 3.0 {
                    for x in [ha, hb] {
                        let h = Rect::from_center_size(
                            egui::pos2(x, r.top() + HANDLE / 2.0),
                            egui::vec2(HANDLE, HANDLE),
                        );
                        inner.rect_filled(h, 1.0, Color32::from_white_alpha(220));
                        inner.rect_stroke(
                            h,
                            1.0,
                            Stroke::new(1.0, Color32::from_black_alpha(200)),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }
            if c.kind == ClipKind::Automation {
                if let Some(a) = project.automation.iter().find(|a| a.id == c.ref_id) {
                    // the curve as it plays, from where the clip starts in it
                    let line: Vec<egui::Pos2> = las_engine::timeline::automation_curve(a)
                        .into_iter()
                        .map(|(t, v)| {
                            egui::pos2(
                                x_of(c.start + t - c.offset),
                                r.bottom() - 4.0 - v as f32 * (r.height() - 18.0),
                            )
                        })
                        .collect();
                    painter.with_clip_rect(r).add(egui::Shape::line(
                        line,
                        Stroke::new(1.5, Color32::from_black_alpha(190)),
                    ));
                }
            }
            // the title bar: its menu opens with a right click, as FL's clip menu
            painter.with_clip_rect(r).rect_filled(
                Rect::from_min_size(r.min, egui::vec2(r.width(), TITLE)),
                3.0,
                Color32::from_black_alpha(40),
            );
            // a small triangle: the clip's menu
            let m = r.left_top() + egui::vec2(4.0, 6.0);
            painter.with_clip_rect(r).add(egui::Shape::convex_polygon(
                vec![m, m + egui::vec2(7.0, 0.0), m + egui::vec2(3.5, 5.0)],
                Color32::from_black_alpha(170),
                Stroke::NONE,
            ));
            painter.with_clip_rect(r).text(
                r.left_top() + egui::vec2(14.0, 2.0),
                egui::Align2::LEFT_TOP,
                name,
                egui::FontId::proportional(11.5),
                Color32::from_gray(15),
            );
        }
        if let Some((a, b)) = state.marquee {
            let m = Rect::from_two_pos(a, b);
            painter.rect_filled(m, 0.0, pl.accent.gamma_multiply(0.18));
            painter.rect_stroke(
                m,
                0.0,
                Stroke::new(1.0, pl.accent),
                egui::StrokeKind::Inside,
            );
        }
        // where "play song" starts, and the playhead
        let mark = |t: f64, col: Color32, w: f32| {
            painter.line_segment(
                [egui::pos2(x_of(t), o.y), egui::pos2(x_of(t), vb)],
                Stroke::new(w, col),
            );
        };
        mark(start, crate::theme::accent(), 1.5);
        if let Some(t) = playhead {
            mark(t, Color32::WHITE, 1.5);
        }
        // the track headers stay at the left of the view: a colour strip, the name,
        // solo and mute, and a menu on a right click
        for r in 0..rows {
            let y = top + r as f32 * state.row;
            let cell = Rect::from_min_size(egui::pos2(vl, y), egui::vec2(HEAD, state.row));
            let track = project.playlist.tracks.get(r);
            painter.rect_filled(cell, 0.0, pl.panel2);
            painter.rect_stroke(
                cell,
                0.0,
                Stroke::new(1.0, pl.border),
                egui::StrokeKind::Inside,
            );
            let colour = track.map_or(Color32::from_gray(70), |t| hex(&t.color));
            painter.rect_filled(
                Rect::from_min_size(
                    cell.min + egui::vec2(1.0, 1.0),
                    egui::vec2(5.0, cell.height() - 2.0),
                ),
                0.0,
                colour,
            );
            let name = track
                .map(|t| t.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Track {}", r + 1));
            let muted = track.is_some_and(|t| t.mute);
            let solo = track.is_some_and(|t| t.solo);
            let silent = silent.get(r).copied().unwrap_or(false);
            painter
                .with_clip_rect(cell.shrink(1.0).with_max_x(cell.right() - 44.0))
                .text(
                    cell.left_center() + egui::vec2(11.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &name,
                    egui::FontId::proportional(12.0),
                    if silent { pl.muted } else { pl.text2 },
                );
            // solo: an "S" that lights up
            let s_rect = Rect::from_center_size(
                egui::pos2(cell.right() - 34.0, cell.center().y),
                egui::vec2(16.0, 16.0),
            );
            painter.rect_filled(s_rect, 3.0, if solo { pl.accent } else { pl.well_deep });
            painter.text(
                s_rect.center(),
                egui::Align2::CENTER_CENTER,
                "S",
                egui::FontId::proportional(10.5),
                if solo { Color32::BLACK } else { pl.muted },
            );
            let hit_s = ui.interact(
                s_rect,
                ui.make_persistent_id(("track-solo", r)),
                Sense::click(),
            );
            if hit_s
                .on_hover_text("Solo: hear only this track (and other soloed ones)")
                .clicked()
            {
                edits.push(PlEdit::Begin);
                edits.push(PlEdit::TrackSolo(r));
            }
            // mute: FL's green light; Ctrl+click solos, as in FL
            let dot = Rect::from_center_size(
                egui::pos2(cell.right() - 14.0, cell.center().y),
                egui::vec2(18.0, 18.0),
            );
            painter.circle_filled(dot.center(), 5.0, if muted { pl.led_off } else { pl.ok });
            let hit_dot = ui.interact(
                dot,
                ui.make_persistent_id(("track-mute", r)),
                Sense::click(),
            );
            if hit_dot
                .on_hover_text("Mute the track (Ctrl+click: solo it)")
                .clicked()
            {
                edits.push(PlEdit::Begin);
                if ui.input(|i| i.modifiers.command) {
                    edits.push(PlEdit::TrackSolo(r));
                } else {
                    edits.push(PlEdit::TrackMute(r));
                }
            }
            let head = ui.interact(
                cell.with_max_x(cell.right() - 44.0),
                ui.make_persistent_id(("track-head", r)),
                Sense::click(),
            );
            if head.double_clicked() {
                edits.push(PlEdit::AskRenameTrack(r));
            }
            head.on_hover_text("Right click for the track's menu, double click to rename")
                .context_menu(|ui| {
                    if ui.button("Rename...").clicked() {
                        edits.push(PlEdit::AskRenameTrack(r));
                        ui.close();
                    }
                    ui.label("Colour");
                    ui.horizontal_wrapped(|ui| {
                        for c in CLIP_COLOURS {
                            let (rc, _) =
                                ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
                            ui.painter().rect_filled(rc, 3.0, hex(c));
                            if ui
                                .interact(
                                    rc,
                                    ui.make_persistent_id(("track-colour", r, c)),
                                    Sense::click(),
                                )
                                .clicked()
                            {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::TrackColour(r, c.to_string()));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button(if solo { "Unsolo" } else { "Solo" }).clicked() {
                        edits.push(PlEdit::Begin);
                        edits.push(PlEdit::TrackSolo(r));
                        ui.close();
                    }
                    if ui.button(if muted { "Unmute" } else { "Mute" }).clicked() {
                        edits.push(PlEdit::Begin);
                        edits.push(PlEdit::TrackMute(r));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button("Insert track")
                        .on_hover_text(
                            "Put an empty track here; this one and those below move down",
                        )
                        .clicked()
                    {
                        edits.push(PlEdit::Begin);
                        edits.push(PlEdit::InsertTrack(r));
                        ui.close();
                    }
                    if ui
                        .button("Delete track")
                        .on_hover_text("Remove this track and its clips")
                        .clicked()
                    {
                        edits.push(PlEdit::Begin);
                        edits.push(PlEdit::DeleteTrack(r));
                        ui.close();
                    }
                });
        }
        painter.rect_filled(
            Rect::from_min_max(egui::pos2(vl, vt), egui::pos2(vr, o.y + RULER)),
            0.0,
            pl.well_deep,
        );
        // the ruler again on top (it stays in view)
        let mut t = 0.0;
        let mut n = 1;
        while t <= ticks {
            let x = x_of(t);
            if x >= vl + HEAD - 30.0 && x <= vr {
                painter.text(
                    egui::pos2(x + 3.0, vt + 4.0),
                    egui::Align2::LEFT_TOP,
                    n.to_string(),
                    egui::FontId::proportional(10.5),
                    pl.muted,
                );
                painter.line_segment(
                    [egui::pos2(x, vt + RULER - 6.0), egui::pos2(x, vt + RULER)],
                    Stroke::new(1.0, pl.border2),
                );
            }
            t += bar;
            n += 1;
        }
        // the song loop: a band over the ruler, and the stretch shaded below it (dim
        // while looping is off)
        if let Some((a, b)) = state.song_loop {
            let on = state.loop_on;
            let (xa, xb) = (x_of(a).max(vl + HEAD), x_of(b).min(vr));
            if xb > xa {
                let band = Rect::from_min_max(egui::pos2(xa, vt + 2.0), egui::pos2(xb, vt + RULER));
                painter.rect_filled(
                    band,
                    2.0,
                    pl.accent.gamma_multiply(if on { 0.45 } else { 0.15 }),
                );
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(xa, vt + RULER), egui::pos2(xb, vb)),
                    0.0,
                    pl.accent.gamma_multiply(if on { 0.07 } else { 0.03 }),
                );
                for x in [x_of(a), x_of(b)] {
                    if x >= vl + HEAD && x <= vr {
                        painter.line_segment(
                            [egui::pos2(x, vt + 2.0), egui::pos2(x, vb)],
                            Stroke::new(1.0, pl.accent.gamma_multiply(if on { 0.9 } else { 0.35 })),
                        );
                    }
                }
            }
        }
        // song markers: a flag and a name on the ruler; a click jumps to one
        let mut near_marker = None;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if let Some(hp) = pointer.filter(|p| resp.rect.contains(*p)) {
            if fade_handle_at(project, hp, x_of, top, state.row).is_some()
                || matches!(state.drag, Some(Drag::Fade { .. }))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        }
        for m in project.markers.iter().flatten() {
            let x = x_of(m.tick);
            if x < vl + HEAD - 80.0 || x > vr {
                continue;
            }
            let hot = pointer.is_some_and(|p| {
                p.y >= vt && p.y < vt + RULER && p.x >= x - 6.0 && p.x < x + 12.0 + m.name.len() as f32 * 5.5
            }) || matches!(&state.drag, Some(Drag::Marker { id }) if *id == m.id);
            let col = if hot { pl.accent_text } else { pl.accent };
            let flag = egui::Shape::convex_polygon(
                vec![
                    egui::pos2(x, vt + 11.0),
                    egui::pos2(x + 7.0, vt + 14.5),
                    egui::pos2(x, vt + 18.0),
                ],
                col,
                Stroke::NONE,
            );
            painter.line_segment(
                [egui::pos2(x, vt + 10.0), egui::pos2(x, vb)],
                Stroke::new(1.0, col.gamma_multiply(if hot { 0.8 } else { 0.35 })),
            );
            painter.add(flag);
            painter.text(
                egui::pos2(x + 9.0, vt + 12.0),
                egui::Align2::LEFT_TOP,
                &m.name,
                egui::FontId::proportional(10.5),
                if hot { pl.accent_text } else { pl.text },
            );
            if hot {
                near_marker = Some(m.id.clone());
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        if let Some(p) = resp
            .interact_pointer_pos()
            .filter(|_| resp.secondary_clicked())
        {
            state.menu_at = Some((
                snapped(tick_at(p.x), state.snap).max(0.0),
                near_marker.clone(),
            ));
            let under = (p.y >= top && p.x > vl + HEAD)
                .then(|| hit(project, tick_at(p.x), track_at(p.y), 0.0))
                .flatten()
                .map(|(i, _)| &project.playlist.clips[i]);
            let in_title = |c: &Clip| p.y < top + c.track as f32 * state.row + 2.0 + TITLE;
            state.menu_clip = under.filter(|c| in_title(c)).map(|c| c.id.clone());
            if under.is_some_and(|c| !in_title(c)) {
                // a right click on a clip's body deletes it (below), as in FL
                state.menu_at = None;
            }
        }
        resp.context_menu(|ui| {
            let Some((t, near)) = state.menu_at.clone() else {
                ui.close();
                return;
            };
            if let Some(id) = state.menu_clip.clone() {
                if ui.button("Delete clip").clicked() {
                    edits.push(PlEdit::Begin);
                    edits.push(PlEdit::Delete { id: id.clone() });
                    ui.close();
                }
                let audio = project
                    .playlist
                    .clips
                    .iter()
                    .find(|c| c.id == id)
                    .is_some_and(|c| c.kind == ClipKind::Audio);
                if audio {
                    if let Some(c) = project.playlist.clips.iter().find(|c| c.id == id) {
                        audio_clip_menu(ui, project, c, &mut edits);
                        ui.separator();
                    }
                }
                if !audio {
                    ui.label("Colour");
                    ui.horizontal_wrapped(|ui| {
                        for c in CLIP_COLOURS {
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
                            ui.painter().rect_filled(r, 3.0, hex(c));
                            if ui
                                .interact(
                                    r,
                                    ui.make_persistent_id(("clip-colour", c)),
                                    Sense::click(),
                                )
                                .clicked()
                            {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::Colour {
                                    clip: id.clone(),
                                    color: c.to_string(),
                                });
                                ui.close();
                            }
                        }
                    });
                    let clip = project.playlist.clips.iter().find(|c| c.id == id);
                    if let Some(c) = clip.filter(|c| c.kind == ClipKind::Pattern) {
                        // FL: the clip and the other selected pattern clips
                        let mut ids: Vec<String> = vec![id.clone()];
                        if state.selected.contains(&id) {
                            ids.extend(
                                project
                                    .playlist
                                    .clips
                                    .iter()
                                    .filter(|x| {
                                        x.id != id
                                            && x.kind == ClipKind::Pattern
                                            && state.selected.contains(&x.id)
                                    })
                                    .map(|x| x.id.clone()),
                            );
                        }
                        let n = ids.len();
                        if ui
                            .button(if n > 1 {
                                format!("Make unique ({n} clips)")
                            } else {
                                "Make unique".to_string()
                            })
                            .on_hover_text(
                                "Give the clip (and the other selected ones) its own copy of the pattern, so editing it changes only this clip",
                            )
                            .clicked()
                        {
                            edits.push(PlEdit::Begin);
                            edits.push(PlEdit::MakeUnique(ids));
                            ui.close();
                        }
                        if ui.button("Rename pattern...").clicked() {
                            edits.push(PlEdit::AskRenamePattern(c.ref_id.clone()));
                            ui.close();
                        }
                        let chans = project
                            .pattern(&c.ref_id)
                            .map_or(0, |p| p.notes.values().filter(|n| !n.is_empty()).count());
                        if ui
                            .add_enabled(chans > 1, egui::Button::new("Split by channel"))
                            .on_hover_text(
                                "One pattern for each channel that plays in this one, their clips stacked on the tracks below",
                            )
                            .on_disabled_hover_text("Only one channel plays in this pattern")
                            .clicked()
                        {
                            edits.push(PlEdit::Begin);
                            edits.push(PlEdit::SplitByChannel(id.clone()));
                            ui.close();
                        }
                        if ui.button("Open in the piano roll").clicked() {
                            edits.push(PlEdit::OpenPattern(c.ref_id.clone()));
                            ui.close();
                        }
                    }
                    if let Some(c) = clip.filter(|c| c.kind == ClipKind::Automation) {
                        if ui.button("Edit automation...").clicked() {
                            edits.push(PlEdit::OpenAuto(c.ref_id.clone()));
                            ui.close();
                        }
                    }
                    ui.separator();
                }
                let what = match project.playlist.clips.iter().find(|c| c.id == id).map(|c| c.kind) {
                    Some(ClipKind::Pattern) => "pattern",
                    Some(ClipKind::Audio) => "sound",
                    _ => "automation",
                };
                if ui
                    .button(format!("Select all clips of this {what}"))
                    .clicked()
                {
                    state.selected = clips_like(project, &id);
                    ui.close();
                }
                ui.separator();
            }
            if ui.button("Add marker here").clicked() {
                edits.push(PlEdit::Begin);
                edits.push(PlEdit::AddMarker(t));
                ui.close();
            }
            if let Some(id) = near {
                if ui.button("Rename marker...").clicked() {
                    edits.push(PlEdit::AskRenameMarker(id.clone()));
                    ui.close();
                }
                if ui.button("Delete marker").clicked() {
                    edits.push(PlEdit::Begin);
                    edits.push(PlEdit::DeleteMarker(id));
                    ui.close();
                }
            }
            let marks: Vec<f64> = project.markers.iter().flatten().map(|m| m.tick).collect();
            if let Some((a, b)) = loop_around(&marks, t) {
                if ui.button("Loop between these markers").clicked() {
                    edits.push(PlEdit::SongLoop(Some((a, b))));
                    ui.close();
                }
            }
            let bar = ticks_per_bar(project.time_sig);
            let from = (t / bar).floor() * bar;
            if ui.button("Loop this bar").clicked() {
                edits.push(PlEdit::SongLoop(Some((from, from + bar))));
                ui.close();
            }
            if ui.button("Loop four bars from here").clicked() {
                edits.push(PlEdit::SongLoop(Some((from, from + bar * 4.0))));
                ui.close();
            }
            if state.song_loop.is_some() {
                let on = state.loop_on;
                if ui
                    .button(if on { "Turn the loop off" } else { "Turn the loop on" })
                    .on_hover_text("Ctrl+Shift+L")
                    .clicked()
                {
                    edits.push(PlEdit::LoopOn(!on));
                    ui.close();
                }
                if ui.button("Clear the loop").clicked() {
                    edits.push(PlEdit::SongLoop(None));
                    ui.close();
                }
            }
            ui.separator();
            for m in project.markers.iter().flatten() {
                if ui.button(format!("Go to {}", m.name)).clicked() {
                    edits.push(PlEdit::Seek(m.tick));
                    ui.close();
                }
            }
        });
        mark_ruler(&painter, x_of(start), vt);

        // the mouse
        let edge = f64::from(EDGE / state.zoom);
        let in_ruler = |p: egui::Pos2| p.y < top;
        let in_head = |p: egui::Pos2| p.x < vl + HEAD;
        let shift = ui.input(|i| i.modifiers.shift);
        let place_tool = matches!(state.tool, Tool::Draw | Tool::Paint);
        // what the draw and paint tools place (the current pattern if the picked
        // clip is gone)
        let template = source_clip(project, &state.pick, pattern, 0.0, 0)
            .or_else(|| new_pattern_clip(project, pattern, 0.0, 0));

        // the cursor tells what a press would do
        if let Some(h) = resp.hover_pos().filter(|h| !in_ruler(*h) && !in_head(*h)) {
            let icon = match (&state.drag, hit(project, tick_at(h.x), track_at(h.y), edge)) {
                (Some(Drag::Resize { .. } | Drag::Trim { .. }), _) => {
                    Some(egui::CursorIcon::ResizeHorizontal)
                }
                (None, Some((_, Part::Left | Part::Right)))
                    if matches!(state.tool, Tool::Draw | Tool::Paint | Tool::Select) =>
                {
                    Some(egui::CursorIcon::ResizeHorizontal)
                }
                (Some(Drag::Move { .. } | Drag::Group), _) => Some(egui::CursorIcon::Grabbing),
                (None, Some((_, Part::Body)))
                    if matches!(state.tool, Tool::Draw | Tool::Paint | Tool::Select) =>
                {
                    Some(egui::CursorIcon::Grab)
                }
                _ => None,
            };
            if let Some(icon) = icon {
                ui.ctx().set_cursor_icon(icon);
            }
        }

        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                let (t, tr) = (tick_at(p.x), track_at(p.y));
                state.just_added = None;
                if in_ruler(p) {
                    if let Some(id) = near_marker.clone() {
                        // a marker's flag: drag it along
                        edits.push(PlEdit::Begin);
                        state.drag = Some(Drag::Marker { id });
                    } else {
                        // FL: a drag along the ruler sets the song loop
                        state.drag = Some(Drag::LoopRange {
                            from: snapped(t, state.snap).max(0.0),
                        });
                    }
                } else if let Some((id, out)) = fade_handle_at(project, p, x_of, top, state.row)
                    .filter(|_| !in_head(p) && matches!(state.tool, Tool::Draw | Tool::Select))
                {
                    edits.push(PlEdit::Begin);
                    state.drag = Some(Drag::Fade { id, out });
                } else if in_head(p)
                    || !matches!(state.tool, Tool::Draw | Tool::Select | Tool::Paint)
                {
                    // the other tools act on a click, not a drag
                } else {
                    match hit(project, t, tr, edge) {
                        Some((i, Part::Right)) => {
                            edits.push(PlEdit::Begin);
                            state.drag = Some(Drag::Resize {
                                id: project.playlist.clips[i].id.clone(),
                            });
                        }
                        Some((i, Part::Left)) => {
                            edits.push(PlEdit::Begin);
                            state.drag = Some(Drag::Trim {
                                id: project.playlist.clips[i].id.clone(),
                            });
                        }
                        Some((i, Part::Body)) if shift && state.tool != Tool::Select => {
                            // Shift+drag copies, as in FL: the copy moves
                            let c = &project.playlist.clips[i];
                            edits.push(PlEdit::Begin);
                            if state.selected.contains(&c.id) && state.selected.len() > 1 {
                                let fresh: Vec<Clip> = project
                                    .playlist
                                    .clips
                                    .iter()
                                    .filter(|x| state.selected.contains(&x.id))
                                    .map(|x| Clip {
                                        id: uid("c"),
                                        ..x.clone()
                                    })
                                    .collect();
                                state.selected = fresh.iter().map(|x| x.id.clone()).collect();
                                edits.push(PlEdit::Paste { clips: fresh });
                                state.drag = Some(Drag::Group);
                                state.group_last = Some((snapped(t, state.snap), tr));
                            } else {
                                let copy = Clip {
                                    id: uid("c"),
                                    ..c.clone()
                                };
                                state.selected = HashSet::from([copy.id.clone()]);
                                state.drag = Some(Drag::Move {
                                    id: copy.id.clone(),
                                    grab: t - c.start,
                                });
                                edits.push(PlEdit::Add(Box::new(copy)));
                            }
                        }
                        Some((i, Part::Body)) => {
                            let id = project.playlist.clips[i].id.clone();
                            edits.push(PlEdit::Begin);
                            let many = state.selected.contains(&id) && state.selected.len() > 1;
                            if many || state.tool == Tool::Select {
                                if !state.selected.contains(&id) {
                                    if !shift {
                                        state.selected.clear();
                                    }
                                    state.selected.insert(id);
                                }
                                state.drag = Some(Drag::Group);
                                state.group_last = Some((snapped(t, state.snap), tr));
                            } else {
                                let c = &project.playlist.clips[i];
                                state.selected = HashSet::from([id.clone()]);
                                state.drag = Some(Drag::Move {
                                    id,
                                    grab: t - c.start,
                                });
                            }
                        }
                        None if state.tool == Tool::Select => {
                            if !shift {
                                state.selected.clear();
                            }
                            state.drag = Some(Drag::Marquee);
                            state.marquee = Some((p, p));
                        }
                        None if state.tool == Tool::Paint => {
                            if let Some(tpl) = &template {
                                edits.push(PlEdit::Begin);
                                let (fresh, next) = paint_clips(
                                    &project.playlist.clips,
                                    tpl,
                                    None,
                                    t,
                                    tr,
                                    state.snap,
                                );
                                for c in fresh {
                                    edits.push(PlEdit::Add(Box::new(c)));
                                }
                                state.paint_next = Some(next);
                                state.drag = Some(Drag::Paint);
                            }
                        }
                        None => {
                            // the draw tool places a clip that follows the pointer
                            if let Some(tpl) = &template {
                                let at = (t / state.snap.max(1.0)).floor().max(0.0)
                                    * state.snap.max(1.0);
                                let c = Clip {
                                    id: uid("c"),
                                    start: at,
                                    track: tr,
                                    ..tpl.clone()
                                };
                                edits.push(PlEdit::Begin);
                                state.selected = HashSet::from([c.id.clone()]);
                                state.drag = Some(Drag::Move {
                                    id: c.id.clone(),
                                    grab: t - at,
                                });
                                edits.push(PlEdit::Add(Box::new(c)));
                            }
                        }
                    }
                }
            }
        } else if resp.dragged_by(egui::PointerButton::Primary) {
            if let (Some(d), Some(p)) = (state.drag.clone(), resp.interact_pointer_pos()) {
                let (t, tr) = (tick_at(p.x), track_at(p.y));
                match d {
                    Drag::Marker { id } => {
                        let fine = if ui.input(|i| i.modifiers.alt) { 1.0 } else { state.snap };
                        edits.push(PlEdit::MoveMarker {
                            id,
                            tick: snapped(t, fine).max(0.0),
                        });
                    }
                    Drag::LoopRange { from } => {
                        let g = state.snap.max(1.0);
                        let to = snapped(t, g).max(0.0);
                        let (a, b) = if to >= from { (from, to) } else { (to, from) };
                        // shown as it is dragged; the player hears of it when let go
                        if b - a >= g.min(f64::from(PPQ)) {
                            state.song_loop = Some((a, b));
                            state.loop_on = true;
                        }
                    }
                    Drag::Paint => {
                        if let Some(tpl) = &template {
                            let (fresh, next) = paint_clips(
                                &project.playlist.clips,
                                tpl,
                                state.paint_next,
                                t,
                                tr,
                                state.snap,
                            );
                            for c in fresh {
                                edits.push(PlEdit::Add(Box::new(c)));
                            }
                            state.paint_next = Some(next);
                        }
                    }
                    Drag::Resize { id } => {
                        if let Some(c) = project.playlist.clips.iter().find(|c| c.id == id) {
                            let end = snapped(t, state.snap);
                            edits.push(PlEdit::Resize {
                                id,
                                length: (end - c.start).max(state.snap.max(1.0)),
                            });
                        }
                    }
                    Drag::Fade { id, out } => {
                        if let Some(c) = project.playlist.clips.iter().find(|c| c.id == id) {
                            let mut e = c.audio.clone().unwrap_or_default();
                            let v = if out {
                                c.start + c.length - t
                            } else {
                                t - c.start
                            };
                            let v = v.clamp(0.0, c.length);
                            let v = (v > 1.0).then_some(v);
                            if out {
                                e.fade_out = v;
                            } else {
                                e.fade_in = v;
                            }
                            edits.push(PlEdit::AudioEdit {
                                id,
                                edit: Box::new(e),
                            });
                        }
                    }
                    Drag::Trim { id } => {
                        edits.push(PlEdit::Trim {
                            id,
                            start: snapped(t, state.snap),
                        });
                    }
                    Drag::Group => {
                        let cur = (snapped(t, state.snap), tr);
                        if let Some(last) = state.group_last {
                            let (dstart, dtrack) = (cur.0 - last.0, cur.1 as i32 - last.1 as i32);
                            if dstart != 0.0 || dtrack != 0 {
                                edits.push(PlEdit::MoveMany {
                                    ids: state.selected.iter().cloned().collect(),
                                    dstart,
                                    dtrack,
                                });
                                state.group_last = Some(cur);
                            }
                        }
                    }
                    Drag::Marquee => {
                        if let Some((a, _)) = state.marquee {
                            state.marquee = Some((a, p));
                        }
                    }
                    Drag::Move { id, grab } => {
                        edits.push(PlEdit::Move {
                            id,
                            start: snapped(t - grab, state.snap).max(0.0),
                            track: tr,
                        });
                    }
                }
            }
        } else if resp.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                if in_ruler(p) {
                    // a click on a marker jumps to it, elsewhere moves the start marker
                    let at = near_marker
                        .as_ref()
                        .and_then(|id| project.markers.iter().flatten().find(|m| &m.id == id))
                        .map_or_else(|| snapped(tick_at(p.x), state.snap).max(0.0), |m| m.tick);
                    edits.push(PlEdit::Seek(at));
                } else if !in_head(p) {
                    let (t, tr) = (tick_at(p.x), track_at(p.y));
                    let under = hit(project, t, tr, edge);
                    let added_before = state.just_added.take();
                    if matches!(state.tool, Tool::Draw | Tool::Select | Tool::Paint) {
                        match under {
                            Some((i, _)) => {
                                let id = project.playlist.clips[i].id.clone();
                                if added_before.as_ref() == Some(&id) {
                                    state.just_added = added_before;
                                }
                                if shift && state.selected.contains(&id) {
                                    state.selected.remove(&id);
                                } else {
                                    if !shift {
                                        state.selected.clear();
                                    }
                                    state.selected.insert(id);
                                }
                            }
                            None if place_tool => {
                                // a click places the picked clip, as in FL
                                let at = (t / state.snap.max(1.0)).floor().max(0.0)
                                    * state.snap.max(1.0);
                                if let Some(c) = template.clone().map(|tpl| Clip {
                                    id: uid("c"),
                                    start: at,
                                    track: tr,
                                    ..tpl
                                }) {
                                    edits.push(PlEdit::Begin);
                                    state.selected.clear();
                                    state.just_added = Some(c.id.clone());
                                    edits.push(PlEdit::Add(Box::new(c)));
                                }
                            }
                            None if !shift => state.selected.clear(),
                            None => {}
                        }
                    }
                    if let Some((i, _)) = under {
                        let id = project.playlist.clips[i].id.clone();
                        match state.tool {
                            Tool::Mute => {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::Mute { id });
                            }
                            Tool::Delete => {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::Delete { id });
                            }
                            Tool::Slice => {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::Split {
                                    id,
                                    at: snapped(t, state.snap),
                                });
                            }
                            Tool::Draw | Tool::Select | Tool::Paint => {}
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::LoopRange { .. }) = state.drag {
                if state.song_loop.is_some() {
                    edits.push(PlEdit::SongLoop(state.song_loop));
                }
            }
            if let Some(Drag::Marquee) = state.drag {
                if let Some((a, b)) = state.marquee.take() {
                    let area = Rect::from_two_pos(a, b);
                    for c in &project.playlist.clips {
                        let r = Rect::from_min_max(
                            egui::pos2(x_of(c.start), top + c.track as f32 * state.row),
                            egui::pos2(
                                x_of(c.start + c.length),
                                top + (c.track + 1) as f32 * state.row,
                            ),
                        );
                        if r.intersects(area) {
                            state.selected.insert(c.id.clone());
                        }
                    }
                }
            }
            state.group_last = None;
            state.paint_next = None;
            state.drag = None;
        }
        if resp.double_clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                if in_ruler(p) {
                    if let Some(id) = near_marker.clone() {
                        edits.push(PlEdit::AskRenameMarker(id));
                    }
                }
                if !in_head(p) && !in_ruler(p) {
                    let (t, tr) = (tick_at(p.x), track_at(p.y));
                    if let Some((i, _)) = hit(project, t, tr, edge) {
                        // a double click on a clip opens it, as in FL Studio (but not
                        // the clip the first click of the double click placed)
                        let c = &project.playlist.clips[i];
                        if state.just_added.as_ref() != Some(&c.id) {
                            match c.kind {
                                ClipKind::Automation => {
                                    edits.push(PlEdit::OpenAuto(c.ref_id.clone()))
                                }
                                ClipKind::Pattern => {
                                    edits.push(PlEdit::OpenPattern(c.ref_id.clone()))
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        if resp.clicked_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Secondary)
        {
            // a right click deletes the clip under it (a right drag, all it passes)
            if let Some(p) = resp.interact_pointer_pos() {
                if !in_head(p) && !in_ruler(p) {
                    let under = hit(project, tick_at(p.x), track_at(p.y), edge).filter(|(i, _)| {
                        let c = &project.playlist.clips[*i];
                        p.y >= top + c.track as f32 * state.row + 2.0 + TITLE
                            || resp.dragged_by(egui::PointerButton::Secondary)
                    });
                    if let Some((i, _)) = under {
                        edits.push(PlEdit::Begin);
                        edits.push(PlEdit::Delete {
                            id: project.playlist.clips[i].id.clone(),
                        });
                    }
                }
            }
        }
        // a pattern, sound or automation clip dragged from the picker
        if let Some(d) = resp.dnd_release_payload::<PickDrag>() {
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                if !in_head(p) && !in_ruler(p) {
                    let at =
                        (tick_at(p.x) / state.snap.max(1.0)).floor().max(0.0) * state.snap.max(1.0);
                    let index = d
                        .pattern
                        .as_ref()
                        .and_then(|id| project.patterns.iter().position(|x| &x.id == id))
                        .unwrap_or(pattern);
                    if let Some(c) = source_clip(project, &d.source, index, at, track_at(p.y)) {
                        edits.push(PlEdit::Begin);
                        state.selected = HashSet::from([c.id.clone()]);
                        edits.push(PlEdit::Add(Box::new(c)));
                    }
                    state.pick = d.source.clone();
                    if let Some(id) = &d.pattern {
                        edits.push(PlEdit::ChoosePattern(id.clone()));
                    }
                }
            }
        }
    });
    state.scroll_x = out.state.clamped_offset().x;
    keyboard(
        ui,
        out.inner_rect,
        project,
        start,
        playhead,
        state,
        &mut edits,
    );
    state.area = drawn;
    edits
}

/// Ctrl+wheel zooms the playlist in time around the pointer.
fn wheel_zoom(ui: &mut Ui, region: Rect, state: &mut PlaylistState) {
    let Some(p) = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| region.contains(*p))
    else {
        return;
    };
    let (z, m) = ui.input(|i| (i.zoom_delta(), i.modifiers));
    if m.command && (z - 1.0).abs() > 1e-4 {
        let to = (state.zoom * z).clamp(0.03, 0.6);
        state.want_x = Some(crate::roll::zoom_about(
            state.scroll_x,
            p.x - region.left(),
            HEAD,
            state.zoom,
            to,
        ));
        state.zoom = to;
    }
}

/// While a picker item is dragged, its name follows the pointer.
fn drag_label(ctx: &egui::Context) {
    let Some(item) = egui::DragAndDrop::payload::<PickDrag>(ctx) else {
        return;
    };
    let Some(pos) = ctx.pointer_hover_pos() else {
        return;
    };
    egui::Area::new(egui::Id::unique("picker-drag-label"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + egui::vec2(14.0, 10.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(&item.name);
                ui.label(
                    egui::RichText::new("Drop it on a track to place it")
                        .size(11.0)
                        .weak(),
                );
            });
        });
}

/// The picker panel, after FL's: the project's patterns, audio clips and automation
/// clips. A click chooses what the draw and paint tools place; a drag places it.
fn picker(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    state: &mut PlaylistState,
    edits: &mut Vec<PlEdit>,
) {
    let pl = crate::theme::pal();
    egui::Panel::left("playlist-picker")
        .resizable(true)
        .default_size(170.0)
        .min_size(110.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("playlist-picker-list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut chosen: Option<PickDrag> = None;
                    let mut item = |ui: &mut Ui,
                                    on: bool,
                                    col: Color32,
                                    name: &str,
                                    drag: PickDrag,
                                    tip: &str|
                     -> egui::Response {
                        let r = ui
                            .horizontal(|ui| {
                                let (sw, _) =
                                    ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::hover());
                                ui.painter().rect_filled(sw, 2.0, col);
                                ui.add(egui::Button::selectable(on, name).truncate())
                            })
                            .inner
                            .interact(Sense::click_and_drag())
                            .on_hover_text(tip);
                        if r.drag_started() || r.clicked() {
                            chosen = Some(drag.clone());
                        }
                        r.dnd_set_drag_payload(drag);
                        r
                    };
                    let heading = |ui: &mut Ui, t: &str| {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(t).strong().size(11.5).color(pl.muted));
                    };
                    heading(ui, "PATTERNS");
                    for (i, x) in project.patterns.iter().enumerate() {
                        let on = state.pick == Source::Pattern && i == pattern;
                        let r = item(
                            ui,
                            on,
                            hex(&x.color),
                            &x.name,
                            PickDrag {
                                source: Source::Pattern,
                                pattern: Some(x.id.clone()),
                                name: x.name.clone(),
                            },
                            "Click to paint this pattern, drag it onto a track to place it, double click to open it in the piano roll",
                        );
                        if r.double_clicked() {
                            edits.push(PlEdit::OpenPattern(x.id.clone()));
                        }
                        r.context_menu(|ui| {
                            ui.label(egui::RichText::new(&x.name).strong());
                            if ui.button("Open in the piano roll").clicked() {
                                edits.push(PlEdit::OpenPattern(x.id.clone()));
                                ui.close();
                            }
                            if ui.button("Rename...").clicked() {
                                edits.push(PlEdit::AskRenamePattern(x.id.clone()));
                                ui.close();
                            }
                            if ui.button("Clone").on_hover_text("A copy to change on its own").clicked() {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::ClonePattern(x.id.clone()));
                                ui.close();
                            }
                            if ui
                                .add_enabled(project.patterns.len() > 1, egui::Button::new("Delete"))
                                .on_hover_text("Remove the pattern and its clips")
                                .clicked()
                            {
                                edits.push(PlEdit::Begin);
                                edits.push(PlEdit::DeletePattern(x.id.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Select all its clips").clicked() {
                                state.selected = project
                                    .playlist
                                    .clips
                                    .iter()
                                    .filter(|c| c.kind == ClipKind::Pattern && c.ref_id == x.id)
                                    .map(|c| c.id.clone())
                                    .collect();
                                ui.close();
                            }
                        });
                    }
                    let audio: Vec<_> = project
                        .channels
                        .iter()
                        .filter(|c| c.plugin_id == "audioclip")
                        .collect();
                    heading(ui, "AUDIO CLIPS");
                    if audio.is_empty() {
                        ui.label(
                            egui::RichText::new("Drop a sound file on the playlist")
                                .weak()
                                .size(11.0),
                        );
                    }
                    for c in audio {
                        let src = Source::Audio(c.id.clone());
                        item(
                            ui,
                            state.pick == src,
                            channel_colour(c),
                            &c.name,
                            PickDrag {
                                source: src,
                                pattern: None,
                                name: c.name.clone(),
                            },
                            "Click to paint this sound, drag it onto a track to place it",
                        );
                    }
                    heading(ui, "AUTOMATION");
                    if project.automation.is_empty() {
                        ui.label(
                            egui::RichText::new("Right click a knob: Create automation clip")
                                .weak()
                                .size(11.0),
                        );
                    }
                    for a in &project.automation {
                        let src = Source::Automation(a.id.clone());
                        let r = item(
                            ui,
                            state.pick == src,
                            hex(&a.color),
                            &a.name,
                            PickDrag {
                                source: src,
                                pattern: None,
                                name: a.name.clone(),
                            },
                            "Click to paint this automation, drag it onto a track to place it, double click to edit it",
                        );
                        if r.double_clicked() {
                            edits.push(PlEdit::OpenAuto(a.id.clone()));
                        }
                    }
                    if let Some(d) = chosen {
                        state.pick = d.source;
                        if let Some(id) = d.pattern {
                            edits.push(PlEdit::ChoosePattern(id));
                        }
                    }
                });
        });
}

/// What the keys do to the clips and the selection (the pure part of the playlist's
/// keyboard): Delete, Ctrl+A, Ctrl+D (deselect), copy, cut, paste (after the selection,
/// else at the playhead, else at the start marker), Ctrl+B (duplicate). Returns whether
/// something was copied.
pub fn keys_edit(
    keys: crate::roll::Keys,
    clips: &[Clip],
    start: f64,
    playhead: Option<f64>,
    state: &mut PlaylistState,
    edits: &mut Vec<PlEdit>,
) -> bool {
    if keys.all {
        state.selected = clips.iter().map(|c| c.id.clone()).collect();
    }
    if keys.deselect {
        state.selected.clear();
    }
    let sel: Vec<&Clip> = clips
        .iter()
        .filter(|c| state.selected.contains(&c.id))
        .collect();
    let first = sel.iter().map(|c| c.start).fold(f64::MAX, f64::min);
    let copied = (keys.copy || keys.cut) && !sel.is_empty();
    if copied {
        state.clipboard = sel
            .iter()
            .map(|c| Clip {
                start: c.start - first,
                ..(*c).clone()
            })
            .collect();
    }
    if (keys.delete || keys.cut) && !sel.is_empty() {
        let ids: Vec<String> = sel.iter().map(|c| c.id.clone()).collect();
        edits.push(PlEdit::Begin);
        edits.push(PlEdit::DeleteMany { ids });
        state.selected.clear();
        return copied;
    }
    if keys.paste && !state.clipboard.is_empty() {
        let end = sel
            .iter()
            .map(|c| c.start + c.length)
            .fold(None, |m: Option<f64>, e| Some(m.map_or(e, |m| m.max(e))));
        let at = end.or(playhead).unwrap_or(start).max(0.0);
        let fresh: Vec<Clip> = state
            .clipboard
            .iter()
            .map(|c| Clip {
                id: uid("c"),
                start: c.start + at,
                ..c.clone()
            })
            .collect();
        state.selected = fresh.iter().map(|c| c.id.clone()).collect();
        edits.push(PlEdit::Begin);
        edits.push(PlEdit::Paste { clips: fresh });
        return copied;
    }
    if keys.duplicate && !clips.is_empty() {
        let fresh = duplicate_clips(clips, &state.selected);
        state.selected = fresh.iter().map(|c| c.id.clone()).collect();
        edits.push(PlEdit::Begin);
        edits.push(PlEdit::Paste { clips: fresh });
    }
    copied
}

/// The playlist's keys, while the pointer is over it and no text field has the keyboard.
fn keyboard(
    ui: &mut Ui,
    area: Rect,
    project: &Project,
    start: f64,
    playhead: Option<f64>,
    state: &mut PlaylistState,
    edits: &mut Vec<PlEdit>,
) {
    if !ui.rect_contains_pointer(area) || ui.memory(|m| m.focused().is_some()) {
        return;
    }
    let keys = ui.input(crate::roll::Keys::read);
    if keys_edit(keys, &project.playlist.clips, start, playhead, state, edits) {
        ui.ctx().copy_text(format!(
            "{} clips from Lumen Audio Studio",
            state.clipboard.len()
        ));
    }
    let (prev, next, fit, toggle) = ui.input(|i| {
        let m = i.modifiers;
        let alt_arrow = |k| m.alt && !m.command && i.key_pressed(k);
        (
            alt_arrow(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp),
            alt_arrow(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown),
            m.command && m.shift && i.key_pressed(egui::Key::F),
            m.command && m.shift && i.key_pressed(egui::Key::L),
        )
    });
    // jump to the marker before or after where the song is
    let now = playhead.unwrap_or(start);
    if prev || next {
        let to = marker_jump(project, now, next).or((prev && !next).then_some(0.0));
        if let Some(t) = to {
            edits.push(PlEdit::Seek(t));
        }
    }
    if fit {
        state.zoom = fit_zoom(project, area.width());
        state.want_x = Some(0.0);
    }
    if toggle && state.song_loop.is_some() {
        edits.push(PlEdit::LoopOn(!state.loop_on));
    }
}

fn mark_ruler(p: &egui::Painter, x: f32, top: f32) {
    let tri = vec![
        egui::pos2(x - 5.0, top),
        egui::pos2(x + 5.0, top),
        egui::pos2(x, top + 9.0),
    ];
    p.add(egui::Shape::convex_polygon(
        tri,
        crate::theme::accent(),
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wave_keeps_the_loudest_level_of_each_stretch() {
        let mut c = vec![0.0f32; 1000];
        c[300] = -0.8;
        c[700] = 0.4;
        let w = Wave::of(&[c], 256.0);
        assert_eq!(w.peaks.len(), 4);
        assert_eq!(w.step, 1.0);
        assert_eq!((w.at(0.5), w.at(1.5), w.at(2.5)), (0.0, 0.8, 0.4));
        assert_eq!((w.at(-1.0), w.at(99.0)), (0.0, 0.0));
    }

    #[test]
    fn a_drop_lands_on_the_snap_and_the_track_under_it() {
        let a = Area {
            rect: Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 400.0)),
            left: 100.0,
            top: 20.0,
            zoom: 0.1,
            row: 40.0,
            rows: 5,
            snap: 96.0,
        };
        // 150 points right of the start is tick 1500, on the beat grid 1440
        assert_eq!(a.place(egui::pos2(250.0, 20.0 + 85.0)), Some((1440.0, 2)));
        assert_eq!(a.place(egui::pos2(50.0, 100.0)), None, "the track names");
        assert_eq!(a.place(egui::pos2(900.0, 100.0)), None, "outside");
        assert_eq!(a.place(egui::pos2(300.0, 399.0)).unwrap().1, 4);
    }

    #[test]
    fn several_files_dropped_land_on_tracks_one_after_another() {
        assert_eq!(
            drop_spots(Some((1440.0, 2)), 3),
            vec![Some((1440.0, 2)), Some((1440.0, 3)), Some((1440.0, 4))]
        );
        assert_eq!(drop_spots(None, 2), vec![None, None]);
        let mut p = project();
        let (at, t) = free_track(&p, 384.0);
        assert_eq!(at, 384.0);
        assert!(p.playlist.clips.iter().all(|c| c.track != t));
        p.playlist.clips.clear();
        assert_eq!(free_track(&p, 0.0), (0.0, 0));
    }

    fn project() -> Project {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/engines.las");
        las_project::decode(bytes).unwrap().project
    }

    #[test]
    fn clips_are_added_moved_resized_and_deleted() {
        let mut p = project();
        let before = p.playlist.clips.len();
        let c = new_pattern_clip(&p, 0, 384.0, 2).unwrap();
        let id = c.id.clone();
        apply(&mut p, &PlEdit::Add(Box::new(c)));
        assert_eq!(p.playlist.clips.len(), before + 1);
        apply(
            &mut p,
            &PlEdit::Move {
                id: id.clone(),
                start: 768.0,
                track: 99,
            },
        );
        apply(
            &mut p,
            &PlEdit::Resize {
                id: id.clone(),
                length: 96.0,
            },
        );
        let c = p.playlist.clips.iter().find(|c| c.id == id).unwrap();
        assert_eq!((c.start, c.length), (768.0, 96.0));
        assert!(
            c.track < track_count(&p),
            "a track past the end is held to the last row"
        );
        apply(&mut p, &PlEdit::Delete { id });
        assert_eq!(p.playlist.clips.len(), before);
    }

    #[test]
    fn a_loop_spans_the_markers_around_a_tick() {
        let m = [0.0, 384.0, 1536.0];
        assert_eq!(loop_around(&m, 500.0), Some((384.0, 1536.0)));
        assert_eq!(loop_around(&m, 384.0), Some((384.0, 1536.0)));
        assert_eq!(loop_around(&m, 2000.0), None);
        assert_eq!(loop_around(&[], 10.0), None);
    }

    #[test]
    fn a_clip_colours_its_pattern() {
        let mut p = project();
        let i = p
            .playlist
            .clips
            .iter()
            .position(|c| c.kind == ClipKind::Pattern)
            .unwrap();
        let (clip, of) = (
            p.playlist.clips[i].id.clone(),
            p.playlist.clips[i].ref_id.clone(),
        );
        apply(
            &mut p,
            &PlEdit::Colour {
                clip,
                color: "#3fbf7f".into(),
            },
        );
        assert_eq!(
            p.patterns.iter().find(|x| x.id == of).unwrap().color,
            "#3fbf7f"
        );
    }

    #[test]
    fn make_unique_gives_a_clip_its_own_pattern() {
        let mut p = project();
        let i = p
            .playlist
            .clips
            .iter()
            .position(|c| c.kind == ClipKind::Pattern)
            .unwrap();
        let (clip, of) = (
            p.playlist.clips[i].id.clone(),
            p.playlist.clips[i].ref_id.clone(),
        );
        let n = p.patterns.len();
        apply(&mut p, &PlEdit::MakeUnique(vec![clip]));
        assert_eq!(p.patterns.len(), n + 1);
        let now = &p.playlist.clips[i].ref_id;
        assert_ne!(now, &of);
        let (a, b) = (
            p.patterns.iter().find(|x| x.id == of).unwrap(),
            p.patterns.iter().find(|x| &x.id == now).unwrap(),
        );
        assert_eq!(
            a.notes.values().map(Vec::len).sum::<usize>(),
            b.notes.values().map(Vec::len).sum::<usize>()
        );
    }

    #[test]
    fn make_unique_works_on_the_selected_clips_too() {
        let mut p = las_project::create::default_project();
        let a = new_pattern_clip(&p, 0, 0.0, 0).unwrap();
        let b = new_pattern_clip(&p, 0, 384.0, 0).unwrap();
        let c = new_pattern_clip(&p, 0, 768.0, 0).unwrap();
        p.playlist.clips = vec![a.clone(), b.clone(), c.clone()];
        let n = p.patterns.len();
        apply(
            &mut p,
            &PlEdit::MakeUnique(vec![a.id.clone(), b.id.clone()]),
        );
        assert_eq!(p.patterns.len(), n + 2);
        let refs: Vec<&str> = p.playlist.clips.iter().map(|c| c.ref_id.as_str()).collect();
        assert!(refs[0] != refs[1] && refs[0] != refs[2] && refs[1] != refs[2]);
        assert_eq!(
            refs[2], c.ref_id,
            "the clip not asked for keeps its pattern"
        );
    }

    #[test]
    fn a_pattern_splits_into_one_per_channel_stacked_below() {
        let mut p = las_project::create::default_project();
        let pat = p.patterns[0].clone();
        let used: Vec<String> = p
            .channels
            .iter()
            .filter(|c| pat.notes.get(&c.id).is_some_and(|n| !n.is_empty()))
            .map(|c| c.id.clone())
            .collect();
        assert!(used.len() >= 2, "the demo pattern plays several channels");
        let clip = new_pattern_clip(&p, 0, 0.0, 1).unwrap();
        // a clip in the way on the track below
        let other = new_pattern_clip(&p, 0, 0.0, 2).unwrap();
        p.playlist.clips = vec![clip.clone(), other.clone()];
        let len = pattern_length(Some(&pat), p.time_sig);
        apply(&mut p, &PlEdit::SplitByChannel(clip.id.clone()));
        let made: Vec<&Clip> = p
            .playlist
            .clips
            .iter()
            .filter(|c| c.id != other.id)
            .collect();
        assert_eq!(made.len(), used.len());
        for (k, c) in made.iter().enumerate() {
            assert_eq!(c.track, 1 + k, "stacked from the clip's track down");
            assert_eq!((c.start, c.length), (clip.start, clip.length));
            let np = p.pattern(&c.ref_id).unwrap();
            assert_eq!(np.notes.len(), 1);
            assert_eq!(
                np.notes.get(&used[k]).map(Vec::len),
                pat.notes.get(&used[k]).map(Vec::len)
            );
            assert_eq!(
                pattern_length(Some(np), p.time_sig),
                len,
                "as long as before"
            );
        }
        let moved = p.playlist.clips.iter().find(|c| c.id == other.id).unwrap();
        assert!(moved.track > used.len(), "the clip in the way moved down");
        assert!(p.pattern(&pat.id).is_some(), "the old pattern stays");
        // a clip of a one-channel pattern stays as it is
        let id = made[0].id.clone();
        let before = p.playlist.clips.len();
        apply(&mut p, &PlEdit::SplitByChannel(id));
        assert_eq!(p.playlist.clips.len(), before);
    }

    #[test]
    fn markers_move_and_the_keys_jump_between_them() {
        let mut p = project();
        apply(&mut p, &PlEdit::AddMarker(384.0));
        apply(&mut p, &PlEdit::AddMarker(1536.0));
        let id = p.markers.as_ref().unwrap()[0].id.clone();
        apply(
            &mut p,
            &PlEdit::MoveMarker {
                id: id.clone(),
                tick: 3072.0,
            },
        );
        let m = p.markers.clone().unwrap();
        assert_eq!(m[1].id, id, "kept in order");
        assert_eq!((m[0].tick, m[1].tick), (1536.0, 3072.0));
        assert_eq!(marker_jump(&p, 0.0, true), Some(1536.0));
        assert_eq!(marker_jump(&p, 1536.0, true), Some(3072.0));
        assert_eq!(marker_jump(&p, 3072.0, true), None);
        assert_eq!(marker_jump(&p, 3000.0, false), Some(1536.0));
        assert_eq!(marker_jump(&p, 1536.0, false), None);
    }

    #[test]
    fn the_loop_keeps_its_range_while_off() {
        let mut s = PlaylistState {
            song_loop: Some((384.0, 768.0)),
            loop_on: true,
            ..Default::default()
        };
        assert_eq!(s.active_loop(), Some((384.0, 768.0)));
        s.loop_on = false;
        assert_eq!(s.active_loop(), None);
        assert_eq!(s.song_loop, Some((384.0, 768.0)));
    }

    #[test]
    fn all_clips_of_a_pattern_or_sound_are_found() {
        let mut p = las_project::create::default_project();
        let a = new_pattern_clip(&p, 0, 0.0, 0).unwrap();
        let b = new_pattern_clip(&p, 0, 384.0, 3).unwrap();
        let mut c = new_pattern_clip(&p, 0, 768.0, 0).unwrap();
        c.kind = ClipKind::Audio;
        p.playlist.clips = vec![a.clone(), b.clone(), c];
        assert_eq!(
            clips_like(&p, &a.id),
            HashSet::from([a.id.clone(), b.id.clone()])
        );
        assert!(clips_like(&p, "nope").is_empty());
    }

    #[test]
    fn patterns_are_cloned_and_deleted_from_the_picker() {
        let mut p = las_project::create::default_project();
        let id = p.patterns[0].id.clone();
        let n = p.patterns.len();
        apply(&mut p, &PlEdit::ClonePattern(id.clone()));
        assert_eq!(p.patterns.len(), n + 1);
        assert_eq!(p.patterns[1].notes.len(), p.patterns[0].notes.len());
        apply(&mut p, &PlEdit::DeletePattern(id.clone()));
        assert!(p.pattern(&id).is_none());
        assert!(p.playlist.clips.iter().all(|c| c.ref_id != id));
    }

    #[test]
    fn fit_zoom_shows_the_whole_song() {
        let p = project();
        let z = fit_zoom(&p, 1200.0);
        assert!((0.03..=0.6).contains(&z));
        let shown = f64::from((1200.0 - HEAD) / z);
        assert!(shown >= song_length(&p) || z == 0.03);
        assert!(fit_zoom(&p, 600.0) <= z);
    }

    #[test]
    fn markers_are_added_in_order_and_deleted() {
        let mut p = project();
        apply(&mut p, &PlEdit::AddMarker(384.0));
        apply(&mut p, &PlEdit::AddMarker(96.0));
        let m = p.markers.clone().unwrap();
        assert_eq!(m.iter().map(|m| m.tick).collect::<Vec<_>>(), [96.0, 384.0]);
        apply(&mut p, &PlEdit::DeleteMarker(m[0].id.clone()));
        apply(&mut p, &PlEdit::DeleteMarker(m[1].id.clone()));
        assert!(p.markers.is_none());
    }

    #[test]
    fn tracks_are_muted_and_made_when_needed() {
        let mut p = project();
        p.playlist.tracks.clear();
        apply(&mut p, &PlEdit::TrackMute(2));
        assert_eq!(p.playlist.tracks.len(), 3);
        assert!(p.playlist.tracks[2].mute && !p.playlist.tracks[0].mute);
        apply(&mut p, &PlEdit::TrackMute(2));
        assert!(!p.playlist.tracks[2].mute);
    }

    #[test]
    fn clips_are_muted_and_sliced() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut c = new_pattern_clip(&p, 0, 0.0, 0).unwrap();
        c.length = 384.0;
        let id = c.id.clone();
        p.playlist.clips.push(c);
        apply(&mut p, &PlEdit::Mute { id: id.clone() });
        assert_eq!(p.playlist.clips[0].muted, Some(true));
        apply(&mut p, &PlEdit::Mute { id: id.clone() });
        assert_eq!(p.playlist.clips[0].muted, Some(false));
        apply(
            &mut p,
            &PlEdit::Split {
                id: id.clone(),
                at: 96.0,
            },
        );
        let [a, b] = &p.playlist.clips[..] else {
            panic!("{:?}", p.playlist.clips.len())
        };
        assert_eq!((a.start, a.length, a.offset), (0.0, 96.0, 0.0));
        assert_eq!((b.start, b.length, b.offset), (96.0, 288.0, 96.0));
        assert_ne!(a.id, b.id);
        // a cut outside the clip, or at its edge, does nothing
        apply(
            &mut p,
            &PlEdit::Split {
                id: id.clone(),
                at: 0.0,
            },
        );
        apply(&mut p, &PlEdit::Split { id, at: 500.0 });
        assert_eq!(p.playlist.clips.len(), 2);
    }

    #[test]
    fn groups_of_clips_move_stay_in_shape_and_paste() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut a = new_pattern_clip(&p, 0, 96.0, 1).unwrap();
        let mut b = new_pattern_clip(&p, 0, 480.0, 3).unwrap();
        a.length = 96.0;
        b.length = 96.0;
        let ids = vec![a.id.clone(), b.id.clone()];
        p.playlist.clips.push(a);
        p.playlist.clips.push(b);
        apply(
            &mut p,
            &PlEdit::MoveMany {
                ids: ids.clone(),
                dstart: 96.0,
                dtrack: 1,
            },
        );
        let c = &p.playlist.clips;
        assert_eq!(
            (c[0].start, c[0].track, c[1].start, c[1].track),
            (192.0, 2, 576.0, 4)
        );
        // never before the start, never off the tracks
        apply(
            &mut p,
            &PlEdit::MoveMany {
                ids: ids.clone(),
                dstart: -9999.0,
                dtrack: -99,
            },
        );
        let c = &p.playlist.clips;
        assert_eq!(
            (c[0].start, c[0].track, c[1].start, c[1].track),
            (0.0, 0, 384.0, 2)
        );
        apply(
            &mut p,
            &PlEdit::MoveMany {
                ids: ids.clone(),
                dstart: 0.0,
                dtrack: 999,
            },
        );
        let last = track_count(&p) - 1;
        assert_eq!(p.playlist.clips[1].track, last);
        assert_eq!(p.playlist.clips[0].track, last - 2, "the shape is kept");
        let mut copy = p.playlist.clips[0].clone();
        copy.id = "new".into();
        apply(&mut p, &PlEdit::Paste { clips: vec![copy] });
        assert_eq!(p.playlist.clips.len(), 3);
        apply(&mut p, &PlEdit::DeleteMany { ids });
        assert_eq!(p.playlist.clips.len(), 1);
    }

    #[test]
    fn hits_find_the_clip_and_its_edge() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut c = new_pattern_clip(&p, 0, 0.0, 1).unwrap();
        c.length = 384.0;
        p.playlist.clips.push(c);
        assert_eq!(hit(&p, 100.0, 1, 20.0), Some((0, Part::Body)));
        assert_eq!(hit(&p, 380.0, 1, 20.0), Some((0, Part::Right)));
        assert_eq!(hit(&p, 10.0, 0, 20.0), None);
        assert_eq!(hit(&p, 384.0, 1, 20.0), None);
    }

    #[test]
    fn every_clip_of_the_fixture_has_a_name() {
        let p = project();
        for c in &p.playlist.clips {
            let (name, _) = describe(&p, c);
            assert!(!name.starts_with('('), "{name}");
        }
        assert!(new_pattern_clip(&p, 999, 0.0, 0).is_none());
    }

    #[test]
    fn the_playlist_draws() {
        let mut p = project();
        apply(&mut p, &PlEdit::AddMarker(0.0));
        let ctx = egui::Context::default();
        let mut state = PlaylistState::default();
        let mut out = Vec::new();
        for _ in 0..2 {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                out = draw(ui, &p, 0, 0.0, &mut state, Some(100.0));
            });
        }
        assert!(out.is_empty());
    }

    fn run(p: &Project, state: &mut PlaylistState, frames: Vec<Vec<egui::Event>>) -> Vec<PlEdit> {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 500.0));
        let mut all = Vec::new();
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                all.extend(draw(ui, p, 0, 0.0, state, None));
            });
        }
        all
    }

    fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn a_click_places_the_picked_pattern_on_the_snap() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut state = PlaylistState {
            show_picker: false,
            ..Default::default()
        };
        let at = egui::pos2(HEAD + 400.0 * 0.12 + 5.0, RULER + 44.0 * 1.5);
        let frames = vec![
            vec![egui::Event::PointerMoved(at)],
            vec![press(at, true)],
            vec![press(at, false)],
            vec![],
        ];
        let all = run(&p, &mut state, frames);
        let added: Vec<_> = all
            .iter()
            .filter_map(|e| match e {
                PlEdit::Add(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(added.len(), 1, "{all:?}");
        assert_eq!(added[0].track, 1);
        assert_eq!(added[0].start % state.snap, 0.0);
        assert_eq!(added[0].kind, ClipKind::Pattern);
    }

    #[test]
    fn the_paint_tool_places_clips_along_a_drag() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut state = PlaylistState {
            show_picker: false,
            tool: Tool::Paint,
            ..Default::default()
        };
        let y = RULER + 44.0 * 0.5;
        let a = egui::pos2(HEAD + 5.0, y);
        let b = egui::pos2(HEAD + 900.0, y);
        let frames = vec![
            vec![egui::Event::PointerMoved(a)],
            vec![press(a, true)],
            vec![egui::Event::PointerMoved(egui::pos2(a.x + 20.0, y))],
            vec![egui::Event::PointerMoved(b)],
            vec![press(b, false)],
            vec![],
        ];
        let all = run(&p, &mut state, frames);
        let n = all.iter().filter(|e| matches!(e, PlEdit::Add(_))).count();
        assert!(n >= 2, "{all:?}");
        assert_eq!(all.iter().filter(|e| matches!(e, PlEdit::Begin)).count(), 1);
    }

    #[test]
    fn paint_places_clips_end_to_end_and_skips_taken_places() {
        let p = project();
        let mut tpl = new_pattern_clip(&p, 0, 0.0, 0).unwrap();
        tpl.length = 384.0;
        // the press places one under the pointer, the drag one after another
        let (first, next) = paint_clips(&[], &tpl, None, 10.0, 2, 96.0);
        assert_eq!(first.iter().map(|c| c.start).collect::<Vec<_>>(), [0.0]);
        assert_eq!(next, (384.0, 2));
        let (got, next) = paint_clips(&first, &tpl, Some(next), 1000.0, 2, 96.0);
        assert_eq!(
            got.iter().map(|c| c.start).collect::<Vec<_>>(),
            [384.0, 768.0]
        );
        assert!(got.iter().all(|c| c.track == 2));
        assert_eq!(next, (1152.0, 2));
        // carrying on from there; a clip in the way is skipped
        let mut taken = tpl.clone();
        taken.start = 1152.0;
        taken.track = 2;
        let (more, _) = paint_clips(&[taken], &tpl, Some(next), 2000.0, 2, 96.0);
        assert_eq!(
            more.iter().map(|c| c.start).collect::<Vec<_>>(),
            [1536.0, 1920.0]
        );
        // another track starts on the snap under the pointer
        let (other, _) = paint_clips(&[], &tpl, Some(next), 500.0, 3, 96.0);
        assert_eq!(other[0].start, 480.0);
    }

    #[test]
    fn ctrl_b_duplicates_clips_after_themselves_and_the_keys_work() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut a = new_pattern_clip(&p, 0, 96.0, 0).unwrap();
        a.length = 192.0;
        let mut b = new_pattern_clip(&p, 0, 192.0, 1).unwrap();
        b.length = 192.0;
        p.playlist.clips = vec![a.clone(), b.clone()];
        let d = duplicate_clips(&p.playlist.clips, &HashSet::new());
        // 96 to 384: a span of 288
        assert_eq!(
            d.iter().map(|c| (c.start, c.track)).collect::<Vec<_>>(),
            [(384.0, 0), (480.0, 1)]
        );
        assert!(d.iter().all(|c| c.id != a.id && c.id != b.id));
        let mut st = PlaylistState::default();
        let mut e = Vec::new();
        let k = |f: fn(&mut crate::roll::Keys)| {
            let mut k = crate::roll::Keys::default();
            f(&mut k);
            k
        };
        keys_edit(
            k(|k| k.all = true),
            &p.playlist.clips,
            0.0,
            None,
            &mut st,
            &mut e,
        );
        assert_eq!(st.selected.len(), 2);
        keys_edit(
            k(|k| k.duplicate = true),
            &p.playlist.clips,
            0.0,
            None,
            &mut st,
            &mut e,
        );
        assert!(matches!(&e[..], [PlEdit::Begin, PlEdit::Paste { clips }] if clips.len() == 2));
        e.clear();
        st.selected = HashSet::from([a.id.clone()]);
        assert!(keys_edit(
            k(|k| k.copy = true),
            &p.playlist.clips,
            0.0,
            None,
            &mut st,
            &mut e
        ));
        keys_edit(
            k(|k| k.paste = true),
            &p.playlist.clips,
            0.0,
            None,
            &mut st,
            &mut e,
        );
        let Some(PlEdit::Paste { clips }) = e.last() else {
            panic!("{e:?}")
        };
        assert_eq!(clips[0].start, 288.0, "right after the selection");
        e.clear();
        st.selected.clear();
        keys_edit(
            k(|k| k.paste = true),
            &p.playlist.clips,
            0.0,
            Some(768.0),
            &mut st,
            &mut e,
        );
        let Some(PlEdit::Paste { clips }) = e.last() else {
            panic!("{e:?}")
        };
        assert_eq!(clips[0].start, 768.0, "at the playhead");
        e.clear();
        st.selected = HashSet::from([b.id.clone()]);
        keys_edit(
            k(|k| k.delete = true),
            &p.playlist.clips,
            0.0,
            None,
            &mut st,
            &mut e,
        );
        assert!(matches!(&e[1], PlEdit::DeleteMany { ids } if ids == &vec![b.id.clone()]));
        st.selected = HashSet::from([b.id.clone()]);
        keys_edit(
            k(|k| k.deselect = true),
            &p.playlist.clips,
            0.0,
            None,
            &mut st,
            &mut e,
        );
        assert!(st.selected.is_empty());
    }

    #[test]
    fn the_left_edge_trims_the_start_and_keeps_the_place_in_the_sound() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut c = new_pattern_clip(&p, 0, 384.0, 0).unwrap();
        c.length = 384.0;
        c.kind = ClipKind::Audio;
        let id = c.id.clone();
        p.playlist.clips.push(c);
        apply(
            &mut p,
            &PlEdit::Trim {
                id: id.clone(),
                start: 480.0,
            },
        );
        let c = &p.playlist.clips[0];
        assert_eq!((c.start, c.length, c.offset), (480.0, 288.0, 96.0));
        // back out to the left: never before the start of the sound
        apply(
            &mut p,
            &PlEdit::Trim {
                id: id.clone(),
                start: 0.0,
            },
        );
        let c = &p.playlist.clips[0];
        assert_eq!((c.start, c.length, c.offset), (384.0, 384.0, 0.0));
        // and never past the right edge
        apply(
            &mut p,
            &PlEdit::Trim {
                id: id.clone(),
                start: 5000.0,
            },
        );
        let c = &p.playlist.clips[0];
        assert_eq!((c.start + c.length, c.length), (768.0, 1.0));
    }

    #[test]
    fn hits_tell_the_left_edge_the_body_and_the_right_edge() {
        let mut p = project();
        p.playlist.clips.clear();
        let mut c = new_pattern_clip(&p, 0, 0.0, 1).unwrap();
        c.length = 384.0;
        p.playlist.clips.push(c);
        assert_eq!(hit(&p, 5.0, 1, 20.0), Some((0, Part::Left)));
        assert_eq!(hit(&p, 100.0, 1, 20.0), Some((0, Part::Body)));
        assert_eq!(hit(&p, 380.0, 1, 20.0), Some((0, Part::Right)));
    }

    #[test]
    fn tracks_solo_take_colours_and_are_inserted_and_deleted() {
        let mut p = project();
        p.playlist.clips.clear();
        p.playlist.tracks.clear();
        let mut a = new_pattern_clip(&p, 0, 0.0, 0).unwrap();
        a.length = 96.0;
        let mut b = a.clone();
        b.id = "b".into();
        b.track = 2;
        p.playlist.clips = vec![a, b];
        apply(&mut p, &PlEdit::TrackSolo(2));
        assert!(p.playlist.tracks[2].solo);
        assert_eq!(p.playlist.silent_tracks(), vec![true, true, false]);
        apply(&mut p, &PlEdit::TrackColour(1, "#3fbf7f".into()));
        assert_eq!(p.playlist.tracks[1].color, "#3fbf7f");
        apply(&mut p, &PlEdit::InsertTrack(1));
        assert_eq!(p.playlist.tracks.len(), 4);
        assert_eq!(p.playlist.clips[0].track, 0);
        assert_eq!(p.playlist.clips[1].track, 3, "the clips below move down");
        assert!(
            p.playlist.tracks[3].solo,
            "the track's settings move with it"
        );
        apply(&mut p, &PlEdit::DeleteTrack(0));
        assert_eq!(p.playlist.tracks.len(), 3);
        assert_eq!(p.playlist.clips.len(), 1, "its clips go with it");
        assert_eq!(p.playlist.clips[0].track, 2);
        apply(&mut p, &PlEdit::TrackSolo(2));
        assert!(!p.playlist.tracks[2].solo);
    }

    #[test]
    fn the_picker_sources_make_clips_as_long_as_what_they_play() {
        let p = project();
        let pat = source_clip(&p, &Source::Pattern, 0, 96.0, 1).unwrap();
        assert_eq!(
            (pat.kind, pat.start, pat.track),
            (ClipKind::Pattern, 96.0, 1)
        );
        if let Some(a) = p.automation.first() {
            let c = source_clip(&p, &Source::Automation(a.id.clone()), 0, 0.0, 0).unwrap();
            assert_eq!(
                (c.kind, c.length),
                (ClipKind::Automation, a.length.max(1.0))
            );
        }
        if let Some(ch) = p.channels.iter().find(|c| c.plugin_id == "audioclip") {
            let c = source_clip(&p, &Source::Audio(ch.id.clone()), 0, 0.0, 0).unwrap();
            assert_eq!(c.kind, ClipKind::Audio);
            assert!(c.length >= 1.0);
        }
        assert!(source_clip(&p, &Source::Automation("gone".into()), 0, 0.0, 0).is_none());
    }
}
