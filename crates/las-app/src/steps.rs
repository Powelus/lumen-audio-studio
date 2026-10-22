// SPDX-License-Identifier: GPL-3.0-or-later
//! The channel rack as a step sequencer, like the web version's: one row per channel
//! with its mute light, level and pan knobs, name, and a cell for every step of the
//! pattern. Like the other editors, drawing reports what was asked for.

use crate::rack::channel_colour;
use crate::roll::{new_note, Edit};
use crate::theme::pal;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui};
use las_engine::timeline::pattern_length;
use las_project::{Note, Pattern, Project, TICKS_PER_STEP};

const STEP: f32 = 24.0;
const ROW: f32 = 33.0;
const NAME: f32 = 130.0;
/// Height of the graph editor's row.
const GRAPH: f32 = 64.0;

/// A change to a channel's settings (level, pan, mute).
#[derive(Clone, Debug, PartialEq)]
pub enum ChanEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    Volume {
        channel: usize,
        value: f64,
    },
    Pan {
        channel: usize,
        value: f64,
    },
    Mute {
        channel: usize,
        on: bool,
    },
    /// Make an automation clip for a parameter of a channel.
    Automate(crate::auto::AutoRequest),
    /// Remove a channel, with its notes in every pattern.
    Delete(usize),
    /// A copy of a channel (with its notes) right after it.
    Clone(usize),
    /// Play the channel through another insert.
    Route {
        channel: usize,
        insert: usize,
    },
    Color {
        channel: usize,
        color: String,
    },
    /// The channel's own swing (0 to 1), or none to follow the project's.
    Swing {
        channel: usize,
        value: Option<f64>,
    },
    /// Ask for a new name for a channel.
    AskRename(usize),
    /// Let the channel sleep while it has nothing to play (FL's smart disable).
    SmartDisable {
        channel: usize,
        on: bool,
    },
    /// Put a channel in a group (`None`: unsorted).
    Group {
        channel: usize,
        group: Option<String>,
    },
}

/// Carry out a channel edit.
pub fn apply(project: &mut Project, e: &ChanEdit) {
    match e {
        ChanEdit::Begin => {}
        ChanEdit::Volume { channel, value } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.volume = value.clamp(0.0, 1.25);
            }
        }
        ChanEdit::Pan { channel, value } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.pan = value.clamp(-1.0, 1.0);
            }
        }
        ChanEdit::Mute { channel, on } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.mute = *on;
            }
        }
        ChanEdit::Automate(_) | ChanEdit::AskRename(_) => {}
        ChanEdit::Delete(i) => {
            if *i < project.channels.len() {
                let id = project.channels.remove(*i).id;
                for pat in &mut project.patterns {
                    pat.notes.remove(&id);
                }
            }
        }
        ChanEdit::Clone(i) => {
            if let Some(src) = project.channels.get(*i).cloned() {
                let mut c = src.clone();
                c.id = las_project::create::uid("ch_");
                c.name = format!("{} copy", src.name);
                for pat in &mut project.patterns {
                    if let Some(notes) = pat.notes.get(&src.id).cloned() {
                        let copy = notes
                            .into_iter()
                            .map(|mut n| {
                                n.id = las_project::create::uid("n");
                                n
                            })
                            .collect();
                        pat.notes.insert(c.id.clone(), copy);
                    }
                }
                project.channels.insert(*i + 1, c);
            }
        }
        ChanEdit::Route { channel, insert } => {
            if *insert < project.mixer.len() {
                if let Some(c) = project.channels.get_mut(*channel) {
                    c.mixer_track = *insert;
                }
            }
        }
        ChanEdit::Color { channel, color } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.color = color.clone();
            }
        }
        ChanEdit::Group { channel, group } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.group = group
                    .as_ref()
                    .map(|g| g.trim().to_string())
                    .filter(|g| !g.is_empty());
            }
        }
        ChanEdit::Swing { channel, value } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                c.swing = value.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0));
            }
        }
        ChanEdit::SmartDisable { channel, on } => {
            if let Some(c) = project.channels.get_mut(*channel) {
                set_smart(&mut c.extra, *on);
            }
        }
    }
}

/// Smart disable is on unless the project says `"smartDisable": false`.
pub(crate) fn set_smart(extra: &mut las_project::Extra, on: bool) {
    if on {
        extra.remove("smartDisable");
    } else {
        extra.insert("smartDisable".into(), serde_json::Value::Bool(false));
    }
}

/// Which channels the rack shows, like FL's channel filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum GroupFilter {
    #[default]
    All,
    /// Channels in no group.
    Unsorted,
    Named(String),
}

impl GroupFilter {
    pub fn shows(&self, c: &las_project::Channel) -> bool {
        let g = c.group.as_deref().filter(|g| !g.is_empty());
        match self {
            GroupFilter::All => true,
            GroupFilter::Unsorted => g.is_none(),
            GroupFilter::Named(n) => g == Some(n.as_str()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            GroupFilter::All => "All".into(),
            GroupFilter::Unsorted => "Unsorted".into(),
            GroupFilter::Named(n) => n.clone(),
        }
    }
}

/// The groups the project's channels are in, sorted.
pub fn groups(project: &Project) -> Vec<String> {
    let mut v: Vec<String> = project
        .channels
        .iter()
        .filter_map(|c| c.group.clone())
        .filter(|g| !g.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// What the graph editor under the selected channel's steps edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphParam {
    #[default]
    Velocity,
    Pan,
    Fine,
}

impl GraphParam {
    pub const ALL: [GraphParam; 3] = [GraphParam::Velocity, GraphParam::Pan, GraphParam::Fine];

    pub fn label(self) -> &'static str {
        match self {
            GraphParam::Velocity => "Velocity",
            GraphParam::Pan => "Pan",
            GraphParam::Fine => "Fine pitch",
        }
    }

    /// Whether the bars grow from the middle (pan, fine pitch) or the bottom.
    pub fn centred(self) -> bool {
        self != GraphParam::Velocity
    }

    /// A note's value as a height from 0 (bottom) to 1 (top).
    pub fn height(self, n: &Note) -> f64 {
        match self {
            GraphParam::Velocity => n.vel.clamp(0.0, 1.0),
            GraphParam::Pan => (n.pan.clamp(-1.0, 1.0) + 1.0) / 2.0,
            GraphParam::Fine => (n.fine.unwrap_or(0.0).clamp(-100.0, 100.0) + 100.0) / 200.0,
        }
    }

    /// The edit that sets a note to a height from 0 (bottom) to 1 (top); `None` for
    /// the default (what a right click gives).
    pub fn edit(self, id: &str, height: Option<f64>) -> Edit {
        let h = height.map(|h| h.clamp(0.0, 1.0));
        let id = id.to_string();
        match self {
            GraphParam::Velocity => Edit::Velocity {
                id,
                vel: h.unwrap_or(0.78),
            },
            GraphParam::Pan => Edit::NotePan {
                id,
                pan: h.map_or(0.0, |h| h * 2.0 - 1.0),
            },
            GraphParam::Fine => Edit::Fine {
                id,
                cents: h.map_or(0.0, |h| (h * 200.0 - 100.0).round()),
            },
        }
    }

    /// What the value reads as.
    pub fn describe(self, n: &Note) -> String {
        match self {
            GraphParam::Velocity => format!("{:.0}%", n.vel * 100.0),
            GraphParam::Pan => {
                let v = (n.pan * 100.0).round();
                if v == 0.0 {
                    "Centred".into()
                } else if v < 0.0 {
                    format!("{:.0}% left", -v)
                } else {
                    format!("{v:.0}% right")
                }
            }
            GraphParam::Fine => format!("{:+.0} cents", n.fine.unwrap_or(0.0)),
        }
    }
}

/// Steps a pattern can be set to from the rack's menu.
pub const STEP_CHOICES: [usize; 8] = [8, 12, 16, 24, 32, 48, 64, 128];

/// What the rack shows and does between frames.
#[derive(Default)]
pub struct StepState {
    /// While the mouse paints across cells: whether it adds (`true`) or removes.
    painting: Option<bool>,
    /// The graph editor is shown under the selected channel.
    pub graph: bool,
    pub graph_param: GraphParam,
    /// Which channels are shown.
    pub filter: GroupFilter,
    /// The name typed for a new group.
    new_group: String,
    /// Where each shown channel's row was drawn (for things dropped on it).
    pub rows: Vec<(usize, Rect)>,
}

impl StepState {
    /// The channel whose row is under `pos`, from the last frame.
    pub fn row_at(&self, pos: egui::Pos2) -> Option<usize> {
        self.rows
            .iter()
            .find(|(_, r)| r.contains(pos))
            .map(|(i, _)| *i)
    }
}

/// The left of step `s`'s cell, from the left of the steps.
fn cell_offset(s: usize) -> f32 {
    s as f32 * (STEP + 3.0) + (s / 4) as f32 * 4.0
}

/// The step under `x`, from the left of the steps.
fn cell_under(x: f32, steps: usize) -> Option<usize> {
    (0..steps).find(|&s| {
        let l = cell_offset(s);
        x >= l && x < l + STEP + 3.0
    })
}

/// The edits that solo a channel the way FL's Alt+click on its light does: it alone
/// plays; done again on the only one playing, every channel plays.
pub fn solo(project: &Project, channel: usize) -> Vec<ChanEdit> {
    if channel >= project.channels.len() {
        return Vec::new();
    }
    let alone = project
        .channels
        .iter()
        .enumerate()
        .all(|(i, c)| c.mute == (i != channel));
    project
        .channels
        .iter()
        .enumerate()
        .map(|(i, _)| ChanEdit::Mute {
            channel: i,
            on: !alone && i != channel,
        })
        .collect()
}

/// The step note of a channel at a step, if there is one.
fn step_note(notes: &[Note], step: usize) -> Option<&Note> {
    let pos = step as f64 * f64::from(TICKS_PER_STEP);
    notes
        .iter()
        .find(|n| n.step == Some(true) && (n.pos - pos).abs() < 0.5)
}

/// The edit that toggles a step: remove the note there, or place one.
pub fn toggle(notes: &[Note], step: usize) -> Edit {
    match step_note(notes, step) {
        Some(n) => Edit::Delete { id: n.id.clone() },
        None => {
            let mut n = new_note(
                60,
                step as f64 * f64::from(TICKS_PER_STEP),
                f64::from(TICKS_PER_STEP),
            );
            n.step = Some(true);
            Edit::Add(n)
        }
    }
}

/// The edits that leave a channel with a step on every `every`th step of `steps` (FL's
/// "Fill each N steps"); with `every` 0, the edits that clear its steps. Notes that were
/// not entered with the step sequencer are left as they are.
pub fn fill_steps(notes: &[Note], steps: usize, every: usize) -> Vec<Edit> {
    let mut out: Vec<Edit> = notes
        .iter()
        .filter(|n| n.step == Some(true))
        .map(|n| Edit::Delete { id: n.id.clone() })
        .collect();
    if every > 0 {
        for s in (0..steps).step_by(every) {
            let mut n = new_note(
                60,
                s as f64 * f64::from(TICKS_PER_STEP),
                f64::from(TICKS_PER_STEP),
            );
            n.step = Some(true);
            out.push(Edit::Add(n));
        }
    }
    out
}

/// The edits that leave a channel with steps on about `density` (0 to 1) of its `steps`,
/// picked from `seed` (the same seed always picks the same steps).
pub fn random_steps(notes: &[Note], steps: usize, density: f64, seed: u64) -> Vec<Edit> {
    let mut out = fill_steps(notes, 0, 0);
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for s in 0..steps {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if ((x >> 11) as f64 / (1u64 << 53) as f64) < density {
            let mut n = new_note(
                60,
                s as f64 * f64::from(TICKS_PER_STEP),
                f64::from(TICKS_PER_STEP),
            );
            n.step = Some(true);
            out.push(Edit::Add(n));
        }
    }
    out
}

/// The edits that rotate a channel's steps by `by` steps (negative is left) around a
/// pattern of `steps` steps, as FL's shift left and right do.
pub fn shift_steps(notes: &[Note], steps: usize, by: i64) -> Vec<Edit> {
    let n = steps.max(1) as i64;
    let tps = f64::from(TICKS_PER_STEP);
    notes
        .iter()
        .filter(|x| x.step == Some(true))
        .map(|x| {
            let i = (x.pos / tps).round() as i64;
            Edit::Move {
                id: x.id.clone(),
                pos: ((i + by).rem_euclid(n)) as f64 * tps,
                key: x.key,
            }
        })
        .collect()
}

/// A small rotary knob: drag up and down, double-click to reset. Returns
/// (changed, gesture started).
pub fn mini_knob(
    ui: &mut Ui,
    v: &mut f64,
    range: (f64, f64),
    default: f64,
    tip: &str,
    colour: Color32,
) -> (bool, bool) {
    let (changed, started, _) = mini_knob_resp(ui, v, range, default, tip, colour);
    (changed, started)
}

/// As [`mini_knob`], also giving the knob's response (for a context menu).
pub fn mini_knob_resp(
    ui: &mut Ui,
    v: &mut f64,
    range: (f64, f64),
    default: f64,
    tip: &str,
    colour: Color32,
) -> (bool, bool, egui::Response) {
    let (changed, started, resp) = knob_core(ui, v, range, default, 22.0, colour);
    let resp = resp.on_hover_text(format!("{tip}: {:.0}%", *v * 100.0));
    (changed, started, resp)
}

/// A rotary knob `size` across with no tooltip of its own: drag up and down,
/// double-click for `default`. Returns (changed, gesture started, response).
pub fn knob_core(
    ui: &mut Ui,
    v: &mut f64,
    range: (f64, f64),
    default: f64,
    size: f32,
    colour: Color32,
) -> (bool, bool, egui::Response) {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let mut changed = false;
    if resp.dragged() {
        let d = -f64::from(resp.drag_delta().y) / 110.0 * (range.1 - range.0);
        let before = *v;
        *v = (*v + d).clamp(range.0, range.1);
        changed = *v != before;
    }
    let reset = resp.double_clicked();
    if reset {
        *v = default;
        changed = true;
    }
    let p = pal();
    let painter = ui.painter_at(r);
    let c = r.center();
    let rad = size / 2.0 - 1.0;
    painter.circle_filled(c, rad, p.well_deep);
    painter.circle_stroke(c, rad, Stroke::new(1.0, p.border2));
    let t = ((*v - range.0) / (range.1 - range.0)).clamp(0.0, 1.0) as f32;
    let ang = (-135.0 + 270.0 * t).to_radians();
    let tip_pt = pos2(c.x + ang.sin() * rad * 0.75, c.y - ang.cos() * rad * 0.75);
    painter.line_segment([c, tip_pt], Stroke::new(2.0, colour));
    (changed, resp.drag_started() || reset, resp)
}

/// What the rack asks for in one frame.
#[derive(Default)]
pub struct Out {
    /// Note edits, with the channel each belongs to.
    pub notes: Vec<(usize, Edit)>,
    pub chans: Vec<ChanEdit>,
    /// A channel whose name was right-clicked: open its piano roll.
    pub open_roll: Option<usize>,
    /// A channel whose name was double-clicked: open its instrument.
    pub open_synth: Option<usize>,
    /// A channel whose Channel settings (Arp and chords) were asked for.
    pub open_settings: Option<usize>,
    /// A new length for the pattern, in steps (0: as long as its notes).
    pub steps: Option<usize>,
}

/// The menu of mixer inserts a channel can play through.
fn insert_menu(ui: &mut Ui, project: &Project, i: usize, out: &mut Out) {
    let current = project.channels[i].mixer_track;
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            for (n, t) in project.mixer.iter().enumerate() {
                let label = if n == 0 {
                    "Master".to_string()
                } else if t.name.is_empty() {
                    format!("{n}  Insert {n}")
                } else {
                    format!("{n}  {}", t.name)
                };
                if ui.selectable_label(current == n, label).clicked() {
                    out.chans.push(ChanEdit::Begin);
                    out.chans.push(ChanEdit::Route {
                        channel: i,
                        insert: n,
                    });
                    ui.close();
                }
            }
        });
}

/// The bar above the channels: the group filter, the pattern's steps and the graph
/// editor's switch.
fn top_bar(
    ui: &mut Ui,
    project: &Project,
    steps: usize,
    explicit: bool,
    state: &mut StepState,
    out: &mut Out,
) {
    let p = pal();
    ui.horizontal(|ui| {
        ui.label(RichText::new("SHOW").size(10.0).color(p.muted));
        let groups = groups(project);
        if let GroupFilter::Named(n) = &state.filter {
            if !groups.contains(n) {
                state.filter = GroupFilter::All;
            }
        }
        egui::ComboBox::from_id_salt("rack-filter")
            .selected_text(state.filter.label())
            .width(110.0)
            .show_ui(ui, |ui| {
                let mut all = vec![GroupFilter::All, GroupFilter::Unsorted];
                all.extend(groups.iter().cloned().map(GroupFilter::Named));
                for f in all {
                    let label = f.label();
                    ui.selectable_value(&mut state.filter, f, label);
                }
            })
            .response
            .on_hover_text(
                "Which channels the rack shows. Put a channel in a group from its right-click menu.",
            );
        ui.separator();
        ui.label(RichText::new("STEPS").size(10.0).color(p.muted));
        let text = if explicit {
            steps.to_string()
        } else {
            format!("{steps} (auto)")
        };
        egui::ComboBox::from_id_salt("rack-steps")
            .selected_text(text)
            .width(80.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(!explicit, "Auto")
                    .on_hover_text("As long as the notes, in whole bars")
                    .clicked()
                {
                    out.steps = Some(0);
                }
                for n in STEP_CHOICES {
                    if ui.selectable_label(explicit && steps == n, n.to_string()).clicked() {
                        out.steps = Some(n);
                    }
                }
            })
            .response
            .on_hover_text("How many steps the pattern has");
        let mut any = steps;
        if ui
            .add(egui::DragValue::new(&mut any).range(1..=256).speed(0.2))
            .on_hover_text("Any number of steps: drag or type")
            .changed()
        {
            out.steps = Some(any);
        }
        ui.separator();
        let g = ui
            .selectable_label(state.graph, "Graph editor")
            .on_hover_text(
                "The graph editor: bars under the selected channel's steps for each step's velocity, pan and fine pitch",
            );
        if g.clicked() {
            state.graph = !state.graph;
        }
        if state.graph {
            for gp in GraphParam::ALL {
                ui.selectable_value(&mut state.graph_param, gp, gp.label());
            }
        }
    });
}

pub fn draw(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    selected: &mut usize,
    state: &mut StepState,
    playhead: Option<f64>,
) -> Out {
    let mut out = Out::default();
    let p = pal();
    let pat: Option<&Pattern> = project.patterns.get(pattern);
    let ticks = pattern_length(pat, project.time_sig);
    let explicit = pat.is_some_and(|x| x.length > 0.0);
    let steps = ((ticks / f64::from(TICKS_PER_STEP)).round() as usize)
        .clamp(if explicit { 1 } else { 16 }, 256);
    let empty: Vec<Note> = Vec::new();
    top_bar(ui, project, steps, explicit, state, &mut out);
    state.rows.clear();
    let dragging = egui::DragAndDrop::has_payload_of_type::<crate::shell::Dragged>(ui.ctx());
    let width = cell_offset(steps);
    egui::ScrollArea::horizontal()
        .auto_shrink([false, true])
        .id_salt("steps")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            let mut shown = 0;
            for (i, c) in project.channels.iter().enumerate() {
                if !state.filter.shows(c) {
                    continue;
                }
                shown += 1;
                let notes = pat.and_then(|x| x.notes.get(&c.id)).unwrap_or(&empty);
                let col = channel_colour(c);
                let mut steps_left = 0.0;
                let row = ui.horizontal(|ui| {
                    // mute light; Alt+click solos
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, ROW), Sense::click());
                    let lit = !c.mute;
                    ui.painter()
                        .circle_filled(r.center(), 6.0, if lit { p.ok } else { p.led_off });
                    if resp
                        .on_hover_text("Mute. Alt+click: solo (only this channel plays)")
                        .clicked()
                    {
                        out.chans.push(ChanEdit::Begin);
                        if ui.input(|i| i.modifiers.alt) {
                            out.chans.extend(solo(project, i));
                        } else {
                            out.chans.push(ChanEdit::Mute {
                                channel: i,
                                on: !c.mute,
                            });
                        }
                    }
                    // volume and pan
                    let mut vol = c.volume;
                    let (ch, start, kr) =
                        mini_knob_resp(ui, &mut vol, (0.0, 1.25), 1.0, "Level", p.text2);
                    kr.context_menu(|ui| {
                        let req = || crate::auto::AutoRequest {
                            target: crate::auto::channel_target(&c.id, "volume"),
                            name: format!("{} level", c.name),
                            start: c.volume / 1.25,
                        };
                        if ui.button("Create automation clip").clicked() {
                            out.chans.push(ChanEdit::Automate(req()));
                            ui.close();
                        }
                        crate::midilearn::menu_item(ui, req);
                    });
                    if start {
                        out.chans.push(ChanEdit::Begin);
                    }
                    if ch {
                        out.chans.push(ChanEdit::Volume {
                            channel: i,
                            value: vol,
                        });
                    }
                    let mut pan = c.pan;
                    let (ch, start, kr) =
                        mini_knob_resp(ui, &mut pan, (-1.0, 1.0), 0.0, "Pan", p.accent);
                    kr.context_menu(|ui| {
                        let req = || crate::auto::AutoRequest {
                            target: crate::auto::channel_target(&c.id, "pan"),
                            name: format!("{} pan", c.name),
                            start: (c.pan + 1.0) / 2.0,
                        };
                        if ui.button("Create automation clip").clicked() {
                            out.chans.push(ChanEdit::Automate(req()));
                            ui.close();
                        }
                        crate::midilearn::menu_item(ui, req);
                    });
                    if start {
                        out.chans.push(ChanEdit::Begin);
                    }
                    if ch {
                        out.chans.push(ChanEdit::Pan {
                            channel: i,
                            value: pan,
                        });
                    }
                    // the mixer insert it plays through (FL's target mixer track)
                    let (nr, nresp) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::click());
                    let hot = nresp.hovered();
                    ui.painter()
                        .rect_filled(nr, 4.0, if hot { p.raised2 } else { p.well_deep });
                    ui.painter().text(
                        nr.center(),
                        Align2::CENTER_CENTER,
                        if c.mixer_track == 0 {
                            "M".to_string()
                        } else {
                            c.mixer_track.to_string()
                        },
                        FontId::proportional(11.0),
                        if hot { p.text } else { p.muted },
                    );
                    egui::Popup::menu(&nresp).show(|ui| {
                        ui.label(RichText::new("Play through insert").strong());
                        insert_menu(ui, project, i, &mut out);
                    });
                    let _ = nresp.on_hover_text(format!(
                        "Mixer insert: {}. Click to pick another.",
                        if c.mixer_track == 0 {
                            "Master".to_string()
                        } else {
                            c.mixer_track.to_string()
                        }
                    ));
                    // name
                    let (nr, resp) = ui.allocate_exact_size(vec2(NAME, 26.0), Sense::click());
                    let sel = *selected == i;
                    let bg = if sel { p.raised2 } else { p.raised };
                    ui.painter().rect_filled(nr, 6.0, bg);
                    if sel {
                        ui.painter().rect_stroke(
                            nr,
                            6.0,
                            Stroke::new(1.5, p.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    ui.painter().rect_filled(
                        Rect::from_min_size(nr.min + vec2(0.0, 4.0), vec2(4.0, nr.height() - 8.0)),
                        2.0,
                        col,
                    );
                    ui.painter().with_clip_rect(nr.shrink(2.0)).text(
                        nr.left_center() + vec2(14.0, 0.0),
                        Align2::LEFT_CENTER,
                        &c.name,
                        FontId::proportional(13.0),
                        p.text,
                    );
                    if resp.clicked() {
                        *selected = i;
                    }
                    if resp.double_clicked() {
                        if c.plugin_id == "audioclip" {
                            // as FL opens an audio clip's sound in Edison
                            crate::sampleedit::request(crate::sampleedit::Request::Edit(
                                c.id.clone(),
                            ));
                        } else {
                            out.open_synth = Some(i);
                        }
                    }
                    let resp = resp.on_hover_text("Double click: instrument. Right click: more.");
                    resp.context_menu(|ui| {
                        channel_menu(ui, project, i, notes, steps, state, &mut out);
                    });
                    // the steps
                    let (area, resp) =
                        ui.allocate_exact_size(vec2(width, ROW - 4.0), Sense::click_and_drag());
                    steps_left = area.left();
                    let cell_at = |x: f32| cell_under(x - area.left(), steps);
                    let painter = ui.painter_at(area);
                    for s in 0..steps {
                        let on = step_note(notes, s).is_some();
                        let cell = Rect::from_min_size(
                            pos2(area.left() + cell_offset(s), area.top() + 2.0),
                            vec2(STEP, area.height() - 4.0),
                        );
                        let fill = if on {
                            col
                        } else if (s / 4) % 2 == 1 {
                            p.step_off_alt
                        } else {
                            p.step_off
                        };
                        painter.rect_filled(cell, 4.0, fill);
                        if on {
                            painter.rect_stroke(
                                cell,
                                4.0,
                                Stroke::new(1.0, Color32::from_white_alpha(70)),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                    if let Some(t) = playhead {
                        let step = t / f64::from(TICKS_PER_STEP);
                        let whole = step.floor() as usize;
                        let x = area.left() + (step as f32) * (STEP + 3.0) + (whole / 4) as f32 * 4.0;
                        painter.line_segment(
                            [pos2(x, area.top()), pos2(x, area.bottom())],
                            Stroke::new(1.5, Color32::WHITE),
                        );
                    }
                    // click toggles, dragging paints
                    let primary = ui.input(|i| i.pointer.primary_down());
                    if resp.drag_started_by(egui::PointerButton::Primary)
                        || resp.clicked_by(egui::PointerButton::Primary)
                    {
                        if let Some(s) = resp.interact_pointer_pos().and_then(|q| cell_at(q.x)) {
                            let e = toggle(notes, s);
                            state.painting = Some(matches!(e, Edit::Add(_)));
                            out.notes.push((i, Edit::Begin));
                            out.notes.push((i, e));
                            *selected = i;
                        }
                    } else if resp.dragged_by(egui::PointerButton::Primary) && primary {
                        if let (Some(add), Some(s)) = (
                            state.painting,
                            resp.interact_pointer_pos().and_then(|q| cell_at(q.x)),
                        ) {
                            let has = step_note(notes, s).is_some();
                            if add != has {
                                out.notes.push((i, toggle(notes, s)));
                            }
                        }
                    }
                    if resp.drag_stopped() {
                        state.painting = None;
                    }
                    // the selection light, as FL's green one
                    let (lr, lresp) = ui.allocate_exact_size(vec2(10.0, 22.0), Sense::click());
                    ui.painter().rect_filled(
                        lr,
                        2.0,
                        if sel { p.ok } else { p.led_off },
                    );
                    if lresp
                        .on_hover_text("Select this channel (the graph editor, keyboard and Route to insert follow it)")
                        .clicked()
                    {
                        *selected = i;
                    }
                });
                let rr = row.response.rect;
                state.rows.push((i, rr));
                if dragging && ui.rect_contains_pointer(rr) {
                    ui.painter().rect_stroke(
                        rr.expand(1.0),
                        6.0,
                        Stroke::new(1.5, p.accent),
                        egui::StrokeKind::Outside,
                    );
                }
                if state.graph && *selected == i {
                    graph_row(ui, notes, i, steps, steps_left, state.graph_param, col, &mut out);
                }
            }
            if shown == 0 {
                ui.label(
                    RichText::new(format!("No channels in {}.", state.filter.label())).weak(),
                );
            }
        });
    out
}

/// The graph editor under a channel's steps: a bar for each step, dragged to set.
#[allow(clippy::too_many_arguments)]
fn graph_row(
    ui: &mut Ui,
    notes: &[Note],
    i: usize,
    steps: usize,
    steps_left: f32,
    param: GraphParam,
    col: Color32,
    out: &mut Out,
) {
    let p = pal();
    ui.horizontal(|ui| {
        let lead = (steps_left - ui.cursor().left() - 6.0).max(0.0);
        let (lr, _) = ui.allocate_exact_size(vec2(lead, GRAPH), Sense::hover());
        ui.painter().text(
            lr.right_center() - vec2(8.0, 0.0),
            Align2::RIGHT_CENTER,
            param.label(),
            FontId::proportional(11.0),
            p.muted,
        );
        let width = cell_offset(steps);
        let (area, resp) = ui.allocate_exact_size(vec2(width, GRAPH), Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 4.0, p.well_deep);
        if param.centred() {
            painter.line_segment(
                [
                    pos2(area.left(), area.center().y),
                    pos2(area.right(), area.center().y),
                ],
                Stroke::new(1.0, p.border2),
            );
        }
        let inner = area.shrink2(vec2(0.0, 3.0));
        for s in 0..steps {
            let Some(n) = step_note(notes, s) else {
                continue;
            };
            let x = area.left() + cell_offset(s);
            let h = param.height(n) as f32;
            let y = inner.bottom() - h * inner.height();
            let base = if param.centred() {
                inner.center().y
            } else {
                inner.bottom()
            };
            let bar = Rect::from_x_y_ranges(x + 4.0..=x + STEP - 4.0, y.min(base)..=y.max(base));
            painter.rect_filled(bar.expand2(vec2(0.0, 0.5)), 1.0, col.gamma_multiply(0.8));
            painter.line_segment(
                [pos2(x + 2.0, y), pos2(x + STEP - 2.0, y)],
                Stroke::new(2.0, col),
            );
        }
        let at = resp.interact_pointer_pos().or(resp.hover_pos());
        let hit = at.and_then(|q| {
            let s = cell_under(q.x - area.left(), steps)?;
            Some((s, step_note(notes, s)?, q))
        });
        let reset = resp.clicked_by(egui::PointerButton::Secondary);
        if resp.drag_started() || resp.clicked() || reset {
            out.notes.push((i, Edit::Begin));
        }
        if let Some((_, n, q)) = hit {
            let h = f64::from((inner.bottom() - q.y) / inner.height());
            if reset {
                out.notes.push((i, param.edit(&n.id, None)));
            } else if resp.dragged_by(egui::PointerButton::Primary)
                || resp.clicked_by(egui::PointerButton::Primary)
            {
                out.notes.push((i, param.edit(&n.id, Some(h))));
            }
        }
        let tip = match hit {
            Some((s, n, _)) => format!(
                "Step {}: {} {}\nDrag to set, right click to reset",
                s + 1,
                param.label(),
                param.describe(n)
            ),
            None => format!(
                "{}: drag over the steps that are on to set it",
                param.label()
            ),
        };
        resp.on_hover_text(tip);
    });
}

/// The channel's right-click menu.
fn channel_menu(
    ui: &mut Ui,
    project: &Project,
    i: usize,
    notes: &[Note],
    steps: usize,
    state: &mut StepState,
    out: &mut Out,
) {
    let c = &project.channels[i];
    if ui.button("Piano roll").clicked() {
        out.open_roll = Some(i);
        ui.close();
    }
    if ui.button("Instrument").clicked() {
        out.open_synth = Some(i);
        ui.close();
    }
    if ui
        .button("Channel settings (arp, chords)...")
        .on_hover_text("The arpeggiator and chord presets of this channel; for a sampler also its sample and envelopes")
        .clicked()
    {
        out.open_settings = Some(i);
        ui.close();
    }
    if c.sample_id.is_some() {
        crate::sampleedit::menu_items(ui, &c.id);
    }
    ui.menu_button("Create automation clip", |ui| {
        let pitch = c.pitch;
        for (param, what, start) in [
            ("volume", "level", c.volume / 1.25),
            ("pan", "pan", (c.pan + 1.0) / 2.0),
            ("pitch", "pitch", ((pitch + 12.0) / 24.0).clamp(0.0, 1.0)),
        ] {
            if ui
                .button(format!("Channel {what}"))
                .on_hover_text(if param == "pitch" {
                    "Sets the pitch notes start at, up to an octave up or down"
                } else {
                    ""
                })
                .clicked()
            {
                out.chans.push(ChanEdit::Automate(crate::auto::AutoRequest {
                    target: crate::auto::channel_target(&c.id, param),
                    name: format!("{} {what}", c.name),
                    start,
                }));
                ui.close();
            }
        }
    });
    ui.separator();
    if ui.button("Rename...").clicked() {
        out.chans.push(ChanEdit::AskRename(i));
        ui.close();
    }
    if ui.button("Clone").clicked() {
        out.chans.push(ChanEdit::Begin);
        out.chans.push(ChanEdit::Clone(i));
        ui.close();
    }
    ui.menu_button("Colour", |ui| {
        for hex in las_project::create::PALETTE {
            let col = crate::rack::channel_colour_of(hex);
            let (r, sw) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
            ui.painter().rect_filled(r, 4.0, col);
            if sw.clicked() {
                out.chans.push(ChanEdit::Begin);
                out.chans.push(ChanEdit::Color {
                    channel: i,
                    color: hex.to_string(),
                });
                ui.close();
            }
        }
    });
    ui.menu_button("Group", |ui| {
        let now = c.group.clone().filter(|g| !g.is_empty());
        if ui
            .selectable_label(now.is_none(), "Unsorted (no group)")
            .clicked()
        {
            out.chans.push(ChanEdit::Begin);
            out.chans.push(ChanEdit::Group {
                channel: i,
                group: None,
            });
            ui.close();
        }
        for g in groups(project) {
            if ui
                .selectable_label(now.as_deref() == Some(g.as_str()), &g)
                .clicked()
            {
                out.chans.push(ChanEdit::Begin);
                out.chans.push(ChanEdit::Group {
                    channel: i,
                    group: Some(g),
                });
                ui.close();
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut state.new_group)
                    .hint_text("New group")
                    .desired_width(110.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let name = state.new_group.trim().to_string();
            if (ui.button("Add").clicked() || enter) && !name.is_empty() {
                out.chans.push(ChanEdit::Begin);
                out.chans.push(ChanEdit::Group {
                    channel: i,
                    group: Some(name),
                });
                state.new_group.clear();
                ui.close();
            }
        });
    })
    .response
    .on_hover_text("Put the channel in a group; the rack's Show menu shows one group at a time");
    ui.menu_button("Fill", |ui| {
        for every in [1usize, 2, 4, 8] {
            let label = if every == 1 {
                "Every step".to_string()
            } else {
                format!("Each {every} steps")
            };
            if ui.button(label).clicked() {
                out.notes.push((i, Edit::Begin));
                for e in fill_steps(notes, steps, every) {
                    out.notes.push((i, e));
                }
                ui.close();
            }
        }
        if ui
            .button("Randomize")
            .on_hover_text("About a third of the steps, picked at random")
            .clicked()
        {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64);
            out.notes.push((i, Edit::Begin));
            for e in random_steps(notes, steps, 0.35, seed) {
                out.notes.push((i, e));
            }
            ui.close();
        }
        ui.separator();
        for (label, by) in [("Shift left", -1i64), ("Shift right", 1)] {
            if ui.button(label).clicked() {
                out.notes.push((i, Edit::Begin));
                for e in shift_steps(notes, steps, by) {
                    out.notes.push((i, e));
                }
                ui.close();
            }
        }
        if ui.button("Clear steps").clicked() {
            out.notes.push((i, Edit::Begin));
            for e in fill_steps(notes, steps, 0) {
                out.notes.push((i, e));
            }
            ui.close();
        }
    });
    let mut smart = las_engine::render::smart_disable(&c.extra);
    if ui
        .checkbox(&mut smart, "Smart disable (save CPU when silent)")
        .on_hover_text("The channel stops working while it has no notes to play and is silent, and wakes at the next note")
        .changed()
    {
        out.chans.push(ChanEdit::Begin);
        out.chans.push(ChanEdit::SmartDisable { channel: i, on: smart });
    }
    ui.menu_button("Swing", |ui| {
        let now = c.swing;
        if ui.selectable_label(now.is_none(), "Song's swing").clicked() {
            out.chans.push(ChanEdit::Begin);
            out.chans.push(ChanEdit::Swing {
                channel: i,
                value: None,
            });
            ui.close();
        }
        for pct in [0u32, 15, 25, 33, 50, 66, 75] {
            let v = f64::from(pct) / 100.0;
            let on = now.is_some_and(|x| (x - v).abs() < 1e-6);
            if ui.selectable_label(on, format!("{pct}%")).clicked() {
                out.chans.push(ChanEdit::Begin);
                out.chans.push(ChanEdit::Swing {
                    channel: i,
                    value: Some(v),
                });
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Swing for this channel's steps only");
    ui.menu_button("Send to insert", |ui| insert_menu(ui, project, i, out));
    ui.separator();
    if ui.button("Delete").clicked() {
        out.chans.push(ChanEdit::Begin);
        out.chans.push(ChanEdit::Delete(i));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/mixer.las");
        las_project::decode(bytes).unwrap().project
    }

    #[test]
    fn fill_puts_a_step_on_every_nth_and_leaves_piano_roll_notes() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        let mut roll = new_note(64, 5.0, 10.0);
        roll.step = None;
        p.patterns[0].notes.insert(ch.clone(), vec![roll]);
        let run = |p: &mut Project, every| {
            let notes = p.patterns[0].notes.get(&ch).cloned().unwrap_or_default();
            for e in fill_steps(&notes, 16, every) {
                crate::roll::apply(p, 0, &ch, &e);
            }
        };
        run(&mut p, 4);
        let steps = |p: &Project| {
            let mut v: Vec<f64> = p.patterns[0].notes[&ch]
                .iter()
                .filter(|n| n.step == Some(true))
                .map(|n| n.pos / f64::from(TICKS_PER_STEP))
                .collect();
            v.sort_by(f64::total_cmp);
            v
        };
        assert_eq!(steps(&p), [0.0, 4.0, 8.0, 12.0]);
        run(&mut p, 2);
        assert_eq!(steps(&p).len(), 8);
        run(&mut p, 4);
        let notes = p.patterns[0].notes[&ch].clone();
        for e in shift_steps(&notes, 16, -1) {
            crate::roll::apply(&mut p, 0, &ch, &e);
        }
        assert_eq!(steps(&p), [3.0, 7.0, 11.0, 15.0]);
        let notes = p.patterns[0].notes[&ch].clone();
        for e in shift_steps(&notes, 16, 2) {
            crate::roll::apply(&mut p, 0, &ch, &e);
        }
        assert_eq!(steps(&p), [1.0, 5.0, 9.0, 13.0]);
        run(&mut p, 0);
        assert!(steps(&p).is_empty());
        // the note from the piano roll is still there
        assert_eq!(p.patterns[0].notes[&ch].len(), 1);
    }

    #[test]
    fn random_steps_repeat_for_a_seed_and_follow_the_density() {
        let count = |e: &[Edit]| e.iter().filter(|x| matches!(x, Edit::Add(_))).count();
        let a = random_steps(&[], 64, 0.35, 42);
        let b = random_steps(&[], 64, 0.35, 42);
        let pos = |e: &[Edit]| {
            e.iter()
                .filter_map(|x| match x {
                    Edit::Add(n) => Some(n.pos),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(pos(&a), pos(&b));
        assert!((10..=36).contains(&count(&a)), "{}", count(&a));
        assert_eq!(count(&random_steps(&[], 64, 0.0, 1)), 0);
        assert_eq!(count(&random_steps(&[], 64, 1.0, 1)), 64);
    }

    #[test]
    fn toggling_a_step_adds_then_removes_a_step_note() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        let e = toggle(&[], 5);
        let Edit::Add(n) = &e else { panic!("{e:?}") };
        assert_eq!(n.pos, 5.0 * f64::from(TICKS_PER_STEP));
        assert_eq!(n.step, Some(true));
        crate::roll::apply(&mut p, 0, &ch, &e);
        let notes = &p.patterns[0].notes[&ch];
        assert!(step_note(notes, 5).is_some() && step_note(notes, 4).is_none());
        let off = toggle(notes, 5);
        assert!(matches!(off, Edit::Delete { .. }));
        crate::roll::apply(&mut p, 0, &ch, &off);
        assert!(!p.patterns[0].notes.contains_key(&ch));
    }

    #[test]
    fn channels_are_cloned_routed_coloured_and_deleted() {
        let mut p = project();
        let n = p.channels.len();
        let first = p.channels[0].id.clone();
        let notes = p.patterns[0].notes.get(&first).map_or(0, Vec::len);
        apply(&mut p, &ChanEdit::Clone(0));
        assert_eq!(p.channels.len(), n + 1);
        let copy = p.channels[1].clone();
        assert_ne!(copy.id, first);
        assert!(copy.name.ends_with("copy"));
        let copied = p.patterns[0].notes.get(&copy.id).map_or(0, Vec::len);
        assert_eq!(copied, notes);
        if notes > 0 {
            assert_ne!(
                p.patterns[0].notes[&copy.id][0].id,
                p.patterns[0].notes[&first][0].id
            );
        }
        apply(
            &mut p,
            &ChanEdit::Route {
                channel: 1,
                insert: 3,
            },
        );
        assert_eq!(p.channels[1].mixer_track, 3);
        apply(
            &mut p,
            &ChanEdit::Route {
                channel: 1,
                insert: 999,
            },
        );
        assert_eq!(p.channels[1].mixer_track, 3);
        apply(
            &mut p,
            &ChanEdit::Color {
                channel: 1,
                color: "#112233".into(),
            },
        );
        assert_eq!(p.channels[1].color, "#112233");
        apply(&mut p, &ChanEdit::Delete(1));
        assert_eq!(p.channels.len(), n);
        assert!(p.patterns.iter().all(|x| !x.notes.contains_key(&copy.id)));
        apply(&mut p, &ChanEdit::Delete(99));
        assert_eq!(p.channels.len(), n);
    }

    #[test]
    fn channel_edits_are_held_to_their_ranges() {
        let mut p = project();
        apply(
            &mut p,
            &ChanEdit::Volume {
                channel: 0,
                value: 5.0,
            },
        );
        apply(
            &mut p,
            &ChanEdit::Pan {
                channel: 0,
                value: -5.0,
            },
        );
        apply(
            &mut p,
            &ChanEdit::Mute {
                channel: 0,
                on: true,
            },
        );
        assert_eq!((p.channels[0].volume, p.channels[0].pan), (1.25, -1.0));
        assert!(p.channels[0].mute);
        apply(
            &mut p,
            &ChanEdit::Volume {
                channel: 99,
                value: 0.5,
            },
        );
    }

    #[test]
    fn the_step_sequencer_draws_and_a_click_toggles_a_step() {
        let p = project();
        let ctx = egui::Context::default();
        let mut state = StepState::default();
        let mut sel = 0usize;
        let mut all = Vec::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 400.0));
        // find an empty step in the first row by clicking a late cell
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        crate::widgets::run_frame(&ctx, input, |ui| {
            draw(ui, &p, 0, &mut sel, &mut state, None);
        });
        let y = state.rows[0].1.center().y;
        let at = pos2(18.0 + 22.0 * 2.0 + 20.0 + NAME + 60.0 + 14.0 * 27.0, y);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![press(true)],
            vec![press(false)],
            vec![],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                let o = draw(ui, &p, 0, &mut sel, &mut state, Some(48.0));
                all.extend(o.notes);
            });
        }
        assert!(
            all.iter()
                .any(|(_, e)| matches!(e, Edit::Add(_) | Edit::Delete { .. })),
            "{all:?}"
        );
    }

    #[test]
    fn alt_click_solos_a_channel_and_again_brings_every_channel_back() {
        let mut p = project();
        assert!(p.channels.len() >= 2);
        for e in solo(&p, 1) {
            apply(&mut p, &e);
        }
        assert!(p
            .channels
            .iter()
            .enumerate()
            .all(|(i, c)| c.mute == (i != 1)));
        for e in solo(&p, 1) {
            apply(&mut p, &e);
        }
        assert!(p.channels.iter().all(|c| !c.mute));
        assert!(solo(&p, 999).is_empty());
    }

    #[test]
    fn channels_are_grouped_and_the_filter_shows_one_group() {
        let mut p = project();
        apply(
            &mut p,
            &ChanEdit::Group {
                channel: 0,
                group: Some(" Drums ".into()),
            },
        );
        apply(
            &mut p,
            &ChanEdit::Group {
                channel: 1,
                group: Some("Drums".into()),
            },
        );
        assert_eq!(groups(&p), ["Drums"]);
        let drums = GroupFilter::Named("Drums".into());
        let shown = |f: &GroupFilter, p: &Project| p.channels.iter().filter(|c| f.shows(c)).count();
        assert_eq!(shown(&drums, &p), 2);
        assert_eq!(shown(&GroupFilter::Unsorted, &p), p.channels.len() - 2);
        assert_eq!(shown(&GroupFilter::All, &p), p.channels.len());
        // an empty name is no group
        apply(
            &mut p,
            &ChanEdit::Group {
                channel: 1,
                group: Some("  ".into()),
            },
        );
        assert_eq!(p.channels[1].group, None);
        assert_eq!(shown(&drums, &p), 1);
    }

    #[test]
    fn the_graph_editor_sets_velocity_pan_and_fine_pitch_of_a_step() {
        let mut p = project();
        let ch = p.channels[0].id.clone();
        p.patterns[0].notes.remove(&ch);
        crate::roll::apply(&mut p, 0, &ch, &toggle(&[], 3));
        let id = p.patterns[0].notes[&ch][0].id.clone();
        let note = |p: &Project| p.patterns[0].notes[&ch][0].clone();
        for (gp, h) in [
            (GraphParam::Velocity, 0.25),
            (GraphParam::Pan, 0.0),
            (GraphParam::Fine, 0.75),
        ] {
            crate::roll::apply(&mut p, 0, &ch, &gp.edit(&id, Some(h)));
        }
        let n = note(&p);
        assert_eq!((n.vel, n.pan, n.fine), (0.25, -1.0, Some(50.0)));
        assert_eq!(GraphParam::Pan.describe(&n), "100% left");
        assert!((GraphParam::Fine.height(&n) - 0.75).abs() < 1e-9);
        // right click puts each back
        for gp in GraphParam::ALL {
            crate::roll::apply(&mut p, 0, &ch, &gp.edit(&id, None));
        }
        let n = note(&p);
        assert_eq!((n.vel, n.pan, n.fine), (0.78, 0.0, None));
        // held to range
        crate::roll::apply(&mut p, 0, &ch, &GraphParam::Velocity.edit(&id, Some(4.0)));
        assert_eq!(note(&p).vel, 1.0);
    }

    #[test]
    fn steps_map_to_cells_and_back() {
        for s in [0usize, 3, 4, 15, 31] {
            let x = cell_offset(s) + 1.0;
            assert_eq!(cell_under(x, 32), Some(s));
        }
        assert_eq!(cell_under(-1.0, 16), None);
        assert_eq!(cell_under(cell_offset(16) + 1.0, 16), None);
    }

    #[test]
    fn the_rack_shows_as_many_steps_as_the_pattern_has() {
        let mut p = project();
        p.patterns[0].length = 32.0 * f64::from(TICKS_PER_STEP);
        let ctx = egui::Context::default();
        let mut state = StepState {
            graph: true,
            ..Default::default()
        };
        let mut sel = 0usize;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(2400.0, 600.0));
        let mut widest = 0.0f32;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            crate::widgets::run_frame(&ctx, input, |ui| {
                draw(ui, &p, 0, &mut sel, &mut state, None);
            });
            widest = state
                .rows
                .iter()
                .map(|(_, r)| r.width())
                .fold(0.0, f32::max);
        }
        assert!(widest > cell_offset(32), "{widest}");
        assert!(!state.rows.is_empty());
        let (i, r) = state.rows[0];
        assert_eq!(state.row_at(r.center()), Some(i));
    }
}
