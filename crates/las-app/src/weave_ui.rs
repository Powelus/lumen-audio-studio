// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave's window: the patch as modules on a canvas, joined by cables. Drag a module by
//! its title; drag from an output (the dots on the right) to an input or a parameter
//! (the dots on the left) to plug a cable in. Click a cable's handle to set its amount
//! or pull it out. Drag a parameter's bar to set it (shift for fine steps, a double
//! click for its default). Ctrl and the wheel zoom; Fit frames every module. Every
//! edit goes into the channel's state as one undo step.

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
    selected: Option<usize>,
    /// Frame every module on the next frame.
    fit: bool,
    /// The channel last shown, to frame a patch the first time it opens.
    seen: Option<usize>,
    /// The size of the canvas last frame.
    view: Vec2,
}

impl WeaveEditor {
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
    /// The window of a Weave channel.
    pub(crate) fn weave_window(&mut self, ui: &mut Ui, channel: usize) {
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
        if ed.seen != Some(channel) {
            ed.seen = Some(channel);
            ed.fit = true;
            ed.selected = None;
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&name).size(15.0).strong());
            ui.menu_button("Patches", |ui| {
                for (pname, fp) in factory() {
                    if ui.button(pname).clicked() {
                        patch = fp;
                        ed.selected = None;
                        ed.fit = true;
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
                                add_node(&mut patch, k, [at.x, at.y]);
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
                ui.add(egui::DragValue::new(&mut patch.voices).range(1..=MAX_VOICES));
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
                ed.selected = None;
                ed.fit = true;
            }
            if let Some(i) = ed.selected.filter(|&i| i < patch.cables.len()) {
                ui.separator();
                let c = &mut patch.cables[i];
                ui.label(format!("{} → {}", c.output, c.input));
                ui.add(
                    egui::Slider::new(&mut c.amount, -1.0..=1.0)
                        .text("amount")
                        .fixed_decimals(2),
                );
                if ui.button("Pull out").clicked() {
                    patch.cables.remove(i);
                    ed.selected = None;
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

        // ctrl and the wheel zoom about the pointer; the wheel alone scrolls
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if let Some(q) = pointer.filter(|q| canvas.contains(*q)) {
            let (zd, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
            let z = ed.z();
            if zd != 1.0 {
                let nz = (z * zd).clamp(MIN_ZOOM, MAX_ZOOM);
                let world = (q - canvas.min - ed.pan) / z;
                ed.pan = q - canvas.min - world * nz;
                ed.zoom = nz;
            } else if scroll != Vec2::ZERO {
                ed.pan += scroll;
            }
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
        if bg.dragged() && ed.dragging.is_none() {
            ed.pan += bg.drag_delta();
        }
        if bg.clicked() {
            ed.selected = None;
        }
        let origin = canvas.min + ed.pan;
        let font = |size: f32| egui::FontId::proportional(size * z);

        // cables under the modules
        let (mut picked, mut pull) = (None, None);
        for (ci, c) in patch.cables.iter().enumerate() {
            let (Some(a), Some(b)) = (patch.node(c.from), patch.node(c.to)) else {
                continue;
            };
            let (Some(o), Some(i)) = (a.kind.output_index(&c.output), b.kind.input_index(&c.input))
            else {
                continue;
            };
            let (pa, pb) = (out_pos(a, origin, o, z), in_pos(b, origin, i, z));
            let selected = ed.selected == Some(ci);
            let col = if selected {
                p.accent_strong
            } else {
                kind_color(a.kind).gamma_multiply(0.4 + 0.6 * c.amount.abs().min(1.0) as f32)
            };
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                bezier(pa, pb, z),
                false,
                Color32::TRANSPARENT,
                Stroke::new(if selected { 3.0 } else { 2.0 }, col),
            ));
            let m = mid(pa, pb, z);
            let handle = Rect::from_center_size(m, vec2(12.0, 12.0));
            let r = ui.interact(handle, ui.scope_id().with(("cable", ci)), Sense::click());
            painter.circle_filled(m, if r.hovered() { 6.0 } else { 4.0 }, col);
            if r.clicked() {
                picked = Some(ci);
            }
            if r.secondary_clicked() {
                pull = Some(ci);
            }
            r.on_hover_text(format!(
                "{} {} → {} {} ({:.2})\nClick to set the amount, right click to pull out",
                kind_name(a.kind),
                c.output,
                kind_name(b.kind),
                c.input,
                c.amount
            ));
        }
        if let Some(ci) = picked {
            ed.selected = Some(ci);
        }
        // a right click on a handle pulls the cable out
        if let Some(ci) = pull {
            patch.cables.remove(ci);
            ed.selected = None;
        }

        // the modules
        let mut remove = None;
        let mut duplicate = None;
        let mut automate: Option<(u32, String, f64)> = None;
        let mut drop_on: Option<(u32, usize)> = None;
        let fine = ui.input(|i| i.modifiers.shift);
        for n in patch.nodes.iter_mut() {
            let rect = node_rect(n, origin, z);
            if !canvas.intersects(rect) {
                continue;
            }
            let spec = n.kind.spec();
            let kc = kind_color(n.kind);
            painter.rect_filled(rect, 6.0 * z, skin.map_or(p.panel2, |s| s.section));
            painter.rect_stroke(
                rect,
                6.0 * z,
                Stroke::new(1.0, kc),
                egui::StrokeKind::Inside,
            );
            let title = Rect::from_min_size(rect.min, vec2(NODE_W, TITLE_H) * z);
            painter.rect_filled(title, 6.0 * z, kc.gamma_multiply(0.35));
            painter.text(
                title.left_center() + vec2(8.0 * z, 0.0),
                egui::Align2::LEFT_CENTER,
                kind_name(n.kind),
                font(13.0),
                skin.map_or(p.text, |s| s.text),
            );
            let t = ui.interact(
                title,
                ui.scope_id().with(("node", n.id)),
                Sense::click_and_drag(),
            );
            if t.dragged() {
                n.pos[0] += t.drag_delta().x / z;
                n.pos[1] += t.drag_delta().y / z;
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
                painter.text(
                    pos2(rect.max.x - 12.0 * z, y),
                    egui::Align2::RIGHT_CENTER,
                    *oname,
                    font(11.0),
                    label,
                );
                let at = pos2(rect.max.x, y);
                let r = ui.interact(
                    Rect::from_center_size(at, vec2(14.0, 14.0)),
                    ui.scope_id().with(("out", id, o)),
                    Sense::drag(),
                );
                painter.circle_filled(at, PORT_R * z.max(0.6), kc);
                if r.drag_started() {
                    ed.dragging = Some((id, o));
                }
                r.on_hover_text(format!("{oname}: drag to an input or a parameter"));
            }
            // inputs and parameters: a dot on the left, a bar for a parameter
            let first = spec.outputs.len();
            for k in 0..spec.inputs.len() + spec.params.len() {
                let y = row_y(&rect, first + k, z);
                let at = pos2(rect.min.x, y);
                let near = pointer.is_some_and(|q| q.distance(at) < 10.0);
                let col = if near && ed.dragging.is_some() {
                    p.accent_strong
                } else {
                    p.border2
                };
                painter.circle_filled(at, PORT_R * z.max(0.6), col);
                if near && ed.dragging.is_some() {
                    drop_on = Some((id, k));
                }
                let row = Rect::from_min_size(
                    pos2(rect.min.x + 10.0 * z, y - 8.0 * z),
                    vec2(NODE_W - 20.0, 16.0) * z,
                );
                if k < spec.inputs.len() {
                    painter.text(
                        row.left_center(),
                        egui::Align2::LEFT_CENTER,
                        spec.inputs[k].name,
                        font(11.0),
                        label,
                    );
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
                // the bar: how far along its range the value is
                let at_n = ps.normalize(v) as f32;
                painter.rect_filled(row, 3.0 * z, skin.map_or(p.well_deep, |s| s.track));
                let fill =
                    Rect::from_min_max(row.min, pos2(row.min.x + row.width() * at_n, row.max.y));
                let lit = if r.hovered() || r.dragged() {
                    0.75
                } else {
                    0.5
                };
                painter.rect_filled(fill, 3.0 * z, kc.gamma_multiply(lit));
                painter.text(
                    row.left_center() + vec2(4.0 * z, 0.0),
                    egui::Align2::LEFT_CENTER,
                    ps.name,
                    font(10.5),
                    skin.map_or(p.text, |s| s.text),
                );
                let shown = value_text(n.kind, ps, v);
                painter.text(
                    row.right_center() - vec2(4.0 * z, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    &shown,
                    font(10.5),
                    skin.map_or(p.text, |s| s.text),
                );
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
                } else if r.dragged() {
                    let speed = if fine { 0.1 } else { 1.0 };
                    let nn = (ps.normalize(v) + f64::from(r.drag_delta().x / row.width() * speed))
                        .clamp(0.0, 1.0);
                    // a whole number moves by its own steps, kept between frames
                    let id_acc = ui.scope_id().with(("acc", id, pi));
                    let acc: f64 = if ps.steps > 0 {
                        ui.data_mut(|d| {
                            let a = d.get_temp_mut_or(id_acc, ps.normalize(v));
                            *a = (*a + f64::from(r.drag_delta().x / row.width() * speed))
                                .clamp(0.0, 1.0);
                            *a
                        })
                    } else {
                        nn
                    };
                    v = ps.denormalize(acc);
                    changed = true;
                }
                if r.drag_stopped() {
                    ui.data_mut(|d| d.remove::<f64>(ui.scope_id().with(("acc", id, pi))));
                }
                if r.double_clicked() && n.kind.choices(ps.name).is_none() {
                    v = ps.default;
                    changed = true;
                }
                let pid = param_id(id, pi);
                let alabel = format!("{} {}", kind_name(n.kind), ps.name);
                let at = ps.normalize(v);
                let range = format!(
                    "{} to {}",
                    value_text(n.kind, ps, ps.min),
                    value_text(n.kind, ps, ps.max)
                );
                r.on_hover_text(format!(
                    "{}: {}\n{range}, default {}\nDrag to set (shift for fine), double click to reset, right click to automate",
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
        if let Some(id) = remove {
            remove_node(&mut patch, id);
            ed.selected = None;
        }
        if let Some(id) = duplicate {
            duplicate_node(&mut patch, id);
        }

        // the cable being drawn
        if let Some((from, o)) = ed.dragging {
            if let (Some(n), Some(q)) = (patch.node(from), pointer) {
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    bezier(out_pos(n, origin, o, z), q, z),
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(2.0, p.accent_strong),
                ));
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let (Some((to, k)), Some(n)) = (drop_on, patch.node(from)) {
                    let output = n.kind.spec().outputs[o];
                    if let Some(target) = patch.node(to) {
                        let input = input_name(target.kind, k);
                        connect(&mut patch, from, output, to, input);
                    }
                }
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
            let value = patch.to_json();
            self.project.modify_coalesced("weave", move |p| {
                if let Some(c) = p.channels.get_mut(channel) {
                    if !c.state.is_object() {
                        c.state = json!({});
                    }
                    c.state["patch"] = value;
                }
            });
        }
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
