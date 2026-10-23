// SPDX-License-Identifier: GPL-3.0-or-later
//! FL's Channel settings window of a sampler channel, opened by double clicking it:
//! tabs for the sample (waveform, start, pitch, root key, loop, reverse), the envelopes
//! (a volume ADSR and a filter with its own envelope, drawn as curves to drag), misc
//! settings and the arpeggiator with chord presets (the Arp tab is shared by every
//! channel). Edits go to the channel's settings through undo, like the other windows.

use crate::app::App;
use crate::theme::pal;
use crate::widgets::{knob, note_name};
use eframe::egui::{self, pos2, vec2, Rect, RichText, Sense, Stroke, Ui};
use las_engine::factory::{render_factory, Audio};
use las_engine::library::decode_wav;
use serde_json::{json, Value};

/// Minimum and maximum of the sample in each of `cols` columns, for drawing.
pub fn peaks(a: &Audio, cols: usize) -> Vec<(f32, f32)> {
    let n = a.frames();
    if n == 0 || cols == 0 {
        return vec![(0.0, 0.0); cols];
    }
    (0..cols)
        .map(|c| {
            let (from, to) = (
                c * n / cols,
                ((c + 1) * n / cols).max(c * n / cols + 1).min(n),
            );
            let mut lo = 0f32;
            let mut hi = 0f32;
            for ch in &a.channels {
                for &v in &ch[from..to] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            (lo, hi)
        })
        .collect()
}

/// A number of a state, or a default.
pub fn num(state: &Value, key: &str, default: f64) -> f64 {
    state.get(key).and_then(Value::as_f64).unwrap_or(default)
}

/// Set a key of a channel's state, making the state an object if it is not one.
pub fn set_key(state: &mut Value, key: &str, v: Value) {
    if !state.is_object() {
        *state = json!({});
    }
    state[key] = v;
}

/// One knob of the window: what it sets, its range, and how its value is shown.
struct Knob {
    key: &'static str,
    label: &'static str,
    lo: f64,
    hi: f64,
    default: f64,
    value: f64,
    log: bool,
    show: fn(f64) -> String,
}

/// The tabs of the Channel settings window, as FL's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Sample,
    Envelope,
    Misc,
    Arp,
}

impl Tab {
    pub const ALL: [(Tab, &'static str, &'static str); 4] = [
        (
            Tab::Sample,
            "Sample",
            "The sample: its waveform, where it starts, pitch, root key, loop and reverse",
        ),
        (
            Tab::Envelope,
            "Envelope",
            "How the volume and the filter move over each note: drag the points of the curves",
        ),
        (
            Tab::Misc,
            "Misc",
            "Velocity, boost and whether a note cuts the one before",
        ),
        (
            Tab::Arp,
            "Arp",
            "The arpeggiator and chord presets: held notes play one after another",
        ),
    ];

    /// The tab a name stands for (`--channel-tab envelope`).
    pub fn from_name(s: &str) -> Option<Tab> {
        Tab::ALL
            .iter()
            .find(|t| t.1.eq_ignore_ascii_case(s))
            .map(|t| t.0)
    }
}

thread_local! {
    /// A tab the window is to switch to on its next frame (from a menu, or
    /// `--channel-tab` for screenshots).
    pub static ASK_TAB: std::cell::Cell<Option<Tab>> = const { std::cell::Cell::new(None) };
}

/// Where a duration (seconds, up to `max`) sits along a quarter of an envelope's width
/// (`w`): the square root, so short times get room.
fn seg_width(t: f64, max: f64, w: f32) -> f32 {
    ((t / max).clamp(0.0, 1.0).sqrt() as f32) * w
}

fn seg_time(dx: f32, max: f64, w: f32, min: f64) -> f64 {
    let f = f64::from((dx / w).clamp(0.0, 1.0));
    (f * f * max).clamp(min, max)
}

/// The points of an ADSR curve in `rect`: start, attack peak, decay end (at the
/// sustain level), sustain end, release end.
pub fn adsr_points(rect: Rect, a: f64, d: f64, s: f64, r: f64, max: f64) -> [egui::Pos2; 5] {
    let q = rect.width() / 4.0;
    let y_of = |v: f64| rect.bottom() - (v.clamp(0.0, 1.0) as f32) * rect.height();
    let x0 = rect.left();
    let x1 = x0 + seg_width(a, max, q);
    let x2 = x1 + seg_width(d, max, q);
    let x3 = x2 + q;
    let x4 = x3 + seg_width(r, max, q);
    [
        pos2(x0, rect.bottom()),
        pos2(x1, rect.top()),
        pos2(x2, y_of(s)),
        pos2(x3, y_of(s)),
        pos2(x4, rect.bottom()),
    ]
}

/// An ADSR envelope drawn as a curve with points to drag, as FL's envelope editor:
/// the peak sets the attack, the next point the decay and sustain, the last the
/// release. Times are seconds (up to `max`), sustain 0 to 1. Returns whether it changed.
#[allow(clippy::too_many_arguments)]
pub fn adsr_editor(
    ui: &mut Ui,
    id: &str,
    a: &mut f64,
    d: &mut f64,
    s: &mut f64,
    r: &mut f64,
    max: f64,
    color: egui::Color32,
) -> bool {
    let p = pal();
    let size = vec2(ui.available_width().clamp(220.0, 420.0), 110.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let inner = Rect::from_min_max(rect.min + vec2(10.0, 18.0), rect.max - vec2(10.0, 8.0));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, p.well_deep);
    // a grid of quarters, as the segments are laid out
    for i in 1..4 {
        let x = inner.left() + inner.width() * i as f32 / 4.0;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, p.grid),
        );
    }
    let pts = adsr_points(inner, *a, *d, *s, *r, max);
    let mut fill = pts.to_vec();
    fill.push(pts[4]);
    painter.add(egui::Shape::convex_polygon(
        vec![pts[0], pts[1], pts[2], pts[3], pts[4]],
        color.gamma_multiply(0.18),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::line(fill, Stroke::new(2.0, color)));
    let key = egui::Id::unique(("adsr-drag", id));
    let hover = resp.hover_pos();
    let near = |q: egui::Pos2| {
        [1usize, 2, 4]
            .into_iter()
            .min_by(|&i, &j| {
                pts[i]
                    .distance(q)
                    .partial_cmp(&pts[j].distance(q))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(1)
    };
    let hot = hover.map(near);
    for (i, label) in [(1usize, "A"), (2, "D/S"), (4, "R")] {
        let lit = hot == Some(i);
        painter.circle_filled(pts[i], if lit { 6.0 } else { 4.5 }, color);
        painter.circle_stroke(pts[i], 6.5, Stroke::new(1.0, p.text));
        painter.text(
            pts[i]
                + if i == 2 {
                    vec2(12.0, 10.0)
                } else {
                    vec2(0.0, -12.0)
                },
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(10.0),
            p.text2,
        );
    }
    let mut changed = false;
    if resp.drag_started() {
        if let Some(q) = resp.interact_pointer_pos() {
            ui.data_mut(|m| m.insert_temp(key, near(q)));
        }
    }
    if resp.dragged() {
        let which: usize = ui.data(|m| m.get_temp(key)).unwrap_or(1);
        if let Some(q) = resp.interact_pointer_pos() {
            let quarter = inner.width() / 4.0;
            match which {
                1 => *a = seg_time(q.x - pts[0].x, max, quarter, 0.001),
                2 => {
                    *d = seg_time(q.x - pts[1].x, max, quarter, 0.001);
                    *s = f64::from(((inner.bottom() - q.y) / inner.height()).clamp(0.0, 1.0));
                }
                _ => *r = seg_time(q.x - pts[3].x, max, quarter, 0.002),
            }
            changed = true;
        }
    }
    let ms = |t: f64| {
        if t >= 1.0 {
            format!("{t:.2} s")
        } else {
            format!("{:.0} ms", t * 1000.0)
        }
    };
    resp.on_hover_text(format!(
        "Drag the points: A is the attack, D/S the decay and sustain level, R the release\nAttack {}, decay {}, sustain {:.0}%, release {}",
        ms(*a),
        ms(*d),
        *s * 100.0,
        ms(*r)
    ));
    changed
}

/// The arpeggiator modes: state value (none for off), label, tooltip.
pub const ARP_MODES: [(&str, &str, &str); 5] = [
    ("", "Off", "Notes play as they are"),
    (
        "up",
        "Up",
        "Held notes play one after another, lowest first",
    ),
    (
        "down",
        "Down",
        "Held notes play one after another, highest first",
    ),
    ("updown", "Up & down", "Up, then back down"),
    ("random", "Random", "Held notes play in a random order"),
];

/// The arpeggiator's step times: steps (a step is a sixteenth note), label.
pub const ARP_TIMES: [(f64, &str); 7] = [
    (0.5, "1/2 step"),
    (1.0, "1 step"),
    (2.0, "2 steps"),
    (3.0, "3 steps"),
    (4.0, "4 steps (1 beat)"),
    (8.0, "8 steps"),
    (16.0, "16 steps (1 bar)"),
];

/// The Arp tab, shared by every channel: FL's arpeggiator and chord presets. Edits
/// `state.arp`; returns whether it changed.
pub fn arp_tab(ui: &mut Ui, state: &mut Value) -> bool {
    let p = pal();
    if !state.is_object() {
        *state = json!({});
    }
    if !state.get("arp").is_some_and(Value::is_object) {
        state["arp"] = json!({});
    }
    let arp = &mut state["arp"];
    let on = arp.get("on").and_then(Value::as_bool).unwrap_or(false);
    let mode = if on {
        arp.get("mode")
            .and_then(Value::as_str)
            .unwrap_or("up")
            .to_string()
    } else {
        String::new()
    };
    let mut changed = false;
    ui.label(RichText::new("ARPEGGIATOR").strong().size(12.0));
    ui.label(
        RichText::new("Hold a chord and its notes play one after another, in time with the song.")
            .weak()
            .size(11.0),
    );
    ui.horizontal_wrapped(|ui| {
        ui.label("Direction");
        for (m, label, tip) in ARP_MODES {
            if ui
                .selectable_label(mode == m, label)
                .on_hover_text(tip)
                .clicked()
            {
                if m.is_empty() {
                    arp["on"] = json!(false);
                } else {
                    arp["on"] = json!(true);
                    arp["mode"] = json!(m);
                }
                changed = true;
            }
        }
    });
    ui.add_enabled_ui(on, |ui| {
        egui::Grid::new("arp-grid")
            .num_columns(2)
            .spacing(vec2(12.0, 6.0))
            .show(ui, |ui| {
                ui.label("Range")
                    .on_hover_text("How many octaves the notes climb through");
                let mut oct = num(arp, "octaves", 1.0).round().clamp(1.0, 4.0) as i32;
                if ui
                    .add(egui::Slider::new(&mut oct, 1..=4).suffix(" oct"))
                    .on_hover_text("How many octaves the notes climb through")
                    .changed()
                {
                    arp["octaves"] = json!(oct);
                    changed = true;
                }
                ui.end_row();
                ui.label("Time")
                    .on_hover_text("How long each arpeggio note is, in steps");
                let steps = num(arp, "div", 0.25) * 4.0;
                let shown = ARP_TIMES
                    .iter()
                    .find(|t| (t.0 - steps).abs() < 1e-6)
                    .map_or_else(|| format!("{steps} steps"), |t| t.1.to_string());
                egui::ComboBox::from_id_salt("arp-time")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (st, label) in ARP_TIMES {
                            if ui
                                .selectable_label((st - steps).abs() < 1e-6, label)
                                .clicked()
                            {
                                arp["div"] = json!(st / 4.0);
                                changed = true;
                            }
                        }
                    });
                ui.end_row();
                ui.label("Gate")
                    .on_hover_text("How much of each step a note lasts");
                let mut gate = (num(arp, "gate", 0.7) * 100.0).clamp(5.0, 100.0);
                if ui
                    .add(egui::Slider::new(&mut gate, 5.0..=100.0).suffix(" %"))
                    .on_hover_text("How much of each step a note lasts")
                    .changed()
                {
                    arp["gate"] = json!(gate / 100.0);
                    changed = true;
                }
                ui.end_row();
                ui.label("Repeat")
                    .on_hover_text("How many times each note plays before the next");
                let mut rep = num(arp, "repeat", 1.0).round().clamp(1.0, 8.0) as i32;
                if ui
                    .add(egui::Slider::new(&mut rep, 1..=8).suffix("x"))
                    .on_hover_text("How many times each note plays before the next")
                    .changed()
                {
                    arp["repeat"] = json!(rep);
                    changed = true;
                }
                ui.end_row();
            });
    });
    ui.add_space(10.0);
    ui.label(RichText::new("CHORD").strong().size(12.0));
    ui.label(
        RichText::new("Every note you play becomes this chord (the arpeggiator then plays its notes in turn).")
            .weak()
            .size(11.0),
    );
    let chord = arp
        .get("chord")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    ui.horizontal_wrapped(|ui| {
        for (id, name, notes) in las_engine::arp::CHORDS {
            let tip = if notes.len() == 1 {
                "Notes play as they are".to_string()
            } else {
                let names: Vec<String> = notes
                    .iter()
                    .map(|n| {
                        note_name((60 + n) as u8)
                            .trim_end_matches(char::is_numeric)
                            .to_string()
                    })
                    .collect();
                format!("On C: {}", names.join(" "))
            };
            if ui
                .selectable_label(chord == id, name)
                .on_hover_text(tip)
                .clicked()
            {
                arp["chord"] = json!(id);
                changed = true;
            }
        }
    });
    let _ = p;
    changed
}

impl App {
    /// The channel's sample as audio, if it has one the app can decode.
    fn sample_audio(&self, channel: usize) -> Option<Audio> {
        let l = self.project.loaded.as_ref()?;
        let id = l.project.channels.get(channel)?.sample_id.clone()?;
        if let Some(a) = render_factory(&id, 44100.0) {
            return Some(a);
        }
        decode_wav(&l.samples.get(&id)?.data)
    }

    /// FL's Channel settings window of a sampler channel: Sample, Envelope, Misc and
    /// Arp tabs. Edits go to the channel's settings through undo.
    pub(crate) fn sampler_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            ui.label("This channel no longer exists.");
            return;
        };
        let state = c.state.clone();
        let name = c.name.clone();
        let sample = c.sample_id.clone().unwrap_or_default();
        let p = pal();
        let tab_id = egui::Id::unique(("chan-settings-tab", channel));
        let mut tab: Tab = ui.data(|m| m.get_temp(tab_id)).unwrap_or_default();
        if let Some(t) = ASK_TAB.with(std::cell::Cell::take) {
            tab = t;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&name)
                    .strong()
                    .size(16.0)
                    .color(p.accent_text),
            );
            ui.label(RichText::new("Channel settings").weak());
        });
        ui.horizontal(|ui| {
            for (t, label, tip) in Tab::ALL {
                if ui
                    .selectable_label(tab == t, RichText::new(label).strong())
                    .on_hover_text(tip)
                    .clicked()
                {
                    tab = t;
                }
            }
        });
        ui.data_mut(|m| m.insert_temp(tab_id, tab));
        ui.separator();
        let mut changes: Vec<(&'static str, Value)> = Vec::new();
        let mut new_state: Option<Value> = None;
        match tab {
            Tab::Sample => self.sample_tab(ui, channel, &state, &sample, &mut changes),
            Tab::Envelope => envelope_tab(ui, &state, &mut changes),
            Tab::Misc => misc_tab(ui, &state, &mut changes),
            Tab::Arp => {
                let mut st = state.clone();
                if arp_tab(ui, &mut st) {
                    new_state = Some(st);
                }
            }
        }
        if let Some(st) = new_state {
            self.edit_state(channel, "sampler", st);
        } else if !changes.is_empty() {
            let mut st = state.clone();
            for (k, v) in changes {
                set_key(&mut st, k, v);
            }
            self.edit_state(channel, "sampler", st);
        }
    }

    /// Open a channel's Channel settings: a sampler's own window (on its Arp tab), or
    /// the shared window for the other instruments.
    pub(crate) fn open_channel_settings(&mut self, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            return;
        };
        if c.plugin_id == "sampler" {
            self.open_channel(channel);
            ASK_TAB.with(|t| t.set(Some(Tab::Arp)));
        } else {
            self.project.channel = channel;
            self.shell.chan_settings = Some(channel);
            crate::raise::raise(crate::raise::Win::ChanSettings);
        }
    }

    /// The shared Channel settings of any channel: the Arp tab.
    pub(crate) fn shared_settings(&mut self, ui: &mut Ui, channel: usize) {
        let Some(state) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(channel))
            .map(|c| c.state.clone())
        else {
            return;
        };
        let mut st = state;
        if arp_tab(ui, &mut st) {
            self.project.modify_coalesced("chan-settings", move |pr| {
                if let Some(ch) = pr.channels.get_mut(channel) {
                    ch.state = st;
                }
            });
        }
    }

    /// The Sample tab: the waveform with its start and end markers, pitch and root key,
    /// loop and reverse.
    fn sample_tab(
        &self,
        ui: &mut Ui,
        channel: usize,
        state: &Value,
        sample: &str,
        changes: &mut Vec<(&'static str, Value)>,
    ) {
        let p = pal();
        ui.label(
            RichText::new(if sample.is_empty() {
                "No sample. Drag one from the browser onto the channel.".to_string()
            } else {
                format!("Sample: {sample}")
            })
            .weak()
            .size(11.0),
        );
        let audio = self.sample_audio(channel);
        let (rect, resp) = ui.allocate_exact_size(
            vec2(ui.available_width().max(360.0), 120.0),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, p.well_deep);
        let (mut start, mut end) = (num(state, "start", 0.0), num(state, "end", 1.0));
        let reverse = state.get("reverse").and_then(Value::as_bool) == Some(true);
        if let Some(a) = &audio {
            let cols = (rect.width() as usize).max(1);
            let mid = rect.center().y;
            let half = rect.height() * 0.45;
            let mut pk = peaks(a, cols);
            if reverse {
                pk.reverse();
            }
            for (i, (lo, hi)) in pk.into_iter().enumerate() {
                let x = rect.left() + i as f32;
                painter.line_segment(
                    [pos2(x, mid - hi * half), pos2(x, mid - lo * half)],
                    Stroke::new(1.0, p.accent.gamma_multiply(0.9)),
                );
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "no waveform",
                egui::FontId::proportional(12.0),
                p.muted,
            );
        }
        let x_of = |f: f64| rect.left() + f as f32 * rect.width();
        // what is cut off at either end is dimmed
        painter.rect_filled(
            Rect::from_min_max(rect.min, pos2(x_of(start), rect.bottom())),
            0.0,
            egui::Color32::from_black_alpha(150),
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(x_of(end), rect.top()), rect.max),
            0.0,
            egui::Color32::from_black_alpha(150),
        );
        for (f, label) in [(start, "Start"), (end, "End")] {
            painter.line_segment(
                [pos2(x_of(f), rect.top()), pos2(x_of(f), rect.bottom())],
                Stroke::new(2.0, p.text),
            );
            painter.text(
                pos2(x_of(f) + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(10.0),
                p.text,
            );
        }
        if resp.dragged() || resp.clicked() {
            if let Some(q) = resp.interact_pointer_pos() {
                let f = f64::from(((q.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
                // the marker nearer to the pointer moves
                if (f - start).abs() <= (f - end).abs() {
                    start = f.min(end - 0.001);
                    changes.push(("start", json!(start)));
                } else {
                    end = f.max(start + 0.001);
                    changes.push(("end", json!(end)));
                }
            }
        }
        resp.on_hover_text("Drag the Start and End markers: the sample plays between them");
        ui.add_space(6.0);
        let knobs = [
            Knob {
                key: "start",
                label: "Start",
                lo: 0.0,
                hi: 1.0,
                default: 0.0,
                value: start,
                log: false,
                show: |v| format!("{:.0}%", v * 100.0),
            },
            Knob {
                key: "tune",
                label: "Pitch (st)",
                lo: -24.0,
                hi: 24.0,
                default: 0.0,
                value: num(state, "tune", 0.0),
                log: false,
                show: |v| format!("{v:+.0} semitones"),
            },
            Knob {
                key: "fine",
                label: "Fine (ct)",
                lo: -100.0,
                hi: 100.0,
                default: 0.0,
                value: num(state, "fine", 0.0),
                log: false,
                show: |v| format!("{v:+.0} cents"),
            },
        ];
        ui.horizontal_wrapped(|ui| {
            for k in knobs {
                let mut v = k.value;
                if knob(ui, &mut v, k.lo..=k.hi, k.default, k.label, k.log, &k.show) {
                    // semitones are whole
                    let v = if k.key == "tune" { v.round() } else { v };
                    changes.push((k.key, json!(v)));
                }
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let root = num(state, "root", 60.0).round().clamp(0.0, 127.0) as i32;
                    let mut r = root;
                    ui.label("Root key")
                        .on_hover_text("The key at which the sample plays at its own pitch");
                    if ui
                        .add(
                            egui::DragValue::new(&mut r)
                                .range(0..=127)
                                .custom_formatter(|n, _| note_name(n as u8)),
                        )
                        .on_hover_text(
                            "Drag or type: the key at which the sample plays at its own pitch",
                        )
                        .changed()
                    {
                        changes.push(("root", json!(r)));
                    }
                });
                for (key, label, tip) in [
                    ("loop", "Loop", "The sample repeats while a note is held"),
                    ("reverse", "Reverse", "The sample plays backwards"),
                ] {
                    let mut on = state.get(key).and_then(Value::as_bool).unwrap_or(false);
                    if ui.checkbox(&mut on, label).on_hover_text(tip).changed() {
                        changes.push((key, json!(on)));
                    }
                }
            });
        });
    }
}

/// The Envelope tab: the volume ADSR, and the filter with its own envelope.
fn envelope_tab(ui: &mut Ui, state: &Value, changes: &mut Vec<(&'static str, Value)>) {
    let p = pal();
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        ui.label(RichText::new("VOLUME").strong().size(12.0));
        let (mut a, mut d, mut s, mut r) = (
            num(state, "attack", 0.001),
            num(state, "decay", 0.3),
            num(state, "sustain", 1.0),
            num(state, "release", 0.08),
        );
        if adsr_editor(ui, "vol", &mut a, &mut d, &mut s, &mut r, 5.0, p.accent) {
            changes.extend([
                ("attack", json!(a)),
                ("decay", json!(d)),
                ("sustain", json!(s)),
                ("release", json!(r)),
            ]);
        }
        env_knobs(
            ui,
            state,
            ["attack", "decay", "sustain", "release"],
            [0.001, 0.3, 1.0, 0.08],
            changes,
        );

        let ui = &mut cols[1];
        ui.horizontal(|ui| {
            ui.label(RichText::new("FILTER").strong().size(12.0));
            let mut kind = state
                .get("filterType")
                .and_then(Value::as_str)
                .unwrap_or("lowpass")
                .to_string();
            egui::ComboBox::from_id_salt("sampler-filter")
                .selected_text(match kind.as_str() {
                    "highpass" => "High pass",
                    "bandpass" => "Band pass",
                    _ => "Low pass",
                })
                .show_ui(ui, |ui| {
                    for (k, n) in [
                        ("lowpass", "Low pass"),
                        ("highpass", "High pass"),
                        ("bandpass", "Band pass"),
                    ] {
                        if ui.selectable_value(&mut kind, k.to_string(), n).changed() {
                            changes.push(("filterType", json!(k)));
                        }
                    }
                })
                .response
                .on_hover_text("The kind of filter");
        });
        let (mut a, mut d, mut s, mut r) = (
            num(state, "fltAttack", 0.01),
            num(state, "fltDecay", 0.4),
            num(state, "fltSustain", 0.0),
            num(state, "fltRelease", 0.3),
        );
        let amount = num(state, "fltEnvAmt", 0.0);
        let color = if amount == 0.0 { p.muted } else { p.warn };
        if adsr_editor(ui, "flt", &mut a, &mut d, &mut s, &mut r, 5.0, color) {
            changes.extend([
                ("fltAttack", json!(a)),
                ("fltDecay", json!(d)),
                ("fltSustain", json!(s)),
                ("fltRelease", json!(r)),
            ]);
        }
        ui.horizontal_wrapped(|ui| {
            let knobs = [
                Knob {
                    key: "cutoff",
                    label: "Cutoff",
                    lo: 20.0,
                    hi: 20000.0,
                    default: 20000.0,
                    value: num(state, "cutoff", 20000.0),
                    log: true,
                    show: |v| format!("{v:.0} Hz"),
                },
                Knob {
                    key: "res",
                    label: "Resonance",
                    lo: 0.1,
                    hi: 18.0,
                    default: 0.7,
                    value: num(state, "res", 0.7),
                    log: true,
                    show: |v| format!("{v:.1}"),
                },
                Knob {
                    key: "fltEnvAmt",
                    label: "Env amount",
                    lo: -6.0,
                    hi: 6.0,
                    default: 0.0,
                    value: amount,
                    log: false,
                    show: |v| {
                        if v.abs() < 0.05 {
                            "off".into()
                        } else {
                            format!("{v:+.1} octaves")
                        }
                    },
                },
            ];
            for k in knobs {
                let mut v = k.value;
                if knob(ui, &mut v, k.lo..=k.hi, k.default, k.label, k.log, &k.show) {
                    let v = if k.key == "fltEnvAmt" && v.abs() < 0.05 {
                        0.0
                    } else {
                        v
                    };
                    changes.push((k.key, json!(v)));
                }
            }
        });
        if amount == 0.0 {
            ui.label(
                RichText::new("Turn Env amount up or down to let the curve above move the cutoff.")
                    .weak()
                    .size(11.0),
            );
        }
        env_knobs(
            ui,
            state,
            ["fltAttack", "fltDecay", "fltSustain", "fltRelease"],
            [0.01, 0.4, 0.0, 0.3],
            changes,
        );
    });
}

/// Attack, decay, sustain and release knobs for four state keys.
fn env_knobs(
    ui: &mut Ui,
    state: &Value,
    keys: [&'static str; 4],
    defaults: [f64; 4],
    changes: &mut Vec<(&'static str, Value)>,
) {
    let ms = |v: f64| {
        if v >= 1.0 {
            format!("{v:.2} s")
        } else {
            format!("{:.0} ms", v * 1000.0)
        }
    };
    ui.horizontal_wrapped(|ui| {
        for (i, label) in ["Attack", "Decay", "Sustain", "Release"].iter().enumerate() {
            let mut v = num(state, keys[i], defaults[i]);
            let changed = if i == 2 {
                knob(ui, &mut v, 0.0..=1.0, defaults[i], label, false, &|v| {
                    format!("{:.0}%", v * 100.0)
                })
            } else {
                knob(ui, &mut v, 0.001..=5.0, defaults[i], label, true, &ms)
            };
            if changed {
                changes.push((keys[i], json!(v)));
            }
        }
    });
}

/// The Misc tab: velocity, boost and cut itself.
fn misc_tab(ui: &mut Ui, state: &Value, changes: &mut Vec<(&'static str, Value)>) {
    ui.horizontal_wrapped(|ui| {
        let mut v = num(state, "velToVol", 0.7);
        if knob(ui, &mut v, 0.0..=1.0, 0.7, "Velocity", false, &|v| {
            format!("{:.0}% of the volume follows velocity", v * 100.0)
        }) {
            changes.push(("velToVol", json!(v)));
        }
        let mut b = num(state, "boost", 0.0);
        if knob(ui, &mut b, -12.0..=12.0, 0.0, "Boost", false, &|v| {
            format!("{v:+.1} dB")
        }) {
            changes.push(("boost", json!(b)));
        }
    });
    let mut on = state.get("cut").and_then(Value::as_bool).unwrap_or(false);
    if ui
        .checkbox(&mut on, "Cut itself")
        .on_hover_text("A new note stops the one still sounding, as an open hat cuts")
        .changed()
    {
        changes.push(("cut", json!(on)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_cover_the_sample_in_columns() {
        let a = Audio {
            channels: vec![vec![0.0, 0.5, -0.25, 1.0, 0.0, 0.0, -1.0, 0.0]],
            sample_rate: 8000.0,
        };
        let p = peaks(&a, 4);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], (0.0, 0.5));
        assert_eq!(p[1], (-0.25, 1.0));
        assert_eq!(p[3], (-1.0, 0.0));
        // more columns than frames never reads past the end
        assert_eq!(peaks(&a, 20).len(), 20);
        assert_eq!(
            peaks(
                &Audio {
                    channels: vec![],
                    sample_rate: 1.0
                },
                3
            ),
            vec![(0.0, 0.0); 3]
        );
    }

    #[test]
    fn the_envelope_curve_maps_times_to_points_and_back() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 100.0));
        let pts = adsr_points(r, 1.25, 5.0, 0.5, 0.0, 5.0);
        assert_eq!(pts[0], pos2(0.0, 100.0));
        // a quarter of the full time is half a quarter of the width (the square root)
        assert!((pts[1].x - 50.0).abs() < 1e-3);
        assert!((pts[2].x - 150.0).abs() < 1e-3);
        assert!((pts[2].y - 50.0).abs() < 1e-3, "sustain at half");
        assert!((pts[3].x - 250.0).abs() < 1e-3);
        // dragging back gives the time again
        assert!((seg_time(50.0, 5.0, 100.0, 0.001) - 1.25).abs() < 1e-9);
        assert_eq!(seg_time(-10.0, 5.0, 100.0, 0.001), 0.001);
    }

    #[test]
    fn tabs_are_named_and_the_arp_tab_writes_what_the_engine_reads() {
        assert_eq!(Tab::from_name("envelope"), Some(Tab::Envelope));
        assert_eq!(Tab::from_name("ARP"), Some(Tab::Arp));
        assert_eq!(Tab::from_name("nope"), None);
        // drawing the tab changes nothing until it is used
        let ctx = egui::Context::default();
        let mut st = json!({ "arp": { "on": true, "mode": "down", "div": 0.5 } });
        let mut out = ctx.run_ui(Default::default(), |ui| assert!(!arp_tab(ui, &mut st)));
        out.textures_delta.clear();
        let a = las_engine::arp::ArpState::from_json(st.get("arp"));
        assert!(a.on);
        assert_eq!(a.mode, "down");
        assert_eq!(a.div, 0.5);
        // every chord the tab offers is one the engine knows
        for (id, _, notes) in las_engine::arp::CHORDS {
            assert_eq!(las_engine::arp::chord_intervals(id), notes);
        }
    }

    #[test]
    fn settings_are_read_and_written_by_key() {
        let mut s = Value::Null;
        set_key(&mut s, "cutoff", json!(800.0));
        assert_eq!(num(&s, "cutoff", 1.0), 800.0);
        assert_eq!(num(&s, "res", 0.7), 0.7);
        set_key(&mut s, "loop", json!(true));
        assert_eq!(s["loop"], true);
    }

    #[test]
    fn a_sampler_with_the_settings_of_the_window_sounds_different() {
        // the keys the window writes are the ones the engine reads: a low cutoff takes the
        // top off a hat, and a later start skips its attack
        use las_project::create::default_project;
        let render = |state: Value| {
            let mut p = default_project();
            let hat = p.channels[2].id.clone();
            p.channels[2].state = state;
            p.patterns[0].notes.retain(|k, _| *k == hat);
            las_engine::render_mix_with(
                &p,
                &Default::default(),
                &las_engine::RenderOptions {
                    sample_rate: 24000.0,
                    tail: 0.1,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let energy = |r: &las_engine::Rendered| r.left.iter().map(|s| s * s).sum::<f32>();
        let bright = render(json!({}));
        let dull = render(json!({ "cutoff": 300.0 }));
        let late = render(json!({ "start": 0.8 }));
        assert!(
            energy(&dull) < energy(&bright) * 0.5,
            "{} vs {}",
            energy(&dull),
            energy(&bright)
        );
        assert!(energy(&late) < energy(&bright) * 0.5);
    }
}
