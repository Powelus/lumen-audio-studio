// SPDX-License-Identifier: GPL-3.0-or-later
//! The mixer: one strip for every insert with its fader, pan, mute and solo and the
//! effects in its slots. Like the other editors, drawing reports [`MixEdit`]s and
//! [`apply`] changes the project.

use crate::rack::channel_colour_of;
use eframe::egui::{self, RichText, Ui};
use las_project::Project;

/// A change the user asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum MixEdit {
    /// A gesture starts: remember the project so that it can be undone.
    Begin,
    /// Put an effect in the first empty slot.
    AddEffect {
        insert: usize,
        slot: usize,
        plugin: String,
    },
    /// Put a hosted plugin (by its id) in a slot.
    AddHosted {
        insert: usize,
        slot: usize,
        plugin: String,
    },
    Rename {
        insert: usize,
        name: String,
    },
    /// Where the insert plays to (the master, or another insert).
    Output {
        insert: usize,
        target: usize,
    },
    AddSend {
        insert: usize,
        target: usize,
    },
    SetSend {
        insert: usize,
        index: usize,
        amount: f64,
    },
    SendSidechain {
        insert: usize,
        index: usize,
        on: bool,
    },
    RemoveSend {
        insert: usize,
        index: usize,
    },
    Width {
        insert: usize,
        value: f64,
    },
    /// Empty a slot.
    RemoveEffect {
        insert: usize,
        slot: usize,
    },
    /// Make an automation clip for a parameter of the insert.
    Automate(crate::auto::AutoRequest),
    /// Open the window of an effect (not a change to the project).
    OpenEffect {
        insert: usize,
        slot: usize,
    },
    Volume {
        insert: usize,
        value: f64,
    },
    Pan {
        insert: usize,
        value: f64,
    },
    Mute {
        insert: usize,
        on: bool,
    },
    Solo {
        insert: usize,
        on: bool,
    },
    SlotOn {
        insert: usize,
        slot: usize,
        on: bool,
    },
    /// Smart disable of an effect slot (on by default).
    SlotSmart {
        insert: usize,
        slot: usize,
        on: bool,
    },
    /// Set a band (0 low, 1 mid, 2 high) of the insert's built-in EQ, dB.
    Eq {
        insert: usize,
        band: usize,
        gain: f64,
    },
    /// Invert the insert's polarity.
    Invert {
        insert: usize,
        on: bool,
    },
    /// Swap the insert's left and right sides.
    Swap {
        insert: usize,
        on: bool,
    },
    /// Move an effect to another slot of the same insert (trading places with what is
    /// there).
    MoveEffect {
        insert: usize,
        from: usize,
        to: usize,
    },
    /// Put another effect (`hosted`: a plugin id) in a slot in place of its own.
    ReplaceEffect {
        insert: usize,
        slot: usize,
        plugin: String,
        hosted: bool,
    },
    /// Replace the insert's whole effects chain.
    SetChain {
        insert: usize,
        slots: Vec<Option<las_project::EffectSlot>>,
    },
    /// Ask where to save the insert's effects chain (not a change to the project).
    SaveChain(usize),
    /// Ask for an effects chain file to load into the insert.
    LoadChain(usize),
    /// Play a channel through this insert (FL's "Route to this track only", Ctrl+L).
    RouteChannel {
        channel: usize,
        insert: usize,
    },
}

/// The file an insert's effects chain is saved as.
pub fn chain_json(t: &las_project::MixerTrack) -> String {
    let v = serde_json::json!({ "lumenMixerChain": 1, "slots": t.slots });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// The slots of a chain file, with new ids, as many as an insert has.
pub fn parse_chain(text: &str) -> Result<Vec<Option<las_project::EffectSlot>>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let slots = v
        .get("slots")
        .ok_or("This is not an effects chain file (it has no slots).")?;
    let mut slots: Vec<Option<las_project::EffectSlot>> =
        serde_json::from_value(slots.clone()).map_err(|e| e.to_string())?;
    slots.truncate(las_project::MIXER_SLOTS);
    slots.resize(las_project::MIXER_SLOTS, None);
    for s in slots.iter_mut().flatten() {
        s.id = las_project::create::uid("fx");
    }
    Ok(slots)
}

/// A new effect for a slot: a built-in one by its id, or a hosted plugin by its id.
fn new_effect(plugin: &str, hosted: bool) -> Option<las_project::EffectSlot> {
    let state = if hosted {
        las_host::plugin_state(plugin, &[])
    } else {
        las_dsp::fx::registry::descriptor(plugin)?
            .default_state
            .clone()
    };
    Some(las_project::EffectSlot {
        id: las_project::create::uid("fx"),
        plugin_id: if hosted {
            las_host::HOSTED.into()
        } else {
            plugin.to_string()
        },
        enabled: true,
        mix: 1.0,
        state,
        ..Default::default()
    })
}

/// What a level meter remembers: its peak hold line and whether it clipped.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterHold {
    /// The highest recent peak (linear).
    pub hold: f32,
    /// When it was reached, seconds.
    pub at: f64,
    /// It went over 0 dB since the last reset.
    pub clip: bool,
}

impl MeterHold {
    /// Seconds the hold line stays before it falls.
    pub const HOLD: f64 = 1.5;

    /// Take the peak of a frame at time `now`.
    pub fn update(&mut self, peak: f32, now: f64) {
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        if peak >= self.hold {
            self.hold = peak;
            self.at = now;
        } else if now - self.at > Self::HOLD {
            // falls about 20 dB a second
            let dt = (now - self.at - Self::HOLD).min(0.1) as f32;
            self.hold = (self.hold * 0.1f32.powf(dt)).max(peak);
        }
        if peak > 1.0 {
            self.clip = true;
        }
    }

    pub fn reset(&mut self) {
        *self = MeterHold::default();
    }
}

/// The response of the insert's three-band EQ at `freq`, dB.
pub fn eq_db(eq: &las_project::InsertEq, freq: f64) -> f64 {
    use las_dsp::biquad::{biquad_db, Kind};
    let sr = 48_000.0;
    let kinds = [Kind::LowShelf, Kind::Peaking, Kind::HighShelf];
    let qs = [0.707, 0.9, 0.707];
    eq.gains()
        .iter()
        .enumerate()
        .filter(|(_, g)| g.abs() > 1e-6)
        .map(|(b, g)| {
            biquad_db(
                kinds[b],
                las_project::InsertEq::FREQS[b],
                *g,
                qs[b],
                freq,
                sr,
            )
        })
        .sum()
}

/// Whether feeding `to` from `from` would make a loop: `to` is `from`, or `from` is
/// reached by following where `to` plays to and sends to.
pub fn would_cycle(project: &Project, from: usize, to: usize) -> bool {
    let mut seen = vec![false; project.mixer.len()];
    let mut stack = vec![to];
    while let Some(i) = stack.pop() {
        if i == from {
            return true;
        }
        if i >= project.mixer.len() || std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let t = &project.mixer[i];
        stack.extend(t.output);
        stack.extend(t.sends.iter().map(|s| s.target));
    }
    false
}

/// Carry out one edit on a project.
pub fn apply(project: &mut Project, edit: &MixEdit) {
    let strip = |p: &mut Project, i: usize| -> Option<usize> { (i < p.mixer.len()).then_some(i) };
    match edit {
        MixEdit::Begin => {}
        MixEdit::Volume { insert, value } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].volume = value.clamp(0.0, 1.25);
            }
        }
        MixEdit::Pan { insert, value } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].pan = value.clamp(-1.0, 1.0);
            }
        }
        MixEdit::Mute { insert, on } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].mute = *on;
            }
        }
        MixEdit::Solo { insert, on } => {
            if let Some(i) = strip(project, *insert) {
                project.mixer[i].solo = *on;
            }
        }
        MixEdit::OpenEffect { .. }
        | MixEdit::Automate(_)
        | MixEdit::SaveChain(_)
        | MixEdit::LoadChain(_) => {}
        MixEdit::Eq { insert, band, gain } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                let g = if gain.is_finite() {
                    gain.clamp(-las_project::InsertEq::RANGE, las_project::InsertEq::RANGE)
                } else {
                    0.0
                };
                match band {
                    0 => t.eq.low = g,
                    1 => t.eq.mid = g,
                    2 => t.eq.high = g,
                    _ => {}
                }
            }
        }
        MixEdit::Invert { insert, on } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.invert = *on;
            }
        }
        MixEdit::Swap { insert, on } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.swap = *on;
            }
        }
        MixEdit::MoveEffect { insert, from, to } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                if t.slots.len() < las_project::MIXER_SLOTS {
                    t.slots.resize(las_project::MIXER_SLOTS, None);
                }
                if *from < t.slots.len() && *to < t.slots.len() {
                    t.slots.swap(*from, *to);
                }
            }
        }
        MixEdit::ReplaceEffect {
            insert,
            slot,
            plugin,
            hosted,
        } => {
            if let (Some(t), Some(fx)) =
                (project.mixer.get_mut(*insert), new_effect(plugin, *hosted))
            {
                if t.slots.len() < las_project::MIXER_SLOTS {
                    t.slots.resize(las_project::MIXER_SLOTS, None);
                }
                if let Some(s) = t.slots.get_mut(*slot) {
                    *s = Some(fx);
                }
            }
        }
        MixEdit::SetChain { insert, slots } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.slots = slots.clone();
                t.slots.resize(las_project::MIXER_SLOTS, None);
            }
        }
        MixEdit::RouteChannel { channel, insert } => {
            if *insert < project.mixer.len() {
                if let Some(c) = project.channels.get_mut(*channel) {
                    c.mixer_track = *insert;
                }
            }
        }
        MixEdit::Output { insert, target } => {
            if *insert > 0
                && *insert < project.mixer.len()
                && *target < project.mixer.len()
                && !would_cycle(project, *insert, *target)
            {
                project.mixer[*insert].output = Some(*target);
            }
        }
        MixEdit::AddSend { insert, target } => {
            if *insert < project.mixer.len()
                && *target < project.mixer.len()
                && *target != 0
                && !would_cycle(project, *insert, *target)
                && !project.mixer[*insert]
                    .sends
                    .iter()
                    .any(|s| s.target == *target)
            {
                project.mixer[*insert].sends.push(las_project::MixSend {
                    target: *target,
                    amount: 0.5,
                    ..Default::default()
                });
            }
        }
        MixEdit::SetSend {
            insert,
            index,
            amount,
        } => {
            if let Some(s) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.sends.get_mut(*index))
            {
                s.amount = amount.clamp(0.0, 1.25);
            }
        }
        MixEdit::SendSidechain { insert, index, on } => {
            if let Some(s) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.sends.get_mut(*index))
            {
                s.sidechain_only = *on;
            }
        }
        MixEdit::RemoveSend { insert, index } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                if *index < t.sends.len() {
                    t.sends.remove(*index);
                }
            }
        }
        MixEdit::Rename { insert, name } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.name = name.clone();
            }
        }
        MixEdit::Width { insert, value } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                t.width = value.clamp(0.0, 2.0);
            }
        }
        MixEdit::AddEffect {
            insert,
            slot,
            plugin,
        } => {
            let Some(d) = las_dsp::fx::registry::descriptor(plugin) else {
                return;
            };
            if let Some(t) = project.mixer.get_mut(*insert) {
                if t.slots.len() < las_project::MIXER_SLOTS {
                    t.slots.resize(las_project::MIXER_SLOTS, None);
                }
                if let Some(free) = t.slots.get_mut(*slot).filter(|s| s.is_none()) {
                    *free = Some(las_project::EffectSlot {
                        id: las_project::create::uid("fx"),
                        plugin_id: plugin.clone(),
                        enabled: true,
                        mix: 1.0,
                        state: d.default_state.clone(),
                        ..Default::default()
                    });
                }
            }
        }
        MixEdit::AddHosted {
            insert,
            slot,
            plugin,
        } => {
            if let Some(t) = project.mixer.get_mut(*insert) {
                if t.slots.len() < las_project::MIXER_SLOTS {
                    t.slots.resize(las_project::MIXER_SLOTS, None);
                }
                if let Some(free) = t.slots.get_mut(*slot).filter(|s| s.is_none()) {
                    *free = Some(las_project::EffectSlot {
                        id: las_project::create::uid("fx"),
                        plugin_id: las_host::HOSTED.into(),
                        enabled: true,
                        mix: 1.0,
                        state: las_host::plugin_state(plugin, &[]),
                        ..Default::default()
                    });
                }
            }
        }
        MixEdit::RemoveEffect { insert, slot } => {
            if let Some(s) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.slots.get_mut(*slot))
            {
                *s = None;
            }
        }
        MixEdit::SlotOn { insert, slot, on } => {
            if let Some(Some(s)) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.slots.get_mut(*slot))
            {
                s.enabled = *on;
            }
        }
        MixEdit::SlotSmart { insert, slot, on } => {
            if let Some(Some(s)) = project
                .mixer
                .get_mut(*insert)
                .and_then(|t| t.slots.get_mut(*slot))
            {
                crate::steps::set_smart(&mut s.extra, *on);
            }
        }
    }
}

/// What the mixer remembers between frames.
#[derive(Default)]
pub struct MixerState {
    /// The insert whose effects and settings the panel on the right shows.
    pub selected: usize,
    /// Peak hold and clip of each insert's meter.
    pub meters: Vec<MeterHold>,
}

/// An effect slot being dragged to another slot.
#[derive(Clone, Copy, Debug)]
struct SlotDrag {
    insert: usize,
    slot: usize,
}

/// The channel selected in the rack: its index and name.
pub type Selected<'a> = Option<(usize, &'a str)>;

pub fn draw(
    ui: &mut Ui,
    project: &Project,
    state: &mut MixerState,
    peaks: &[f32],
    channel: Selected,
) -> Vec<MixEdit> {
    let mut edits = Vec::new();
    state.selected = state.selected.min(project.mixer.len().saturating_sub(1));
    let now = ui.input(|i| i.time);
    state
        .meters
        .resize(project.mixer.len(), MeterHold::default());
    for (i, m) in state.meters.iter_mut().enumerate() {
        m.update(peaks.get(i).copied().unwrap_or(0.0), now);
    }
    if !peaks.is_empty() || state.meters.iter().any(|m| m.hold > 0.0) {
        ui.ctx().request_repaint();
    }
    // Ctrl+L, as in FL: the selected channel plays through the selected insert
    if let Some((c, _)) = channel {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::L)) {
            edits.push(MixEdit::Begin);
            edits.push(MixEdit::RouteChannel {
                channel: c,
                insert: state.selected,
            });
        }
    }
    let panel_w = 190.0;
    let avail = ui.available_width();
    ui.horizontal_top(|ui| {
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .id_salt("mixer")
            .max_width((avail - panel_w - 28.0).max(200.0))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for i in 0..project.mixer.len() {
                        let peak = peaks.get(i).copied().unwrap_or(0.0);
                        strip(ui, project, i, state, peak, channel, &mut edits);
                    }
                });
            });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(panel_w);
            detail(ui, project, state.selected, &mut edits);
            routing(ui, project, state.selected, &mut edits);
        });
    });
    edits
}

/// The panel of the selected insert: its name, its effect slots, its stereo width.
fn detail(ui: &mut Ui, project: &Project, i: usize, edits: &mut Vec<MixEdit>) {
    let p = crate::theme::pal();
    let t = &project.mixer[i];
    let mut name = t.name.clone();
    if ui
        .add(egui::TextEdit::singleline(&mut name).desired_width(170.0))
        .changed()
    {
        edits.push(MixEdit::Begin);
        edits.push(MixEdit::Rename { insert: i, name });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("EFFECTS").size(11.0).strong().color(p.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Load")
                .on_hover_text("Load chain...: replace this insert's effects with a saved chain")
                .clicked()
            {
                edits.push(MixEdit::LoadChain(i));
            }
            if ui
                .small_button("Save")
                .on_hover_text("Save chain...: save this insert's effects to a file")
                .clicked()
            {
                edits.push(MixEdit::SaveChain(i));
            }
        });
    });
    let carrying = egui::DragAndDrop::payload::<SlotDrag>(ui.ctx())
        .map(|d| *d)
        .filter(|d| d.insert == i);
    for n in 0..las_project::MIXER_SLOTS {
        let row = ui.horizontal(|ui| match t.slots.get(n).and_then(Option::as_ref) {
            Some(s) => {
                let led = ui
                    .add(
                        egui::Button::new(if s.enabled { "\u{25cf}" } else { "\u{25cb}" })
                            .small()
                            .frame(false),
                    )
                    .on_hover_text("Switch on or off");
                if led.clicked() {
                    edits.push(MixEdit::Begin);
                    edits.push(MixEdit::SlotOn {
                        insert: i,
                        slot: n,
                        on: !s.enabled,
                    });
                }
                let b = ui
                    .add(egui::Button::new(slot_label(s)).min_size(egui::vec2(100.0, 0.0)))
                    .interact(egui::Sense::click_and_drag())
                    .on_hover_text(format!(
                        "{}\nClick: open. Drag onto another slot to move it. Right click: more.",
                        s.plugin_id
                    ));
                b.dnd_set_drag_payload(SlotDrag { insert: i, slot: n });
                if b.clicked() {
                    edits.push(MixEdit::OpenEffect { insert: i, slot: n });
                }
                b.context_menu(|ui| {
                    if ui
                        .add_enabled(n > 0, egui::Button::new("Move up"))
                        .clicked()
                    {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::MoveEffect {
                            insert: i,
                            from: n,
                            to: n - 1,
                        });
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            n + 1 < las_project::MIXER_SLOTS,
                            egui::Button::new("Move down"),
                        )
                        .clicked()
                    {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::MoveEffect {
                            insert: i,
                            from: n,
                            to: n + 1,
                        });
                        ui.close();
                    }
                    let mut smart = las_engine::render::smart_disable(&s.extra);
                    if ui
                        .checkbox(&mut smart, "Smart disable (save CPU when silent)")
                        .on_hover_text("The insert sleeps once nothing comes in and its tail has died away. Turn off for effects that make sound on their own.")
                        .changed()
                    {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::SlotSmart { insert: i, slot: n, on: smart });
                    }
                    ui.menu_button("Replace", |ui| {
                        if let Some((plugin, hosted)) = effect_menu(ui) {
                            edits.push(MixEdit::Begin);
                            edits.push(MixEdit::ReplaceEffect {
                                insert: i,
                                slot: n,
                                plugin,
                                hosted,
                            });
                            ui.close();
                        }
                    });
                    if ui.button("Remove").clicked() {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::RemoveEffect { insert: i, slot: n });
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Save chain...").clicked() {
                        edits.push(MixEdit::SaveChain(i));
                        ui.close();
                    }
                    if ui.button("Load chain...").clicked() {
                        edits.push(MixEdit::LoadChain(i));
                        ui.close();
                    }
                });
            }
            None => {
                let r = ui.menu_button(
                    RichText::new(format!("+  Slot {}", n + 1)).color(p.muted),
                    |ui| {
                        if let Some((plugin, hosted)) = effect_menu(ui) {
                            edits.push(MixEdit::Begin);
                            edits.push(if hosted {
                                MixEdit::AddHosted {
                                    insert: i,
                                    slot: n,
                                    plugin,
                                }
                            } else {
                                MixEdit::AddEffect {
                                    insert: i,
                                    slot: n,
                                    plugin,
                                }
                            });
                            ui.close();
                        }
                    },
                );
                r.response.on_hover_text("Add an effect");
            }
        });
        // a slot dragged here moves (trading places with what is here)
        let rr = row.response;
        if let Some(d) = carrying {
            if d.slot != n && rr.contains_pointer() {
                ui.painter().rect_stroke(
                    rr.rect.expand(1.0),
                    3.0,
                    egui::Stroke::new(1.5, p.accent),
                    egui::StrokeKind::Outside,
                );
                if ui.input(|x| x.pointer.any_released()) {
                    egui::DragAndDrop::clear_payload(ui.ctx());
                    edits.push(MixEdit::Begin);
                    edits.push(MixEdit::MoveEffect {
                        insert: i,
                        from: d.slot,
                        to: n,
                    });
                }
            }
        }
    }
    ui.add_space(8.0);
    ui.label(RichText::new("STEREO").size(11.0).strong().color(p.muted));
    ui.horizontal(|ui| {
        let mut w = t.width;
        let (ch, start) = crate::steps::mini_knob(ui, &mut w, (0.0, 2.0), 1.0, "Width", p.accent);
        if start {
            edits.push(MixEdit::Begin);
        }
        if ch {
            edits.push(MixEdit::Width {
                insert: i,
                value: w,
            });
        }
        ui.label(
            RichText::new(format!("Separation {:.0}%", t.width * 100.0))
                .size(11.0)
                .color(p.muted),
        )
        .on_hover_text("Stereo separation: 0% is mono, 100% as it is, 200% extra wide");
    });
    ui.horizontal(|ui| {
        if ui
            .selectable_label(t.invert, "\u{00d8} Invert")
            .on_hover_text("Invert the polarity (phase) of this insert")
            .clicked()
        {
            edits.push(MixEdit::Begin);
            edits.push(MixEdit::Invert {
                insert: i,
                on: !t.invert,
            });
        }
        if ui
            .selectable_label(t.swap, "Swap L/R")
            .on_hover_text("Swap the left and right sides of this insert")
            .clicked()
        {
            edits.push(MixEdit::Begin);
            edits.push(MixEdit::Swap {
                insert: i,
                on: !t.swap,
            });
        }
    });
}

/// The menu of effects to put in a slot: the built-in ones by category, then the
/// plugins found on the computer. Returns the pick (and whether it is a plugin).
fn effect_menu(ui: &mut Ui) -> Option<(String, bool)> {
    let p = crate::theme::pal();
    let mut pick = None;
    let mut shown = "";
    let mut list: Vec<_> = las_dsp::fx::registry::descriptors().iter().collect();
    list.sort_by(|a, b| a.category.cmp(&b.category));
    for d in list {
        if d.category != shown {
            shown = &d.category;
            ui.label(RichText::new(shown).strong().color(p.accent_text));
        }
        if ui.button(&d.name).on_hover_text(&d.description).clicked() {
            pick = Some((d.id.clone(), false));
        }
    }
    let external: Vec<_> = las_host::known()
        .into_iter()
        .filter(las_host::PluginInfo::is_effect)
        .collect();
    if !external.is_empty() {
        ui.label(RichText::new("External").strong().color(p.accent_text));
    }
    for info in external {
        if ui.button(&info.name).on_hover_text(&info.vendor).clicked() {
            pick = Some((info.id, true));
        }
    }
    pick
}

/// The name an insert goes by in the routing menus.
fn insert_name(project: &Project, i: usize) -> String {
    let t = &project.mixer[i];
    if i == 0 {
        "Master".to_string()
    } else if t.name.is_empty() {
        format!("Insert {i}")
    } else {
        t.name.clone()
    }
}

/// Output and sends of the selected insert.
fn routing(ui: &mut Ui, project: &Project, i: usize, edits: &mut Vec<MixEdit>) {
    let p = crate::theme::pal();
    let t = &project.mixer[i];
    if i > 0 {
        ui.add_space(8.0);
        ui.label(RichText::new("OUTPUT").size(11.0).strong().color(p.muted));
        let current = t.output.unwrap_or(0);
        egui::ComboBox::from_id_salt("mixer-output")
            .selected_text(insert_name(project, current))
            .width(150.0)
            .show_ui(ui, |ui| {
                for to in 0..project.mixer.len() {
                    if to == i || would_cycle(project, i, to) {
                        continue;
                    }
                    if ui
                        .selectable_label(current == to, insert_name(project, to))
                        .clicked()
                    {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::Output {
                            insert: i,
                            target: to,
                        });
                    }
                }
            });
    }
    ui.add_space(8.0);
    ui.label(RichText::new("SENDS").size(11.0).strong().color(p.muted));
    for (n, s) in t.sends.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(insert_name(project, s.target)).size(11.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("x")
                    .on_hover_text("Remove the send")
                    .clicked()
                {
                    edits.push(MixEdit::Begin);
                    edits.push(MixEdit::RemoveSend {
                        insert: i,
                        index: n,
                    });
                }
                let mut sc = s.sidechain_only;
                if ui
                    .checkbox(&mut sc, "SC")
                    .on_hover_text("Only feeds the sidechain")
                    .changed()
                {
                    edits.push(MixEdit::Begin);
                    edits.push(MixEdit::SendSidechain {
                        insert: i,
                        index: n,
                        on: sc,
                    });
                }
            });
        });
        let mut amount = s.amount;
        let r = ui.add(egui::Slider::new(&mut amount, 0.0..=1.25).show_value(false));
        if r.changed() {
            if r.drag_started() || !r.dragged() {
                edits.push(MixEdit::Begin);
            }
            edits.push(MixEdit::SetSend {
                insert: i,
                index: n,
                amount,
            });
        }
    }
    ui.menu_button(RichText::new("+  Send to...").color(p.muted), |ui| {
        let mut any = false;
        for to in 1..project.mixer.len() {
            if to == i || t.sends.iter().any(|s| s.target == to) || would_cycle(project, i, to) {
                continue;
            }
            any = true;
            if ui.button(insert_name(project, to)).clicked() {
                edits.push(MixEdit::Begin);
                edits.push(MixEdit::AddSend {
                    insert: i,
                    target: to,
                });
                ui.close();
            }
        }
        if !any {
            ui.label("No insert to send to.");
        }
    });
}

fn strip(
    ui: &mut Ui,
    project: &Project,
    i: usize,
    state: &mut MixerState,
    peak: f32,
    channel: Selected,
    edits: &mut Vec<MixEdit>,
) {
    let t = &project.mixer[i];
    let p = crate::theme::pal();
    let col = channel_colour_of(&t.color);
    let selected = state.selected == i;
    let frame = egui::Frame::new()
        .fill(p.panel)
        .stroke(egui::Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected { p.accent } else { p.border },
        ))
        .corner_radius(6.0)
        .inner_margin(4.0)
        .show(ui, |ui| {
            ui.set_width(58.0);
            ui.set_min_height(420.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if i == 0 {
                        "M".to_string()
                    } else {
                        i.to_string()
                    })
                    .size(10.0)
                    .color(p.muted),
                );
                // the name on a pill in the insert's colour
                let name = if t.name.is_empty() {
                    format!("Insert {i}")
                } else {
                    t.name.clone()
                };
                let (r, resp) =
                    ui.allocate_exact_size(egui::vec2(54.0, 17.0), egui::Sense::click());
                ui.painter().rect_filled(r, 4.0, col.gamma_multiply(0.55));
                ui.painter().with_clip_rect(r).text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::proportional(10.5),
                    p.text,
                );
                if resp.clicked() {
                    state.selected = i;
                }
                let mut pan = t.pan;
                let (ch, start, pr) =
                    crate::steps::mini_knob_resp(ui, &mut pan, (-1.0, 1.0), 0.0, "Pan", p.text2);
                pr.context_menu(|ui| {
                    if ui.button("Create automation clip").clicked() {
                        edits.push(MixEdit::Automate(crate::auto::AutoRequest {
                            target: crate::auto::mixer_target(i, "pan"),
                            name: format!("{} pan", insert_name(project, i)),
                            start: (t.pan + 1.0) / 2.0,
                        }));
                        ui.close();
                    }
                });
                if start {
                    edits.push(MixEdit::Begin);
                }
                if ch {
                    edits.push(MixEdit::Pan {
                        insert: i,
                        value: pan,
                    });
                }
                eq_strip(ui, t, i, edits);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let m = ui.selectable_label(t.mute, "M");
                    if m.clicked() {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::Mute {
                            insert: i,
                            on: !t.mute,
                        });
                    }
                    let s = ui.selectable_label(t.solo, "S");
                    if s.clicked() {
                        edits.push(MixEdit::Begin);
                        edits.push(MixEdit::Solo {
                            insert: i,
                            on: !t.solo,
                        });
                    }
                });
                // the clip light: lit once the strip went over 0 dB; click to reset
                let meter = state.meters.get(i).copied().unwrap_or_default();
                let (cr, cresp) =
                    ui.allocate_exact_size(egui::vec2(44.0, 9.0), egui::Sense::click());
                ui.painter().rect_filled(
                    cr,
                    2.0,
                    if meter.clip {
                        egui::Color32::from_rgb(240, 60, 50)
                    } else {
                        p.well_deep
                    },
                );
                ui.painter().text(
                    cr.center(),
                    egui::Align2::CENTER_CENTER,
                    "CLIP",
                    egui::FontId::proportional(7.5),
                    if meter.clip {
                        egui::Color32::WHITE
                    } else {
                        p.muted
                    },
                );
                let peak_db = if meter.hold > 0.0 {
                    format!("{:.1} dB", 20.0 * meter.hold.log10())
                } else {
                    "-\u{221e} dB".into()
                };
                if cresp
                    .on_hover_text(format!(
                        "Peak {peak_db}. Lights red when the level goes over 0 dB. Click to reset."
                    ))
                    .clicked()
                {
                    if let Some(m) = state.meters.get_mut(i) {
                        m.reset();
                    }
                }
                let mut vol = t.volume;
                let r = crate::widgets::fader(ui, &mut vol, 1.25, peak, 190.0);
                // the peak hold line over the fader's meter
                if meter.hold > 0.0 {
                    let fr = r.rect;
                    let m = egui::Rect::from_min_max(
                        egui::pos2(fr.right() - 12.0, fr.top() + 8.0),
                        egui::pos2(fr.right() - 4.0, fr.bottom() - 8.0),
                    );
                    let pos = crate::widgets::fader_pos(f64::from(meter.hold), 1.25) as f32;
                    let y = m.bottom() - pos * m.height();
                    let col = if meter.hold > 1.0 {
                        egui::Color32::from_rgb(240, 70, 60)
                    } else {
                        egui::Color32::from_gray(230)
                    };
                    ui.painter().line_segment(
                        [egui::pos2(m.left(), y), egui::pos2(m.right(), y)],
                        egui::Stroke::new(1.5, col),
                    );
                }
                if r.changed() {
                    if r.drag_started() || !r.dragged() {
                        edits.push(MixEdit::Begin);
                    }
                    edits.push(MixEdit::Volume {
                        insert: i,
                        value: vol,
                    });
                }
                r.context_menu(|ui| {
                    if ui.button("Create automation clip").clicked() {
                        // the master's volume is the project's own, not its strip's
                        let (target, start) = if i == 0 {
                            (
                                crate::auto::master_target("volume"),
                                project.master_volume / 1.25,
                            )
                        } else {
                            (crate::auto::mixer_target(i, "volume"), t.volume / 1.25)
                        };
                        edits.push(MixEdit::Automate(crate::auto::AutoRequest {
                            target,
                            name: format!("{} volume", insert_name(project, i)),
                            start,
                        }));
                        ui.close();
                    }
                });
                r.on_hover_text(format!("{} ({:.1} dB)", pct(vol), db(vol)));
                let label = if t.volume <= 0.0 {
                    "-\u{221e}".to_string()
                } else {
                    format!("{:.1}", db(vol))
                };
                ui.label(RichText::new(label).weak().size(10.0));
            });
        });
    let fr = frame.response.interact(egui::Sense::click());
    if fr.clicked() {
        state.selected = i;
    }
    fr.context_menu(|ui| strip_menu(ui, project, i, channel, edits));
}

/// The right-click menu of a strip.
fn strip_menu(
    ui: &mut Ui,
    project: &Project,
    i: usize,
    channel: Selected,
    edits: &mut Vec<MixEdit>,
) {
    let label = match channel {
        Some((_, name)) => format!("Route selected channel here ({name})"),
        None => "Route selected channel here".into(),
    };
    let r = ui
        .add_enabled(channel.is_some(), egui::Button::new(label))
        .on_hover_text(
            "The channel selected in the channel rack plays through this insert (Ctrl+L)",
        );
    if r.clicked() {
        if let Some((c, _)) = channel {
            edits.push(MixEdit::Begin);
            edits.push(MixEdit::RouteChannel {
                channel: c,
                insert: i,
            });
        }
        ui.close();
    }
    let feeding: Vec<&str> = project
        .channels
        .iter()
        .filter(|c| c.mixer_track == i)
        .map(|c| c.name.as_str())
        .collect();
    if !feeding.is_empty() {
        ui.label(
            RichText::new(format!("Channels here: {}", feeding.join(", ")))
                .weak()
                .size(11.0),
        );
    }
    ui.separator();
    let t = &project.mixer[i];
    if ui
        .button(if t.invert {
            "Polarity: normal"
        } else {
            "Invert polarity"
        })
        .clicked()
    {
        edits.push(MixEdit::Begin);
        edits.push(MixEdit::Invert {
            insert: i,
            on: !t.invert,
        });
        ui.close();
    }
    if ui
        .button(if t.swap {
            "Unswap left and right"
        } else {
            "Swap left and right"
        })
        .clicked()
    {
        edits.push(MixEdit::Begin);
        edits.push(MixEdit::Swap {
            insert: i,
            on: !t.swap,
        });
        ui.close();
    }
    if !t.eq.is_flat() && ui.button("Reset EQ").clicked() {
        edits.push(MixEdit::Begin);
        for band in 0..3 {
            edits.push(MixEdit::Eq {
                insert: i,
                band,
                gain: 0.0,
            });
        }
        ui.close();
    }
}

/// The strip's built-in EQ: a small curve and low, mid and high knobs.
fn eq_strip(ui: &mut Ui, t: &las_project::MixerTrack, i: usize, edits: &mut Vec<MixEdit>) {
    let p = crate::theme::pal();
    let (r, resp) = ui.allocate_exact_size(egui::vec2(54.0, 18.0), egui::Sense::hover());
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 3.0, p.well_deep);
    painter.line_segment(
        [r.left_center(), r.right_center()],
        egui::Stroke::new(1.0, p.border),
    );
    let pts: Vec<egui::Pos2> = (0..=26)
        .map(|k| {
            let f = 20.0 * 1000f64.powf(f64::from(k) / 26.0);
            let db = eq_db(&t.eq, f).clamp(-18.0, 18.0);
            egui::pos2(
                r.left() + r.width() * k as f32 / 26.0,
                r.center().y - (db / 18.0) as f32 * (r.height() / 2.0 - 1.0),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        pts,
        egui::Stroke::new(
            1.2,
            if t.eq.is_flat() {
                p.muted
            } else {
                crate::theme::accent()
            },
        ),
    ));
    resp.on_hover_text(format!(
        "Built-in EQ: low {:+.1} dB (100 Hz), mid {:+.1} dB (1 kHz), high {:+.1} dB (8 kHz)",
        t.eq.low, t.eq.mid, t.eq.high
    ));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let names = ["Low (100 Hz)", "Mid (1 kHz)", "High (8 kHz)"];
        let cols = [
            egui::Color32::from_rgb(240, 150, 70),
            egui::Color32::from_rgb(120, 210, 120),
            egui::Color32::from_rgb(110, 170, 250),
        ];
        for (band, g) in t.eq.gains().into_iter().enumerate() {
            let mut v = g;
            let range = las_project::InsertEq::RANGE;
            let (ch, start, kr) =
                crate::steps::knob_core(ui, &mut v, (-range, range), 0.0, 17.0, cols[band]);
            kr.on_hover_text(format!(
                "EQ {}: {v:+.1} dB\nDrag up or down, double click for 0 dB",
                names[band]
            ));
            if start {
                edits.push(MixEdit::Begin);
            }
            if ch {
                edits.push(MixEdit::Eq {
                    insert: i,
                    band,
                    gain: v,
                });
            }
        }
    });
}

fn db(v: f64) -> f64 {
    if v <= 0.0 {
        -99.0
    } else {
        20.0 * v.log10()
    }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// An effect's id without its `las-` prefix.
fn short(id: &str) -> String {
    id.strip_prefix("las-").unwrap_or(id).to_string()
}

/// What a slot's button says: the effect, or the hosted plugin's name.
fn slot_label(s: &las_project::EffectSlot) -> String {
    if s.plugin_id == las_host::HOSTED {
        crate::hosted::name_of(&s.state)
    } else {
        short(&s.plugin_id)
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
    fn faders_pan_mute_solo_and_slots_change_the_project() {
        let mut p = project();
        apply(
            &mut p,
            &MixEdit::Volume {
                insert: 1,
                value: 9.0,
            },
        );
        assert_eq!(p.mixer[1].volume, 1.25, "held to the top of the fader");
        apply(
            &mut p,
            &MixEdit::Pan {
                insert: 1,
                value: -3.0,
            },
        );
        assert_eq!(p.mixer[1].pan, -1.0);
        apply(
            &mut p,
            &MixEdit::Mute {
                insert: 1,
                on: true,
            },
        );
        apply(
            &mut p,
            &MixEdit::Solo {
                insert: 1,
                on: true,
            },
        );
        assert!(p.mixer[1].mute && p.mixer[1].solo);
        // an insert that does not exist is ignored
        apply(
            &mut p,
            &MixEdit::Volume {
                insert: 999,
                value: 0.5,
            },
        );
        apply(
            &mut p,
            &MixEdit::SlotOn {
                insert: 999,
                slot: 0,
                on: false,
            },
        );
        if let Some(n) = p
            .mixer
            .iter()
            .position(|t| t.slots.iter().any(Option::is_some))
        {
            let slot = p.mixer[n].slots.iter().position(Option::is_some).unwrap();
            apply(
                &mut p,
                &MixEdit::SlotOn {
                    insert: n,
                    slot,
                    on: false,
                },
            );
            assert!(!p.mixer[n].slots[slot].as_ref().unwrap().enabled);
        }
    }

    #[test]
    fn routing_never_makes_a_loop() {
        let mut p = project();
        apply(
            &mut p,
            &MixEdit::Output {
                insert: 1,
                target: 2,
            },
        );
        assert_eq!(p.mixer[1].output, Some(2));
        // sending 2 back to 1 would loop, and so would 2 to itself
        let sends = p.mixer[2].sends.len();
        apply(
            &mut p,
            &MixEdit::Output {
                insert: 2,
                target: 1,
            },
        );
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 2,
                target: 1,
            },
        );
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 2,
                target: 2,
            },
        );
        assert_eq!(p.mixer[2].output, Some(0));
        assert_eq!(p.mixer[2].sends.len(), sends);
        assert!(would_cycle(&p, 2, 1) && !would_cycle(&p, 1, 3));
        // the master has no output to change, and cannot be a send target
        apply(
            &mut p,
            &MixEdit::Output {
                insert: 0,
                target: 1,
            },
        );
        assert_eq!(p.mixer[0].output, None);
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 1,
                target: 0,
            },
        );
        assert!(p.mixer[1].sends.iter().all(|s| s.target != 0));
    }

    #[test]
    fn sends_are_added_once_set_flagged_and_removed() {
        let mut p = project();
        let before = p.mixer[1].sends.len();
        let fresh = (3..p.mixer.len())
            .find(|t| p.mixer[1].sends.iter().all(|s| s.target != *t))
            .unwrap();
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 1,
                target: fresh,
            },
        );
        apply(
            &mut p,
            &MixEdit::AddSend {
                insert: 1,
                target: fresh,
            },
        );
        assert_eq!(p.mixer[1].sends.len(), before + 1);
        let i = p.mixer[1]
            .sends
            .iter()
            .position(|s| s.target == fresh)
            .unwrap();
        apply(
            &mut p,
            &MixEdit::SetSend {
                insert: 1,
                index: i,
                amount: 9.0,
            },
        );
        apply(
            &mut p,
            &MixEdit::SendSidechain {
                insert: 1,
                index: i,
                on: true,
            },
        );
        assert_eq!(p.mixer[1].sends[i].amount, 1.25);
        assert!(p.mixer[1].sends[i].sidechain_only);
        apply(
            &mut p,
            &MixEdit::RemoveSend {
                insert: 1,
                index: i,
            },
        );
        assert_eq!(p.mixer[1].sends.len(), before);
        apply(
            &mut p,
            &MixEdit::RemoveSend {
                insert: 1,
                index: 99,
            },
        );
    }

    #[test]
    fn effects_are_added_to_the_first_free_slot_and_removed() {
        let mut p = project();
        let n = p.mixer[2].slots.iter().filter(|s| s.is_some()).count();
        let free = p.mixer[2].slots.iter().position(|s| s.is_none()).unwrap();
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 2,
                slot: free,
                plugin: "las-hall".into(),
            },
        );
        let slots = &p.mixer[2].slots;
        assert_eq!(slots.iter().filter(|s| s.is_some()).count(), n + 1);
        let at = slots
            .iter()
            .rposition(|s| s.as_ref().is_some_and(|s| s.plugin_id == "las-hall"));
        let at = at.expect("the effect is in a slot");
        assert!(slots[at].as_ref().unwrap().enabled);
        apply(
            &mut p,
            &MixEdit::RemoveEffect {
                insert: 2,
                slot: at,
            },
        );
        assert!(p.mixer[2].slots[at].is_none());
        // unknown effects and inserts are ignored
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 2,
                slot: 0,
                plugin: "nothing".into(),
            },
        );
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: 999,
                slot: 0,
                plugin: "las-hall".into(),
            },
        );
        assert_eq!(p.mixer[2].slots.iter().filter(|s| s.is_some()).count(), n);
    }

    #[test]
    fn a_hosted_plugin_goes_in_a_free_slot_with_its_id_in_the_state() {
        let mut p = project();
        let free = p.mixer[2].slots.iter().position(|s| s.is_none()).unwrap();
        apply(
            &mut p,
            &MixEdit::AddHosted {
                insert: 2,
                slot: free,
                plugin: "vendor.plugin".into(),
            },
        );
        let s = p.mixer[2].slots[free].as_ref().unwrap();
        assert_eq!(s.plugin_id, las_host::HOSTED);
        assert_eq!(s.state["id"], "vendor.plugin");
        assert_eq!(
            slot_label(s),
            "vendor.plugin",
            "unknown plugins show their id"
        );
        // an occupied slot is left alone
        apply(
            &mut p,
            &MixEdit::AddHosted {
                insert: 2,
                slot: free,
                plugin: "other".into(),
            },
        );
        assert_eq!(
            p.mixer[2].slots[free].as_ref().unwrap().state["id"],
            "vendor.plugin"
        );
    }

    #[test]
    fn decibels_read_sensibly() {
        assert!((db(1.0)).abs() < 1e-9);
        assert!((db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(db(0.0), -99.0);
    }

    #[test]
    fn the_mixer_draws() {
        let p = project();
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let mut state = MixerState::default();
        for _ in 0..2 {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                out = draw(ui, &p, &mut state, &[0.5, 1.4], Some((0, "Kick")));
            });
        }
        assert!(out.is_empty());
        // the second insert went over 0 dB: its clip light is on, and holds its peak
        assert!(!state.meters[0].clip && state.meters[1].clip);
        assert!((state.meters[1].hold - 1.4).abs() < 1e-6);
    }

    #[test]
    fn the_built_in_eq_polarity_and_swap_are_set_and_held_to_range() {
        let mut p = project();
        apply(
            &mut p,
            &MixEdit::Eq {
                insert: 1,
                band: 0,
                gain: 6.0,
            },
        );
        apply(
            &mut p,
            &MixEdit::Eq {
                insert: 1,
                band: 2,
                gain: -99.0,
            },
        );
        apply(
            &mut p,
            &MixEdit::Eq {
                insert: 1,
                band: 1,
                gain: f64::NAN,
            },
        );
        apply(
            &mut p,
            &MixEdit::Eq {
                insert: 99,
                band: 0,
                gain: 1.0,
            },
        );
        let eq = p.mixer[1].eq;
        assert_eq!((eq.low, eq.mid, eq.high), (6.0, 0.0, -18.0));
        // the curve shows the boost in the lows and the cut in the highs
        assert!(eq_db(&eq, 40.0) > 5.0 && eq_db(&eq, 1000.0).abs() < 2.0);
        assert!(eq_db(&eq, 16000.0) < -15.0);
        apply(
            &mut p,
            &MixEdit::Invert {
                insert: 1,
                on: true,
            },
        );
        apply(
            &mut p,
            &MixEdit::Swap {
                insert: 1,
                on: true,
            },
        );
        assert!(p.mixer[1].invert && p.mixer[1].swap);
    }

    #[test]
    fn effects_move_are_replaced_and_chains_round_trip() {
        let mut p = project();
        let i = 1;
        p.mixer[i].slots = vec![None; las_project::MIXER_SLOTS];
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: i,
                slot: 0,
                plugin: "las-eq".into(),
            },
        );
        apply(
            &mut p,
            &MixEdit::AddEffect {
                insert: i,
                slot: 1,
                plugin: "las-delay".into(),
            },
        );
        let id = |p: &Project, n: usize| p.mixer[i].slots[n].as_ref().map(|s| s.plugin_id.clone());
        apply(
            &mut p,
            &MixEdit::MoveEffect {
                insert: i,
                from: 0,
                to: 1,
            },
        );
        assert_eq!(id(&p, 0).as_deref(), Some("las-delay"));
        assert_eq!(id(&p, 1).as_deref(), Some("las-eq"));
        // onto an empty slot it just moves
        apply(
            &mut p,
            &MixEdit::MoveEffect {
                insert: i,
                from: 1,
                to: 5,
            },
        );
        assert_eq!(id(&p, 1), None);
        assert_eq!(id(&p, 5).as_deref(), Some("las-eq"));
        apply(
            &mut p,
            &MixEdit::MoveEffect {
                insert: i,
                from: 5,
                to: 999,
            },
        );
        assert_eq!(id(&p, 5).as_deref(), Some("las-eq"));
        apply(
            &mut p,
            &MixEdit::ReplaceEffect {
                insert: i,
                slot: 0,
                plugin: "las-reverb".into(),
                hosted: false,
            },
        );
        assert_eq!(id(&p, 0).as_deref(), Some("las-reverb"));
        apply(
            &mut p,
            &MixEdit::ReplaceEffect {
                insert: i,
                slot: 0,
                plugin: "no-such".into(),
                hosted: false,
            },
        );
        assert_eq!(id(&p, 0).as_deref(), Some("las-reverb"));
        // save the chain, load it on another insert: same effects, new ids
        let text = chain_json(&p.mixer[i]);
        let slots = parse_chain(&text).unwrap();
        assert_eq!(slots.len(), las_project::MIXER_SLOTS);
        apply(&mut p, &MixEdit::SetChain { insert: 2, slots });
        for n in 0..las_project::MIXER_SLOTS {
            let (a, b) = (&p.mixer[i].slots[n], &p.mixer[2].slots[n]);
            assert_eq!(
                a.as_ref().map(|s| &s.plugin_id),
                b.as_ref().map(|s| &s.plugin_id)
            );
            if let (Some(a), Some(b)) = (a, b) {
                assert_ne!(a.id, b.id);
                assert_eq!(a.state, b.state);
            }
        }
        assert!(parse_chain("{}").is_err() && parse_chain("nonsense").is_err());
    }

    #[test]
    fn a_channel_is_routed_to_an_insert() {
        let mut p = project();
        apply(
            &mut p,
            &MixEdit::RouteChannel {
                channel: 0,
                insert: 2,
            },
        );
        assert_eq!(p.channels[0].mixer_track, 2);
        apply(
            &mut p,
            &MixEdit::RouteChannel {
                channel: 0,
                insert: 999,
            },
        );
        assert_eq!(p.channels[0].mixer_track, 2);
    }

    #[test]
    fn the_peak_hold_waits_then_falls_and_the_clip_light_stays_until_reset() {
        let mut m = MeterHold::default();
        m.update(0.8, 0.0);
        m.update(0.2, 1.0);
        assert_eq!(m.hold, 0.8);
        m.update(0.2, 2.0);
        assert!(m.hold < 0.8 && m.hold >= 0.2);
        assert!(!m.clip);
        m.update(1.2, 3.0);
        m.update(0.0, 10.0);
        assert!(m.clip);
        m.reset();
        assert!(!m.clip && m.hold == 0.0);
        m.update(f32::NAN, 11.0);
        assert_eq!(m.hold, 0.0);
    }
}
