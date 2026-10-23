// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave's window: the patch as modules on a canvas, joined by cables. Drag a module by
//! its title; drag from an output (the dots on the right) to any row of a module (an
//! input or a parameter; the places it can go are ringed) to plug a cable in, or from a
//! plugged input to take its cable out and plug it elsewhere. Click a cable's handle to
//! select it (its amount is set in the bar above, Delete pulls it out), double click to
//! pull it out, right click for its menu. Drag a parameter's bar up or down to set it
//! (shift for fine steps, a double or ctrl click for its default), or turn the wheel
//! over it. Ctrl and the wheel zoom; the wheel elsewhere or a drag of the background
//! scrolls; Fit frames every module. Values change the sound as they move (the player
//! takes them live); new modules or cables build the player again. A drag is one undo
//! step.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use las_dsp::modular::{
    factory, param_id, Cable, Kind, Node, ParamSpec, Patch, CATEGORIES, MAX_GLIDE, MAX_VOICES,
};
use serde_json::{json, Value};

const NODE_W: f32 = 180.0;
const TITLE_H: f32 = 22.0;
const ROW_H: f32 = 20.0;
const PORT_R: f32 = 5.0;
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.0;

/// A whole drag of the mouse turns a control through its range over this many points.
const DRAG_PX: f32 = 200.0;
/// Points of wheel scrolling that make one notch.
const WHEEL_NOTCH: f32 = 50.0;

/// A cable, by its ends (an index would point at another cable after an undo).
type CableKey = (u32, String, u32, String);

fn cable_key(c: &Cable) -> CableKey {
    (c.from, c.output.clone(), c.to, c.input.clone())
}

/// The control being turned: its module, its parameter and where along its range
/// (0 to 1) the drag has got to, so that a whole number moves by its own steps.
struct Knob {
    node: u32,
    param: usize,
    at: f64,
}

/// A control turned by a drag of `dx`, `dy` points: up and right turn it up, a whole
/// range in [`DRAG_PX`] points (ten times that with shift).
pub fn knob_turn(at: f64, dx: f32, dy: f32, fine: bool) -> f64 {
    let speed = if fine { 0.1 } else { 1.0 };
    (at + f64::from((dx - dy) / DRAG_PX) * speed).clamp(0.0, 1.0)
}

/// A control (not a whole number) turned by `points` of wheel scrolling: 2% of its
/// range a notch, a tenth of that with shift.
pub fn wheel_turn(ps: &ParamSpec, v: f64, points: f32, fine: bool) -> f64 {
    let speed = if fine { 0.002 } else { 0.02 };
    let n = ps.normalize(v) + f64::from(points / WHEEL_NOTCH) * speed;
    ps.denormalize(n.clamp(0.0, 1.0))
}

/// What the editor remembers between frames.
#[derive(Default)]
pub struct WeaveEditor {
    /// How far the canvas is scrolled, in screen points.
    pan: Vec2,
    /// How big the modules are drawn (0 is taken as 1).
    zoom: f32,
    /// A cable being drawn from an output: the node and the output's place.
    dragging: Option<(u32, usize)>,
    /// The cable whose amount is being set.
    selected: Option<CableKey>,
    /// The module picked (Delete takes it out).
    node: Option<u32>,
    /// The control being dragged.
    knob: Option<Knob>,
    /// The pointer is down and has already made an undo step.
    gesture: bool,
    /// When the last edit was (the input clock, in seconds).
    last_edit: f64,
    /// Wheel scrolling over a whole-number control not yet a notch.
    wheel: f32,
    /// The patch as this editor left it last frame, to see changes from outside.
    last: Option<Patch>,
    /// Where the canvas's point (0, 0) was drawn last frame.
    origin: Pos2,
    /// What was sent to the player live (for the tests).
    #[cfg(test)]
    pub sent: Vec<(usize, Patch)>,
    /// Frame every module on the next frame.
    fit: bool,
    /// The channel last shown, to frame a patch the first time it opens.
    seen: Option<usize>,
    /// The size of the canvas last frame.
    view: Vec2,
}

#[cfg(test)]
impl WeaveEditor {
    /// Where output `o` of module `node` was drawn last frame.
    pub fn out_at(&self, p: &Patch, node: u32, o: usize) -> Pos2 {
        out_pos(p.node(node).expect("a module"), self.origin, o, self.z())
    }

    /// The middle of row `k` (an input, then the parameters) of module `node`.
    pub fn row_at(&self, p: &Patch, node: u32, k: usize) -> Pos2 {
        let n = p.node(node).expect("a module");
        let r = node_rect(n, self.origin, self.z());
        row_rect(&r, n.kind.spec().outputs.len() + k, self.z()).center()
    }

    /// The dot of input `k` of module `node`.
    pub fn in_at(&self, p: &Patch, node: u32, k: usize) -> Pos2 {
        in_pos(p.node(node).expect("a module"), self.origin, k, self.z())
    }

    /// The handle of cable `i`.
    pub fn cable_at(&self, p: &Patch, i: usize) -> Pos2 {
        let c = &p.cables[i];
        let (a, b) = (p.node(c.from).unwrap(), p.node(c.to).unwrap());
        let o = a.kind.output_index(&c.output).unwrap();
        let k = b.kind.input_index(&c.input).unwrap();
        mid(self.out_at(p, c.from, o), self.in_at(p, c.to, k), self.z())
    }

    /// The control being dragged now: (module, parameter).
    pub fn turning(&self) -> Option<(u32, usize)> {
        self.knob.as_ref().map(|k| (k.node, k.param))
    }
}

impl WeaveEditor {
    /// Let go of everything picked or held (a new patch came).
    fn forget(&mut self) {
        self.selected = None;
        self.node = None;
        self.knob = None;
        self.dragging = None;
    }

    fn z(&self) -> f32 {
        if self.zoom <= 0.0 {
            1.0
        } else {
            self.zoom
        }
    }
}

pub fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Keys => "Keys",
        Kind::Osc => "Oscillator",
        Kind::Fm => "FM operator",
        Kind::Noise => "Noise",
        Kind::Macro => "Macro",
        Kind::Lfo => "LFO",
        Kind::Env => "Envelope",
        Kind::Hold => "Sample & hold",
        Kind::Seq => "Step sequencer",
        Kind::Slew => "Slew",
        Kind::Filter => "Filter",
        Kind::Ladder => "Ladder filter",
        Kind::Shaper => "Wavefolder",
        Kind::Ring => "Ring mod",
        Kind::Delay => "Delay",
        Kind::Chorus => "Chorus",
        Kind::Reverb => "Reverb",
        Kind::Vca => "VCA",
        Kind::Mix => "Mixer",
        Kind::Math => "Math",
        Kind::Quant => "Quantizer",
        Kind::Compare => "Comparator",
        Kind::Pan => "Pan",
        Kind::Output => "Output",
    }
}

/// The size of a module at zoom 1.
fn node_size(k: Kind) -> Vec2 {
    vec2(NODE_W, TITLE_H + k.rows() as f32 * ROW_H + 6.0)
}

fn node_rect(n: &Node, origin: Pos2, z: f32) -> Rect {
    Rect::from_min_size(origin + vec2(n.pos[0], n.pos[1]) * z, node_size(n.kind) * z)
}

fn row_y(r: &Rect, row: usize, z: f32) -> f32 {
    r.min.y + (TITLE_H + row as f32 * ROW_H + ROW_H * 0.5) * z
}

/// The bar of a row (a parameter's control, or an input's label).
fn row_rect(r: &Rect, row: usize, z: f32) -> Rect {
    let y = row_y(r, row, z);
    Rect::from_min_size(
        pos2(r.min.x + 10.0 * z, y - 8.0 * z),
        vec2(NODE_W - 20.0, 16.0) * z,
    )
}

/// Where output `o` of a module is drawn.
fn out_pos(n: &Node, origin: Pos2, o: usize, z: f32) -> Pos2 {
    let r = node_rect(n, origin, z);
    pos2(r.max.x, row_y(&r, o, z))
}

/// Where input `i` (plain inputs, then parameters) of a module is drawn.
fn in_pos(n: &Node, origin: Pos2, i: usize, z: f32) -> Pos2 {
    let r = node_rect(n, origin, z);
    pos2(r.min.x, row_y(&r, n.kind.spec().outputs.len() + i, z))
}

/// The name of input `i` of a kind (a plain input or a parameter).
fn input_name(k: Kind, i: usize) -> &'static str {
    let s = k.spec();
    s.inputs
        .get(i)
        .map(|x| x.name)
        .unwrap_or_else(|| s.params[i - s.inputs.len()].name)
}

/// Plug a cable in. A cable into a parameter starts at half its range; one that is
/// there already is not doubled, and a module is not wired to its own input from the
/// same output twice.
pub fn connect(p: &mut Patch, from: u32, output: &str, to: u32, input: &str) -> bool {
    let Some(target) = p.node(to) else {
        return false;
    };
    let is_param = target.kind.param(input).is_some();
    let c = Cable::new(from, output, to, input, if is_param { 0.5 } else { 1.0 });
    if !p.is_valid(&c)
        || p.cables
            .iter()
            .any(|x| x.from == from && x.output == output && x.to == to && x.input == input)
    {
        return false;
    }
    p.cables.push(c);
    true
}

/// Take a module out, with its cables.
pub fn remove_node(p: &mut Patch, id: u32) {
    p.nodes.retain(|n| n.id != id);
    p.cables.retain(|c| c.from != id && c.to != id);
}

/// Add a module of a kind at a place; its id.
pub fn add_node(p: &mut Patch, kind: Kind, pos: [f32; 2]) -> u32 {
    let id = p.free_id();
    p.nodes.push(Node::new(id, kind, pos));
    id
}

/// A copy of a module with its parameters (not its cables), a little below and to the
/// right; the copy's id.
pub fn duplicate_node(p: &mut Patch, id: u32) -> Option<u32> {
    let mut n = p.node(id)?.clone();
    n.id = p.free_id();
    n.pos = [n.pos[0] + 30.0, n.pos[1] + 30.0];
    let new = n.id;
    p.nodes.push(n);
    Some(new)
}

/// The zoom and scroll that frame every module in a view of this size.
fn fit_view(p: &Patch, view: Vec2) -> (f32, Vec2) {
    let mut min = pos2(f32::MAX, f32::MAX);
    let mut max = pos2(f32::MIN, f32::MIN);
    for n in &p.nodes {
        let r = node_rect(n, Pos2::ZERO, 1.0);
        min = min.min(r.min);
        max = max.max(r.max);
    }
    if p.nodes.is_empty() || view.x <= 0.0 || view.y <= 0.0 {
        return (1.0, Vec2::ZERO);
    }
    let size = max - min + vec2(40.0, 40.0);
    let z = (view.x / size.x).min(view.y / size.y).clamp(MIN_ZOOM, 1.0);
    let pan = (view - (max - min) * z) * 0.5 - min.to_vec2() * z;
    (z, pan)
}

/// The patch a channel's state holds.
pub fn patch_of(state: &Value) -> Patch {
    las_engine::weave::patch_of(state)
}

fn bezier(a: Pos2, b: Pos2, z: f32) -> [Pos2; 4] {
    let d = ((b.x - a.x).abs() * 0.5).max(40.0 * z);
    [a, a + vec2(d, 0.0), b - vec2(d, 0.0), b]
}

/// The middle of a cable, where its handle is.
fn mid(a: Pos2, b: Pos2, z: f32) -> Pos2 {
    let [p0, p1, p2, p3] = bezier(a, b, z);
    // the point at t = 0.5 of a cubic bezier
    pos2(
        (p0.x + 3.0 * p1.x + 3.0 * p2.x + p3.x) / 8.0,
        (p0.y + 3.0 * p1.y + 3.0 * p2.y + p3.y) / 8.0,
    )
}

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Keys | Kind::Output | Kind::Macro => Color32::from_rgb(120, 140, 160),
        _ => match k.category() {
            0 => Color32::from_rgb(230, 140, 60),
            1 => Color32::from_rgb(90, 170, 230),
            2 => Color32::from_rgb(170, 110, 220),
            3 => Color32::from_rgb(230, 90, 140),
            _ => Color32::from_rgb(100, 190, 120),
        },
    }
}

/// A parameter's value as it is read out, with its unit.
pub fn value_text(k: Kind, ps: &ParamSpec, v: f64) -> String {
    if let Some(c) = k.choices(ps.name) {
        return c.get(v as usize).unwrap_or(&"").to_string();
    }
    if ps.steps > 0 {
        return format!("{v:.0}");
    }
    match ps.name {
        "cutoff" if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        "cutoff" => format!("{v:.0} Hz"),
        "rate" => format!("{v:.2} Hz"),
        "attack" | "decay" | "release" | "rise" | "fall" | "glide" | "time" if v < 1.0 => {
            format!("{:.0} ms", v * 1000.0)
        }
        "attack" | "decay" | "release" | "rise" | "fall" | "glide" | "time" => {
            format!("{v:.2} s")
        }
        "tune" | "bend_range" => format!("{v:+.1} st"),
        "fine" | "detune" => format!("{v:+.0} ct"),
        "ratio" | "drive" => format!("{v:.2}x"),
        "pan" | "curve" | "threshold" | "offset" | "scale" => format!("{v:+.2}"),
        n if n.starts_with("step_") => format!("{:+.1} st", v * 12.0),
        _ => format!("{:.0}%", v * 100.0),
    }
}

impl App {
    /// The window of a Weave channel. A panic while drawing it is caught: the editor
    /// starts again and says so, instead of the app closing.
    pub(crate) fn weave_window(&mut self, ui: &mut Ui, channel: usize) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.weave_window_inner(ui, channel)
        }));
        if caught.is_err() {
            self.weave = WeaveEditor::default();
            self.project.error =
                Some("Weave's editor hit an error and was reset. Please report it.".into());
        }
    }

    /// Tell the player about new values of a Weave patch, so they are heard at once.
    fn send_weave_live(&mut self, channel: usize, patch: &Patch) {
        #[cfg(test)]
        self.weave.sent.push((channel, patch.clone()));
        if let Some(a) = &mut self.audio {
            a.send(crate::audio::Command::WeavePatch {
                channel,
                patch: Box::new(patch.clone()),
            });
        }
    }

    fn weave_window_inner(&mut self, ui: &mut Ui, channel: usize) {
        let Some((name, state, channel_id)) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(channel))
            .map(|c| (c.name.clone(), c.state.clone(), c.id.clone()))
        else {
            return;
        };
        let mut patch = patch_of(&state);
        let before = patch.clone();
        let ed = &mut self.weave;
        let p = pal();
        let skin = crate::skin::current();
        let (down, now, fine, ctrl) = ui.input(|i| {
            (
                i.pointer.any_down(),
                i.time,
                i.modifiers.shift,
                i.modifiers.command,
            )
        });
        if !down {
            ed.gesture = false;
        }
        if ed.seen != Some(channel) {
            ed.seen = Some(channel);
            ed.fit = true;
            ed.forget();
        } else if ed.last.as_ref().is_some_and(|l| *l != patch) {
            // changed from outside (undo, redo, a preset, automation clips): what was
            // being dragged may be gone
            ed.knob = None;
            ed.dragging = None;
        }
        // an edit that is a step of its own (not part of a drag)
        let mut discrete = false;

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&name).size(15.0).strong());
            ui.menu_button("Patches", |ui| {
                for (pname, fp) in factory() {
                    if ui.button(pname).clicked() {
                        patch = fp;
                        ed.forget();
                        ed.fit = true;
                        discrete = true;
                        ui.close();
                    }
                }
            });
            ui.menu_button("Add module", |ui| {
                for (ci, cat) in CATEGORIES.iter().enumerate() {
                    ui.menu_button(*cat, |ui| {
                        for k in Kind::ALL.into_iter().filter(|k| k.category() == ci) {
                            if ui.button(kind_name(k)).clicked() {
                                // in the middle of the view
                                let z = ed.z();
                                let at = (ed.view * 0.5 - ed.pan) / z - node_size(k) * 0.5;
                                ed.node = Some(add_node(&mut patch, k, [at.x, at.y]));
                                discrete = true;
                                ui.close();
                            }
                        }
                    });
                }
            });
            if ui
                .button("Fit")
                .on_hover_text("Zoom and scroll to show every module")
                .clicked()
            {
                ed.fit = true;
            }
            ui.separator();
            ui.add_enabled_ui(!patch.mono, |ui| {
                ui.label("Voices");
                ui.add(
                    egui::DragValue::new(&mut patch.voices)
                        .range(1..=MAX_VOICES)
                        .speed(0.1),
                )
                .on_hover_text("How many keys play at once (drag, type or use the wheel)");
            });
            ui.checkbox(&mut patch.mono, "Mono")
                .on_hover_text("One voice that plays the newest key held");
            ui.add_enabled_ui(patch.mono, |ui| {
                ui.checkbox(&mut patch.legato, "Legato").on_hover_text(
                    "A key played while another is held does not start the envelopes again",
                );
                ui.label("Glide");
                ui.add(
                    egui::DragValue::new(&mut patch.glide)
                        .range(0.0..=MAX_GLIDE)
                        .speed(0.005)
                        .suffix(" s")
                        .fixed_decimals(2),
                );
            });
            ui.label("Volume");
            ui.add(
                egui::DragValue::new(&mut patch.volume)
                    .range(0.0..=2.0)
                    .speed(0.005)
                    .fixed_decimals(2),
            );
            if ui
                .button("Reset")
                .on_hover_text("Start again from the default patch")
                .clicked()
            {
                patch = Patch::default();
                ed.forget();
                ed.fit = true;
                discrete = true;
            }
            let sel = ed
                .selected
                .as_ref()
                .and_then(|k| patch.cables.iter().position(|c| cable_key(c) == *k));
            if let Some(i) = sel {
                ui.separator();
                let c = &mut patch.cables[i];
                ui.label(format!("{} ➡ {}", c.output, c.input));
                ui.add(
                    egui::Slider::new(&mut c.amount, -1.0..=1.0)
                        .text("amount")
                        .fixed_decimals(2),
                );
                if ui.button("Pull out").clicked() {
                    patch.cables.remove(i);
                    ed.selected = None;
                    discrete = true;
                }
            }
        });
        ui.separator();

        let (canvas, bg) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        ed.view = canvas.size();
        if ed.fit {
            (ed.zoom, ed.pan) = fit_view(&patch, canvas.size());
            ed.fit = false;
        }
        // modules partly off the canvas are cut at its edge
        ui.set_clip_rect(canvas.intersect(ui.clip_rect()));
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 4.0, skin.map_or(p.roll_bg, |s| s.screen));
        let dot = skin.map_or(p.grid, |s| s.edge);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over_canvas =
            pointer.is_some_and(|q| canvas.contains(q) && crate::widgets::on_top(ui, q));

        // the parameter under the pointer (the wheel turns it rather than scrolling)
        let hovered_param = pointer.filter(|_| over_canvas).and_then(|q| {
            let (z, origin) = (ed.z(), canvas.min + ed.pan);
            patch.nodes.iter().rev().find_map(|n| {
                let rect = node_rect(n, origin, z);
                if !rect.contains(q) {
                    return None;
                }
                let s = n.kind.spec();
                let first = s.outputs.len() + s.inputs.len();
                (0..s.params.len())
                    .find(|&pi| row_rect(&rect, first + pi, z).contains(q))
                    .map(|pi| (n.id, pi))
            })
        });

        // ctrl and the wheel zoom about the pointer; the wheel turns the parameter under
        // the pointer, or scrolls
        if let Some(q) = pointer.filter(|_| over_canvas) {
            let (zd, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
            let z = ed.z();
            if zd != 1.0 {
                let nz = (z * zd).clamp(MIN_ZOOM, MAX_ZOOM);
                let world = (q - canvas.min - ed.pan) / z;
                ed.pan = q - canvas.min - world * nz;
                ed.zoom = nz;
            } else if scroll != Vec2::ZERO {
                if let Some((id, pi)) = hovered_param.filter(|_| ed.dragging.is_none()) {
                    if let Some(n) = patch.nodes.iter_mut().find(|n| n.id == id) {
                        let ps = &n.kind.spec().params[pi];
                        let v = n.param(ps.name);
                        let turned = scroll.y + scroll.x;
                        let nv = if ps.steps > 0 {
                            ed.wheel += turned;
                            let notches = (ed.wheel / WHEEL_NOTCH).trunc();
                            ed.wheel -= notches * WHEEL_NOTCH;
                            (v + f64::from(notches)).clamp(ps.min, ps.max)
                        } else {
                            wheel_turn(ps, v, turned, fine)
                        };
                        if nv != v {
                            n.params.insert(ps.name.to_string(), nv);
                        }
                    }
                } else {
                    ed.pan += scroll;
                }
            }
            // the window around the canvas does not scroll as well
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
        let z = ed.z();
        // a dotted grid that moves with the canvas
        let step = 24.0 * z;
        let off = vec2(ed.pan.x.rem_euclid(step), ed.pan.y.rem_euclid(step));
        let mut y = canvas.min.y + off.y;
        while y < canvas.max.y {
            let mut x = canvas.min.x + off.x;
            while x < canvas.max.x {
                painter.circle_filled(pos2(x, y), 1.0, dot);
                x += step;
            }
            y += step;
        }
        if bg.dragged() && ed.dragging.is_none() && ed.knob.is_none() {
            ed.pan += bg.drag_delta();
        }
        if bg.clicked() {
            ed.selected = None;
            ed.node = None;
        }
        let origin = canvas.min + ed.pan;
        ed.origin = origin;
        let font = |size: f32| egui::FontId::proportional(size * z);
        let text_col = skin.map_or(p.text, |s| s.text);
        // text this small is not drawn
        let words = z >= 0.45;

        // cables under the modules
        let mut picked = None;
        let mut pull = None;
        let mut amount: Option<(usize, f64)> = None;
        for (ci, c) in patch.cables.iter().enumerate() {
            let (Some(a), Some(b)) = (patch.node(c.from), patch.node(c.to)) else {
                continue;
            };
            let (Some(o), Some(i)) = (a.kind.output_index(&c.output), b.kind.input_index(&c.input))
            else {
                continue;
            };
            let (pa, pb) = (out_pos(a, origin, o, z), in_pos(b, origin, i, z));
            let key = cable_key(c);
            let selected = ed.selected.as_ref() == Some(&key);
            let m = mid(pa, pb, z);
            let handle = Rect::from_center_size(m, vec2(16.0, 16.0));
            let r = ui.interact(handle, ui.scope_id().with(("cable", &key)), Sense::click());
            let col = if selected || r.hovered() {
                p.accent_strong
            } else {
                kind_color(a.kind).gamma_multiply(0.4 + 0.6 * c.amount.abs().min(1.0) as f32)
            };
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                bezier(pa, pb, z),
                false,
                Color32::TRANSPARENT,
                Stroke::new(if selected || r.hovered() { 3.0 } else { 2.0 }, col),
            ));
            painter.circle_filled(m, if r.hovered() { 6.0 } else { 4.0 }, col);
            if r.double_clicked() {
                pull = Some(ci);
            } else if r.clicked() {
                picked = Some(key.clone());
            }
            let mut amt = c.amount;
            r.on_hover_text(format!(
                "{} {} ➡ {} {} ({:.2})\nClick to select (Delete pulls it out), double click to pull out, right click for more",
                kind_name(a.kind),
                c.output,
                kind_name(b.kind),
                c.input,
                c.amount
            ))
            .context_menu(|ui| {
                if ui
                    .add(egui::Slider::new(&mut amt, -1.0..=1.0).text("amount"))
                    .changed()
                {
                    amount = Some((ci, amt));
                }
                if ui.button("Pull out cable").clicked() {
                    pull = Some(ci);
                    ui.close();
                }
            });
        }
        if let Some(k) = picked {
            ed.selected = Some(k);
            ed.node = None;
        }
        if let Some((ci, a)) = amount {
            if let Some(c) = patch.cables.get_mut(ci) {
                c.amount = a.clamp(-1.0, 1.0);
            }
        }
        if let Some(ci) = pull.filter(|&ci| ci < patch.cables.len()) {
            patch.cables.remove(ci);
            ed.selected = None;
            discrete = true;
        }
        // Delete pulls out the selected cable, or takes out the selected module
        if over_canvas && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(k) = ed.selected.take() {
                patch.cables.retain(|c| cable_key(c) != k);
                discrete = true;
            } else if let Some(id) = ed.node.take() {
                remove_node(&mut patch, id);
                discrete = true;
            }
        }

        // what can take the cable being drawn, and what it would go into
        let drawing = ed.dragging.and_then(|(from, o)| {
            let n = patch.node(from)?;
            Some((from, n.kind.spec().outputs.get(o).copied()?))
        });
        if ed.dragging.is_some() && drawing.is_none() {
            ed.dragging = None;
        }
        let idle: Vec<Vec<Option<&'static str>>> = patch
            .nodes
            .iter()
            .map(|n| {
                n.kind
                    .spec()
                    .params
                    .iter()
                    .map(|ps| patch.idle_reason(n, ps.name))
                    .collect()
            })
            .collect();
        let free_to: Vec<Vec<bool>> = patch
            .nodes
            .iter()
            .map(|n| {
                let s = n.kind.spec();
                (0..s.inputs.len() + s.params.len())
                    .map(|k| {
                        drawing.is_some_and(|(from, output)| {
                            let input = input_name(n.kind, k);
                            !patch.cables.iter().any(|c| {
                                c.from == from
                                    && c.output == output
                                    && c.to == n.id
                                    && c.input == input
                            })
                        })
                    })
                    .collect()
            })
            .collect();
        let into: Vec<Vec<usize>> = patch
            .nodes
            .iter()
            .map(|n| {
                let s = n.kind.spec();
                (0..s.inputs.len() + s.params.len())
                    .map(|k| {
                        let input = input_name(n.kind, k);
                        patch
                            .cables
                            .iter()
                            .filter(|c| c.to == n.id && c.input == input)
                            .count()
                    })
                    .collect()
            })
            .collect();

        // the modules
        let mut remove = None;
        let mut duplicate = None;
        let mut unplug: Option<(u32, usize)> = None;
        let mut automate: Option<(u32, String, f64)> = None;
        let mut drop_on: Option<(u32, usize)> = None;
        let mut bubble: Option<(Pos2, String)> = None;
        for (ni, n) in patch.nodes.iter_mut().enumerate() {
            let rect = node_rect(n, origin, z);
            if !canvas.intersects(rect) {
                continue;
            }
            let spec = n.kind.spec();
            let kc = kind_color(n.kind);
            let picked_node = ed.node == Some(n.id);
            painter.rect_filled(rect, 6.0 * z, skin.map_or(p.panel2, |s| s.section));
            painter.rect_stroke(
                rect,
                6.0 * z,
                Stroke::new(
                    if picked_node { 2.0 } else { 1.0 },
                    if picked_node { p.accent_strong } else { kc },
                ),
                egui::StrokeKind::Inside,
            );
            let title = Rect::from_min_size(rect.min, vec2(NODE_W, TITLE_H) * z);
            painter.rect_filled(title, 6.0 * z, kc.gamma_multiply(0.35));
            if words {
                painter.text(
                    title.left_center() + vec2(8.0 * z, 0.0),
                    egui::Align2::LEFT_CENTER,
                    kind_name(n.kind),
                    font(13.0),
                    text_col,
                );
            }
            let t = ui.interact(
                title,
                ui.scope_id().with(("node", n.id)),
                Sense::click_and_drag(),
            );
            if t.drag_started() || t.clicked() {
                ed.node = Some(n.id);
                ed.selected = None;
            }
            if t.dragged() {
                let d = t.drag_delta() / z;
                n.pos[0] += d.x;
                n.pos[1] += d.y;
            }
            if t.hovered() || t.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            let id = n.id;
            t.on_hover_text(format!(
                "{} ({})\nDrag to move, right click for more",
                kind_name(n.kind),
                CATEGORIES[n.kind.category()]
            ))
            .context_menu(|ui| {
                if ui.button("Duplicate module").clicked() {
                    duplicate = Some(id);
                    ui.close();
                }
                if ui.button("Remove module").clicked() {
                    remove = Some(id);
                    ui.close();
                }
            });

            let label = skin.map_or(p.text2, |s| s.dim);
            // outputs: label and a dot on the right; drag from the dot
            for (o, oname) in spec.outputs.iter().enumerate() {
                let y = row_y(&rect, o, z);
                if words {
                    painter.text(
                        pos2(rect.max.x - 12.0 * z, y),
                        egui::Align2::RIGHT_CENTER,
                        *oname,
                        font(11.0),
                        label,
                    );
                }
                let at = pos2(rect.max.x, y);
                let r = ui.interact(
                    Rect::from_center_size(at, vec2(18.0, 18.0)),
                    ui.scope_id().with(("out", id, o)),
                    Sense::drag(),
                );
                let lit = r.hovered() || ed.dragging == Some((id, o));
                painter.circle_filled(at, PORT_R * z.max(0.6) * if lit { 1.4 } else { 1.0 }, kc);
                if r.drag_started() {
                    ed.dragging = Some((id, o));
                    ed.knob = None;
                }
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                r.on_hover_text(format!("{oname}: drag to an input or a parameter"));
            }
            // inputs and parameters: a dot on the left, a bar for a parameter
            let first = spec.outputs.len();
            for k in 0..spec.inputs.len() + spec.params.len() {
                let y = row_y(&rect, first + k, z);
                let at = pos2(rect.min.x, y);
                let valid = free_to[ni][k];
                // the whole row takes the cable being drawn, not just its dot
                let band = Rect::from_min_max(
                    pos2(rect.min.x - 12.0 * z.max(0.6), y - ROW_H * 0.5 * z),
                    pos2(rect.max.x, y + ROW_H * 0.5 * z),
                );
                let target = valid && pointer.is_some_and(|q| band.contains(q));
                if target {
                    drop_on = Some((id, k));
                }
                let plugged = into[ni][k] > 0;
                let col = if target {
                    p.accent_strong
                } else if plugged {
                    kc
                } else {
                    p.border2
                };
                painter.circle_filled(
                    at,
                    PORT_R * z.max(0.6) * if target { 1.4 } else { 1.0 },
                    col,
                );
                if valid {
                    // every place the cable can go is ringed
                    painter.circle_stroke(
                        at,
                        (PORT_R + 3.0) * z.max(0.6),
                        Stroke::new(
                            1.5,
                            p.accent_strong
                                .gamma_multiply(if target { 1.0 } else { 0.5 }),
                        ),
                    );
                }
                // drag from a plugged input to take its cable out and plug it elsewhere
                if plugged {
                    let pr = ui.interact(
                        Rect::from_center_size(at, vec2(18.0, 18.0)),
                        ui.scope_id().with(("in", id, k)),
                        Sense::drag(),
                    );
                    if pr.drag_started() {
                        unplug = Some((id, k));
                    }
                    pr.on_hover_text("Drag to take this cable out and plug it in elsewhere");
                }
                let row = row_rect(&rect, first + k, z);
                if k < spec.inputs.len() {
                    if words {
                        painter.text(
                            row.left_center(),
                            egui::Align2::LEFT_CENTER,
                            spec.inputs[k].name,
                            font(11.0),
                            label,
                        );
                    }
                    continue;
                }
                let pi = k - spec.inputs.len();
                let ps = &spec.params[pi];
                let mut v = n.param(ps.name);
                let r = ui.interact(
                    row,
                    ui.scope_id().with(("param", id, pi)),
                    Sense::click_and_drag(),
                );
                let turning = ed
                    .knob
                    .as_ref()
                    .is_some_and(|kn| kn.node == id && kn.param == pi);
                let why = idle[ni][pi];
                // the bar: how far along its range the value is
                let at_n = ps.normalize(v) as f32;
                painter.rect_filled(row, 3.0 * z, skin.map_or(p.well_deep, |s| s.track));
                let fill =
                    Rect::from_min_max(row.min, pos2(row.min.x + row.width() * at_n, row.max.y));
                let lit = if turning {
                    0.95
                } else if r.hovered() {
                    0.75
                } else if why.is_some() {
                    0.25
                } else {
                    0.5
                };
                painter.rect_filled(fill, 3.0 * z, kc.gamma_multiply(lit));
                let shown = value_text(n.kind, ps, v);
                if words {
                    let tc = if why.is_some() { label } else { text_col };
                    let val = painter.layout_no_wrap(shown.clone(), font(10.5), tc);
                    let nm = painter.layout_no_wrap(ps.name.replace('_', " "), font(10.5), tc);
                    let pad = 4.0 * z;
                    painter.galley(
                        pos2(
                            row.max.x - pad - val.size().x,
                            row.center().y - val.size().y * 0.5,
                        ),
                        val.clone(),
                        tc,
                    );
                    // the name only where it does not run into the value
                    if nm.size().x + val.size().x + 3.0 * pad <= row.width() {
                        painter.galley(
                            pos2(row.min.x + pad, row.center().y - nm.size().y * 0.5),
                            nm,
                            tc,
                        );
                    }
                }
                let mut changed = false;
                if let Some(choices) = n.kind.choices(ps.name) {
                    egui::Popup::menu(&r).show(|ui| {
                        for (ci, nm) in choices.iter().enumerate() {
                            if ui.selectable_label(v as usize == ci, *nm).clicked() {
                                v = ci as f64;
                                changed = true;
                            }
                        }
                    });
                } else {
                    if r.drag_started() {
                        ed.knob = Some(Knob {
                            node: id,
                            param: pi,
                            at: ps.normalize(v),
                        });
                    }
                    if r.dragged() {
                        // read outside any lock of the context
                        let d = r.drag_delta();
                        if let Some(kn) = ed
                            .knob
                            .as_mut()
                            .filter(|kn| kn.node == id && kn.param == pi)
                        {
                            kn.at = knob_turn(kn.at, d.x, d.y, fine);
                            let nv = ps.denormalize(kn.at);
                            if nv != v {
                                v = nv;
                                changed = true;
                            }
                        }
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    }
                    if r.drag_stopped() {
                        ed.knob = None;
                    }
                    if r.double_clicked() || (r.clicked() && ctrl) {
                        v = ps.default;
                        changed = true;
                    }
                }
                if turning || r.dragged() {
                    bubble = Some((
                        row.center_top(),
                        format!(
                            "{} {}",
                            ps.name.replace('_', " "),
                            value_text(n.kind, ps, v)
                        ),
                    ));
                }
                let pid = param_id(id, pi);
                let alabel = format!("{} {}", kind_name(n.kind), ps.name);
                let at = ps.normalize(v);
                let range = format!(
                    "{} to {}",
                    value_text(n.kind, ps, ps.min),
                    value_text(n.kind, ps, ps.max)
                );
                let how = if n.kind.choices(ps.name).is_some() {
                    "Click to choose, wheel to step, right click to automate"
                } else {
                    "Drag up or down to set (shift for fine), wheel to nudge, double click to reset, right click to automate"
                };
                let note = why
                    .map(|w| format!("\nNot heard now: {w}"))
                    .unwrap_or_default();
                r.on_hover_text(format!(
                    "{}: {}\n{range}, default {}{note}\n{how}",
                    ps.name,
                    shown,
                    value_text(n.kind, ps, ps.default)
                ))
                .context_menu(|ui| {
                    if ui.button("Create automation clip").clicked() {
                        automate = Some((pid, alabel.clone(), at));
                        ui.close();
                    }
                    if ui.button("Reset to default").clicked() {
                        v = ps.default;
                        changed = true;
                        ui.close();
                    }
                });
                if changed {
                    n.params.insert(ps.name.to_string(), v);
                }
            }
        }
        // the value of the control being turned, over it
        if let Some((at, text)) = bubble {
            let g = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), text_col);
            let r = Rect::from_center_size(
                at - vec2(0.0, g.size().y * 0.5 + 6.0),
                g.size() + vec2(10.0, 4.0),
            );
            painter.rect_filled(r, 4.0, skin.map_or(p.panel2, |s| s.section));
            painter.rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, p.accent_strong),
                egui::StrokeKind::Inside,
            );
            painter.galley(r.min + vec2(5.0, 2.0), g, text_col);
        }
        if let Some(id) = remove {
            remove_node(&mut patch, id);
            ed.selected = None;
            ed.node = None;
            discrete = true;
        }
        if let Some(id) = duplicate {
            ed.node = duplicate_node(&mut patch, id);
            discrete = true;
        }
        // a plugged input dragged: its newest cable comes out and is drawn again
        if let Some((to, k)) = unplug {
            let input = patch.node(to).map(|n| input_name(n.kind, k));
            if let Some(ci) = input.and_then(|inp| {
                patch
                    .cables
                    .iter()
                    .rposition(|c| c.to == to && c.input == inp)
            }) {
                let c = patch.cables.remove(ci);
                let o = patch
                    .node(c.from)
                    .and_then(|n| n.kind.output_index(&c.output));
                ed.dragging = o.map(|o| (c.from, o));
                ed.selected = None;
                discrete = true;
            }
        }

        // the cable being drawn
        if let Some((from, o)) = ed.dragging {
            if let (Some(n), Some(q)) = (patch.node(from), pointer) {
                let end = drop_on
                    .and_then(|(to, k)| patch.node(to).map(|t| in_pos(t, origin, k, z)))
                    .unwrap_or(q);
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    bezier(out_pos(n, origin, o, z), end, z),
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(2.5, p.accent_strong),
                ));
            }
            let (released, escape) =
                ui.input(|i| (i.pointer.any_released(), i.key_pressed(egui::Key::Escape)));
            if released {
                if let (Some((to, k)), Some(n)) = (drop_on, patch.node(from)) {
                    let output = n.kind.spec().outputs[o];
                    if let Some(target) = patch.node(to) {
                        let input = input_name(target.kind, k);
                        if connect(&mut patch, from, output, to, input) {
                            discrete = true;
                        }
                    }
                }
                ed.dragging = None;
            } else if escape {
                ed.dragging = None;
            }
            ui.ctx().request_repaint();
        }

        if let Some((pid, label, start)) = automate {
            self.project.create_automation(&crate::auto::AutoRequest {
                target: crate::auto::plugin_target(&channel_id, &format!("p.{pid}")),
                name: format!("{name} {label}"),
                start,
            });
        }
        if patch != before {
            // one undo step for each drag (or each wheel turn, a second apart)
            let begin = discrete
                || if down {
                    !ed.gesture
                } else {
                    now - ed.last_edit > 1.0
                };
            ed.gesture = down;
            ed.last_edit = now;
            // values only: the player takes them as they change, without being built
            // again; new modules or cables need a new player (built once let go)
            let live = before.same_shape(&patch) && self.project.takes_live_edits();
            if live {
                self.send_weave_live(channel, &patch);
            }
            let value = patch.to_json();
            let f = move |p: &mut las_project::Project| {
                if let Some(c) = p.channels.get_mut(channel) {
                    if !c.state.is_object() {
                        c.state = json!({});
                    }
                    c.state["patch"] = value;
                }
            };
            if live {
                self.project.modify_live(begin, f);
            } else {
                self.project.modify(begin, f);
            }
        }
        self.weave.last = Some(patch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cables_go_from_outputs_to_inputs_once() {
        let mut p = Patch::default();
        let lfo = add_node(&mut p, Kind::Lfo, [0.0, 0.0]);
        // into a parameter: half the range
        assert!(connect(&mut p, lfo, "out", 4, "cutoff"));
        assert_eq!(p.cables.last().unwrap().amount, 0.5);
        // not twice
        assert!(!connect(&mut p, lfo, "out", 4, "cutoff"));
        // into a plain input: the whole signal
        assert!(connect(&mut p, lfo, "out", 5, "in"));
        assert_eq!(p.cables.last().unwrap().amount, 1.0);
        // ports that do not exist
        assert!(!connect(&mut p, lfo, "nope", 4, "in"));
        assert!(!connect(&mut p, lfo, "out", 4, "nope"));
        assert!(!connect(&mut p, 99, "out", 4, "in"));
    }

    #[test]
    fn removing_a_module_takes_its_cables() {
        let mut p = Patch::default();
        let n = p.cables.len();
        remove_node(&mut p, 3);
        assert!(p.node(3).is_none());
        assert!(p.cables.iter().all(|c| c.from != 3 && c.to != 3));
        assert_eq!(p.cables.len(), n - 3);
    }

    #[test]
    fn every_port_has_a_place() {
        for k in Kind::ALL {
            let n = Node::new(1, k, [10.0, 20.0]);
            let s = k.spec();
            for z in [0.5, 1.0, 1.5] {
                let r = node_rect(&n, Pos2::ZERO, z);
                for o in 0..s.outputs.len() {
                    assert!(r.contains(out_pos(&n, Pos2::ZERO, o, z) - vec2(1.0, 0.0)));
                }
                for i in 0..s.inputs.len() + s.params.len() {
                    assert!(r.contains(in_pos(&n, Pos2::ZERO, i, z) + vec2(1.0, 0.0)));
                    assert!(k.input_index(input_name(k, i)) == Some(i));
                }
            }
            // every parameter reads out
            for ps in s.params {
                assert!(
                    !value_text(k, ps, ps.default).is_empty(),
                    "{k:?} {}",
                    ps.name
                );
            }
        }
    }

    #[test]
    fn a_duplicate_keeps_the_parameters_but_not_the_cables() {
        let mut p = Patch::default();
        let cables = p.cables.len();
        let id = duplicate_node(&mut p, 4).unwrap();
        let (a, b) = (p.node(4).unwrap(), p.node(id).unwrap());
        assert_eq!((a.kind, &a.params), (b.kind, &b.params));
        assert_ne!(a.pos, b.pos);
        assert_eq!(p.cables.len(), cables);
        assert!(duplicate_node(&mut p, 99).is_none());
    }

    #[test]
    fn fit_shows_every_module() {
        for (_, p) in factory() {
            let view = vec2(860.0, 520.0);
            let (z, pan) = fit_view(&p, view);
            let shown = Rect::from_min_size(Pos2::ZERO, view);
            // at the smallest zoom a very big patch may not fit; these all do
            assert!(z >= MIN_ZOOM);
            for n in &p.nodes {
                let r = node_rect(n, Pos2::ZERO + pan, z);
                assert!(shown.expand(1.0).contains_rect(r), "{:?}", n.kind);
            }
        }
    }

    #[test]
    fn values_read_out_in_their_units() {
        let cutoff = Kind::Filter.param("cutoff").unwrap();
        assert_eq!(value_text(Kind::Filter, cutoff, 2_000.0), "2.00 kHz");
        let attack = Kind::Env.param("attack").unwrap();
        assert_eq!(value_text(Kind::Env, attack, 0.005), "5 ms");
        let wave = Kind::Osc.param("wave").unwrap();
        assert_eq!(value_text(Kind::Osc, wave, 4.0), "morph");
    }
}
