// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave's window: the patch as modules on a canvas, joined by cables. Drag a module by
//! its title; drag from an output (the dots on the right) to an input or a parameter
//! (the dots on the left) to plug a cable in. Click a cable's handle to set its amount
//! or pull it out. Every edit goes into the channel's state as one undo step.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use las_dsp::modular::{
    factory, param_id, Cable, Kind, Node, Patch, FILTER_MODES, MAX_VOICES, WAVES,
};
use serde_json::{json, Value};

const NODE_W: f32 = 180.0;
const TITLE_H: f32 = 22.0;
const ROW_H: f32 = 20.0;
const PORT_R: f32 = 5.0;

/// What the editor remembers between frames.
#[derive(Default)]
pub struct WeaveEditor {
    /// How far the canvas is scrolled.
    pan: Vec2,
    /// A cable being drawn from an output: the node and the output's place.
    dragging: Option<(u32, usize)>,
    /// The cable whose amount is being set.
    selected: Option<usize>,
}

pub fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Keys => "Keys",
        Kind::Osc => "Oscillator",
        Kind::Noise => "Noise",
        Kind::Lfo => "LFO",
        Kind::Env => "Envelope",
        Kind::Filter => "Filter",
        Kind::Vca => "VCA",
        Kind::Mix => "Mixer",
        Kind::Math => "Math",
        Kind::Output => "Output",
    }
}

/// The rows of a module: its outputs, then its inputs, then its parameters.
fn rows(k: Kind) -> usize {
    let s = k.spec();
    s.outputs.len() + s.inputs.len() + s.params.len()
}

fn node_rect(n: &Node, origin: Pos2) -> Rect {
    Rect::from_min_size(
        origin + vec2(n.pos[0], n.pos[1]),
        vec2(NODE_W, TITLE_H + rows(n.kind) as f32 * ROW_H + 6.0),
    )
}

fn row_y(r: &Rect, row: usize) -> f32 {
    r.min.y + TITLE_H + row as f32 * ROW_H + ROW_H * 0.5
}

/// Where output `o` of a module is drawn.
fn out_pos(n: &Node, origin: Pos2, o: usize) -> Pos2 {
    let r = node_rect(n, origin);
    pos2(r.max.x, row_y(&r, o))
}

/// Where input `i` (plain inputs, then parameters) of a module is drawn.
fn in_pos(n: &Node, origin: Pos2, i: usize) -> Pos2 {
    let r = node_rect(n, origin);
    pos2(r.min.x, row_y(&r, n.kind.spec().outputs.len() + i))
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

/// The patch a channel's state holds.
pub fn patch_of(state: &Value) -> Patch {
    las_engine::weave::patch_of(state)
}

fn bezier(a: Pos2, b: Pos2) -> [Pos2; 4] {
    let d = ((b.x - a.x).abs() * 0.5).max(40.0);
    [a, a + vec2(d, 0.0), b - vec2(d, 0.0), b]
}

/// The middle of a cable, where its handle is.
fn mid(a: Pos2, b: Pos2) -> Pos2 {
    let [p0, p1, p2, p3] = bezier(a, b);
    // the point at t = 0.5 of a cubic bezier
    pos2(
        (p0.x + 3.0 * p1.x + 3.0 * p2.x + p3.x) / 8.0,
        (p0.y + 3.0 * p1.y + 3.0 * p2.y + p3.y) / 8.0,
    )
}

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Keys | Kind::Output => Color32::from_rgb(120, 140, 160),
        Kind::Osc | Kind::Noise => Color32::from_rgb(230, 140, 60),
        Kind::Lfo | Kind::Env => Color32::from_rgb(90, 170, 230),
        Kind::Filter => Color32::from_rgb(170, 110, 220),
        Kind::Vca | Kind::Mix | Kind::Math => Color32::from_rgb(100, 190, 120),
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

        ui.horizontal(|ui| {
            ui.label(RichText::new(&name).size(16.0).strong());
            ui.label(RichText::new("Weave modular").color(p.muted));
            ui.menu_button("Patches", |ui| {
                for (pname, fp) in factory() {
                    if ui.button(pname).clicked() {
                        patch = fp;
                        ed.selected = None;
                        ui.close();
                    }
                }
            });
            ui.menu_button("Add module", |ui| {
                for k in Kind::ALL {
                    if ui.button(kind_name(k)).clicked() {
                        // where the view is, a little in
                        let at = [40.0 - ed.pan.x, 40.0 - ed.pan.y];
                        add_node(&mut patch, k, at);
                        ui.close();
                    }
                }
            });
            ui.label("Voices");
            ui.add(egui::DragValue::new(&mut patch.voices).range(1..=MAX_VOICES));
            if ui
                .button("Reset")
                .on_hover_text("Start again from the default patch")
                .clicked()
            {
                patch = Patch::default();
                ed.selected = None;
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
        // modules partly off the canvas are cut at its edge
        ui.set_clip_rect(canvas.intersect(ui.clip_rect()));
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 4.0, p.roll_bg);
        // a dotted grid that moves with the canvas
        let step = 24.0;
        let off = vec2(ed.pan.x.rem_euclid(step), ed.pan.y.rem_euclid(step));
        let mut y = canvas.min.y + off.y;
        while y < canvas.max.y {
            let mut x = canvas.min.x + off.x;
            while x < canvas.max.x {
                painter.circle_filled(pos2(x, y), 1.0, p.grid);
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
        let pointer = ui.input(|i| i.pointer.hover_pos());

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
            let (pa, pb) = (out_pos(a, origin, o), in_pos(b, origin, i));
            let selected = ed.selected == Some(ci);
            let col = if selected {
                p.accent_strong
            } else {
                kind_color(a.kind).gamma_multiply(0.4 + 0.6 * c.amount.abs().min(1.0) as f32)
            };
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                bezier(pa, pb),
                false,
                Color32::TRANSPARENT,
                Stroke::new(if selected { 3.0 } else { 2.0 }, col),
            ));
            let m = mid(pa, pb);
            let handle = Rect::from_center_size(m, vec2(12.0, 12.0));
            let r = ui.interact(handle, ui.scope_id().with(("cable", ci)), Sense::click());
            painter.circle_filled(m, if r.hovered() { 6.0 } else { 4.0 }, col);
            if r.clicked() {
                picked = Some(ci);
            }
            if r.secondary_clicked() {
                pull = Some(ci);
            }
            r.on_hover_text(format!("{} → {} ({:.2})", c.output, c.input, c.amount));
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
        let mut automate: Option<(u32, String, f64)> = None;
        let mut drop_on: Option<(u32, usize)> = None;
        for n in patch.nodes.iter_mut() {
            let rect = node_rect(n, origin);
            if !canvas.intersects(rect) {
                continue;
            }
            let spec = n.kind.spec();
            painter.rect_filled(rect, 6.0, p.panel2);
            painter.rect_stroke(
                rect,
                6.0,
                Stroke::new(1.0, kind_color(n.kind)),
                egui::StrokeKind::Inside,
            );
            let title = Rect::from_min_size(rect.min, vec2(NODE_W, TITLE_H));
            painter.rect_filled(title, 6.0, kind_color(n.kind).gamma_multiply(0.35));
            painter.text(
                title.left_center() + vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                kind_name(n.kind),
                egui::FontId::proportional(13.0),
                p.text,
            );
            let t = ui.interact(
                title,
                ui.scope_id().with(("node", n.id)),
                Sense::click_and_drag(),
            );
            if t.dragged() {
                n.pos[0] += t.drag_delta().x;
                n.pos[1] += t.drag_delta().y;
            }
            let id = n.id;
            t.context_menu(|ui| {
                if ui.button("Remove module").clicked() {
                    remove = Some(id);
                    ui.close();
                }
            });

            // outputs: label and a dot on the right; drag from the dot
            for (o, oname) in spec.outputs.iter().enumerate() {
                let y = row_y(&rect, o);
                painter.text(
                    pos2(rect.max.x - 12.0, y),
                    egui::Align2::RIGHT_CENTER,
                    *oname,
                    egui::FontId::proportional(11.0),
                    p.text2,
                );
                let at = pos2(rect.max.x, y);
                let r = ui.interact(
                    Rect::from_center_size(at, vec2(14.0, 14.0)),
                    ui.scope_id().with(("out", id, o)),
                    Sense::drag(),
                );
                painter.circle_filled(at, PORT_R, kind_color(n.kind));
                if r.drag_started() {
                    ed.dragging = Some((id, o));
                }
            }
            // inputs and parameters: a dot on the left, a slider for a parameter
            let first = spec.outputs.len();
            for k in 0..spec.inputs.len() + spec.params.len() {
                let y = row_y(&rect, first + k);
                let at = pos2(rect.min.x, y);
                let near = pointer.is_some_and(|q| q.distance(at) < 10.0);
                let col = if near && ed.dragging.is_some() {
                    p.accent_strong
                } else {
                    p.border2
                };
                painter.circle_filled(at, PORT_R, col);
                if near && ed.dragging.is_some() {
                    drop_on = Some((id, k));
                }
                let row = Rect::from_min_size(
                    pos2(rect.min.x + 10.0, y - 9.0),
                    vec2(NODE_W - 18.0, 18.0),
                );
                if k < spec.inputs.len() {
                    painter.text(
                        row.left_center(),
                        egui::Align2::LEFT_CENTER,
                        spec.inputs[k].name,
                        egui::FontId::proportional(11.0),
                        p.text2,
                    );
                    continue;
                }
                let ps = &spec.params[k - spec.inputs.len()];
                let mut v = n.param(ps.name);
                let changed = if ps.steps > 0 {
                    let names: &[&str] = match (n.kind, ps.name) {
                        (Kind::Filter, "mode") => &FILTER_MODES,
                        _ => &WAVES,
                    };
                    let mut c = false;
                    ui.put(row, |ui: &mut Ui| {
                        egui::ComboBox::from_id_salt(("weave", id, ps.name))
                            .width(NODE_W - 24.0)
                            .selected_text(format!(
                                "{} {}",
                                ps.name,
                                names.get(v as usize).unwrap_or(&"")
                            ))
                            .show_ui(ui, |ui| {
                                for (i, nm) in names.iter().enumerate() {
                                    c |= ui.selectable_value(&mut v, i as f64, *nm).changed();
                                }
                            })
                            .response
                    });
                    c
                } else {
                    let r = ui.put(row, |ui: &mut Ui| {
                        ui.spacing_mut().slider_width = NODE_W - 80.0;
                        // the rail shows against the module
                        ui.visuals_mut().widgets.inactive.bg_fill = p.well_deep;
                        ui.add(
                            egui::Slider::new(&mut v, ps.min..=ps.max)
                                .logarithmic(ps.log)
                                .text(ps.name)
                                .show_value(false),
                        )
                    });
                    if r.double_clicked() {
                        v = ps.default;
                    }
                    let changed = r.changed() || r.double_clicked();
                    let pid = param_id(id, k - spec.inputs.len());
                    let label = format!("{} {}", kind_name(n.kind), ps.name);
                    let at = ps.normalize(v);
                    r.on_hover_text(format!("{} {v:.3}", ps.name))
                        .context_menu(|ui| {
                            if ui.button("Create automation clip").clicked() {
                                automate = Some((pid, label.clone(), at));
                                ui.close();
                            }
                        });
                    changed
                };
                if changed {
                    n.params.insert(ps.name.to_string(), v);
                }
            }
        }
        if let Some(id) = remove {
            remove_node(&mut patch, id);
            ed.selected = None;
        }

        // the cable being drawn
        if let Some((from, o)) = ed.dragging {
            if let (Some(n), Some(q)) = (patch.node(from), pointer) {
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    bezier(out_pos(n, origin, o), q),
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
            let r = node_rect(&n, Pos2::ZERO);
            let s = k.spec();
            for o in 0..s.outputs.len() {
                assert!(r.contains(out_pos(&n, Pos2::ZERO, o) - vec2(1.0, 0.0)));
            }
            for i in 0..s.inputs.len() + s.params.len() {
                assert!(r.contains(in_pos(&n, Pos2::ZERO, i) + vec2(1.0, 0.0)));
                assert!(k.input_index(input_name(k, i)) == Some(i));
            }
        }
    }
}
