// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave's window: the patch as modules on a canvas, joined by cables.
//!
//! Modules: click one to pick it (Shift or Ctrl click to add or take it from the
//! picked ones), drag the empty canvas to pick every module in a box, drag a module to
//! move it (and the others picked with it). Ctrl+C copies the picked modules with the
//! cables between them, Ctrl+V pastes them at the pointer, Ctrl+D duplicates them,
//! Delete takes them out; the right click menus have the same. A module with no cables
//! dropped onto a cable goes into it (the cable is lit while it would); Alt and a drag
//! pulls a module out of its chain and joins its neighbours.
//!
//! Cables: drag from an output (the dots on the right) to any row of a module (an
//! input or a parameter; the places it can go are ringed). An input takes one cable,
//! so a new one replaces the old, unless it sums them (the mixer's inputs, the output,
//! a pitch input, any parameter). Drag from a plugged input to take its cable out and
//! plug it elsewhere. Click a cable's handle to pick it (its amount is set in the bar
//! above, Delete pulls it out), double click to pull it out, right click for its menu.
//!
//! Controls: drag a parameter's bar up or down to set it (shift for fine steps, a
//! double or ctrl click for its default), or turn the wheel over it. A choice (a wave,
//! a filter type, the math operation) opens a list on a click, steps with the arrows
//! at its ends, the wheel or a drag. The right click on a control has only its own
//! menu (automation, a MIDI link, its default).
//!
//! The canvas: Ctrl and the wheel zoom about the pointer; the wheel elsewhere, or a
//! drag with the middle or right button, scrolls; Fit frames every module. Values
//! change the sound as they move (the player takes them live); new modules or cables
//! build the player again. A drag is one undo step.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{
    self, pos2, vec2, Color32, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
};
use las_dsp::modular::{
    factory, param_id, Cable, Kind, Node, ParamSpec, Patch, CATEGORIES, MATH_SYMBOLS, MAX_GLIDE,
    MAX_VOICES,
};
use serde_json::{json, Value};

const NODE_W: f32 = 180.0;
const TITLE_H: f32 = 22.0;
const ROW_H: f32 = 20.0;
const PORT_R: f32 = 5.0;
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.0;
/// The width of the step arrows at each end of a choice, at zoom 1.
const ARROW_W: f32 = 14.0;
/// The height of a sampler's waveform, at zoom 1.
const WAVE_H: f32 = 40.0;
/// The height of a frame's label strip, at zoom 1.
const FRAME_H: f32 = 20.0;
/// How near (in points) a moved module must come to a cable to go into it.
const INSERT_DIST: f32 = 14.0;

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

/// The choice `by` steps on from `v` of `n` choices, round from the last to the first.
pub fn step_choice(v: f64, by: i32, n: usize) -> f64 {
    let n = n.max(1) as i32;
    f64::from((v as i32 + by).rem_euclid(n))
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
    /// The modules picked (Delete takes them out, Ctrl+C copies them).
    picked: Vec<u32>,
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
    /// Where a box of the empty canvas is being dragged from (on the screen).
    boxing: Option<Pos2>,
    /// The module being moved by a drag, and whether the cable under it would take it.
    moving: Option<u32>,
    insert_on: Option<CableKey>,
    /// Modules copied, with the cables between them.
    clip: Option<Patch>,
    /// Where on the canvas the background's menu was opened (canvas units).
    menu_at: Option<Pos2>,
    /// What is typed in the add menu's search box.
    search: String,
    /// A press came while a popup or menu was open: the canvas does not take it, so
    /// the click that closes a menu does nothing else.
    swallow: bool,
    /// The module heard alone, and the cables the output had before.
    viewer: Option<(u32, Vec<Cable>)>,
    /// A right button drag with a modifier: a knife or a lazy connection.
    rgest: Option<RGesture>,
    /// Each sample's waveform (min and max by column), by sample id.
    peaks: std::collections::HashMap<String, Vec<(f32, f32)>>,
    /// Where the canvas was last drawn, for the channel, and when: things let go over
    /// it go to this patch.
    pub(crate) drop_zone: Option<(Rect, usize, std::time::Instant)>,
    /// A frame being dragged by its label.
    frame_drag: Option<usize>,
}

/// What a right button drag with a modifier does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RGesture {
    /// Shift: cut the cables the line crosses, from here.
    Knife(Pos2),
    /// Ctrl or Alt: connect this module to the one let go on.
    Lazy(u32, Pos2),
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

    /// The bar of row `k` (an input, then the parameters) of module `node`.
    pub fn row_rect_at(&self, p: &Patch, node: u32, k: usize) -> Rect {
        let n = p.node(node).expect("a module");
        let r = node_rect(n, self.origin, self.z());
        row_rect(&r, n.kind.spec().outputs.len() + k, self.z())
    }

    /// The middle of the title of module `node`.
    pub fn title_at(&self, p: &Patch, node: u32) -> Pos2 {
        let n = p.node(node).expect("a module");
        let r = node_rect(n, self.origin, self.z());
        pos2(r.center().x, r.min.y + TITLE_H * 0.5 * self.z())
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

    /// The middle of frame `i`'s label strip.
    pub fn frame_label_at(&self, p: &Patch, i: usize) -> Pos2 {
        let r = frame_rect(p, &p.frames[i], self.origin, self.z()).expect("a frame");
        pos2(r.center().x, r.min.y + FRAME_H * 0.5 * self.z())
    }

    /// The module heard alone.
    pub fn viewing(&self) -> Option<u32> {
        self.viewer.as_ref().map(|v| v.0)
    }

    /// The modules picked.
    pub fn picked(&self) -> Vec<u32> {
        let mut v = self.picked.clone();
        v.sort_unstable();
        v
    }

    /// The cable a dragged module would go into.
    pub fn insert_target(&self) -> Option<(u32, String, u32, String)> {
        self.insert_on.clone()
    }

    /// The canvas, on the screen, as it was drawn last frame.
    pub fn canvas(&self) -> Rect {
        Rect::from_min_size(self.origin - self.pan, self.view)
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Scroll so that canvas point (0, 0) is `at` from the canvas's corner.
    pub fn set_pan(&mut self, at: Vec2) {
        self.pan = at;
    }

    /// The canvas point under a place on the screen.
    pub fn world_at(&self, q: Pos2) -> Pos2 {
        ((q - self.origin) / self.z()).to_pos2()
    }
}

impl WeaveEditor {
    /// Let go of everything picked or held (a new patch came).
    fn forget(&mut self) {
        self.selected = None;
        self.picked.clear();
        self.knob = None;
        self.dragging = None;
        self.boxing = None;
        self.moving = None;
        self.insert_on = None;
    }

    fn z(&self) -> f32 {
        if self.zoom <= 0.0 {
            1.0
        } else {
            self.zoom
        }
    }

    fn pick_only(&mut self, id: u32) {
        self.picked = vec![id];
        self.selected = None;
    }

    /// Shift or Ctrl and a click: in or out of the picked ones.
    fn pick_toggle(&mut self, id: u32) {
        if let Some(i) = self.picked.iter().position(|&x| x == id) {
            self.picked.remove(i);
        } else {
            self.picked.push(id);
        }
        self.selected = None;
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
        Kind::Sampler => "Sampler",
        Kind::Reroute => "Reroute",
    }
}

/// The parameter that sets what a kind of module does (its wave, its type, its
/// operation), shown in its title.
pub fn mode_param(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::Osc | Kind::Lfo => "wave",
        Kind::Filter | Kind::Ladder => "mode",
        Kind::Shaper => "shape",
        Kind::Quant => "scale",
        Kind::Math => "op",
        _ => return None,
    })
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A module's title: its kind and, for one whose mode is a choice, that choice
/// ("Math: Multiply", "Filter: Lowpass").
pub fn title_text(n: &Node) -> String {
    let name = kind_name(n.kind);
    match mode_param(n.kind).and_then(|m| Some((n.kind.choices(m)?, n.param(m) as usize))) {
        Some((c, v)) => format!("{name}: {}", capitalized(c.get(v).copied().unwrap_or(""))),
        None => name.to_string(),
    }
}

/// The small mark drawn at the right of a module's title for its mode: the math
/// operation's symbol, or a filter's or quantizer's short name (waves are drawn).
pub fn mode_badge(n: &Node) -> Option<String> {
    let v = mode_param(n.kind).map(|m| n.param(m) as usize)?;
    Some(match n.kind {
        Kind::Math => MATH_SYMBOLS.get(v).copied().unwrap_or("").to_string(),
        Kind::Filter => ["LP", "BP", "HP"].get(v).copied().unwrap_or("").to_string(),
        Kind::Ladder => ["24", "12"].get(v).copied().unwrap_or("").to_string(),
        Kind::Shaper => ["tanh", "fold", "clip"]
            .get(v)
            .copied()
            .unwrap_or("")
            .to_string(),
        Kind::Quant => ["chr", "maj", "min", "pent"]
            .get(v)
            .copied()
            .unwrap_or("")
            .to_string(),
        // a wave is drawn, not written
        _ => return None,
    })
}

/// What a port is for, for its tooltip.
pub fn port_tip(k: Kind, name: &str, output: bool) -> String {
    let what = match (output, name) {
        (true, "pitch") => "the pitch of the key played (into an oscillator's pitch)",
        (true, "gate") => "1 while a key is held, 0 when let go (into an envelope's gate)",
        (true, "velocity") => "how hard the key was played, 0 to 1",
        (true, "wheel") => "the mod wheel, 0 to 1",
        (true, "bend") => "the pitch bend, -1 to 1",
        (true, "pressure") => "the key pressure (aftertouch), 0 to 1",
        (true, "left" | "right") => "one side of the stereo signal",
        (true, "inv") => "the opposite of out (1 where out is 0)",
        (true, _) if k.category() == 1 || k == Kind::Macro => {
            "a control signal, to move parameters or other inputs"
        }
        (true, _) => "the signal this module makes",
        (false, "gate") if k == Kind::Sampler => {
            "starts the sample as this goes up (unplugged: the key's gate)"
        }
        (false, "pitch") if k == Kind::Sampler => {
            "pitch in, 1 an octave (unplugged: the key played)"
        }
        (false, "pitch") => "pitch in, 1 an octave (from Keys, a sequencer or a quantizer)",
        (false, "fm") => "frequency modulation: a signal that bends the pitch",
        (false, "pfm") => "phase modulation: a signal that pushes the wave's phase",
        (false, "pm") => "phase modulation from another operator",
        (false, "sync") => "hard sync: the wave starts again as this goes up",
        (false, "gate") => "the envelope starts when this goes up and lets go when it falls",
        (false, "pos") => "moves where the sample is read from, by up to its whole length",
        (false, "in") if k == Kind::Reroute => "passes straight to out",
        (false, "trig") => "takes a new sample each time this goes up",
        (false, "clock") => "steps the sequence each time this goes up",
        (false, "reset") => "back to the first step when this goes up",
        (false, "cv") => "how loud: 0 is silent, 1 lets the signal through",
        (false, "in") => "the signal to work on",
        (false, "left" | "right") => "one side of what is heard",
        (false, "a" | "b" | "c" | "d") if k == Kind::Mix => "a signal to mix in",
        (false, "a") => "the first signal",
        (false, "b") => "the second signal",
        (false, _) => "a signal",
    };
    let cables = if output {
        "drag to an input or a parameter (as many cables as you like)"
    } else if k.accepts_many(name) {
        "takes several cables, added together"
    } else {
        "takes one cable: a new one replaces it"
    };
    format!("{name}: {what}\n{cables}")
}

/// The size of a module at zoom 1.
fn node_size(k: Kind) -> Vec2 {
    let w = if k == Kind::Reroute { 90.0 } else { NODE_W };
    let extra = if k == Kind::Sampler {
        WAVE_H + 4.0
    } else {
        0.0
    };
    vec2(w, TITLE_H + k.rows() as f32 * ROW_H + 6.0 + extra)
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
        vec2(r.width() - 20.0 * z, 16.0 * z),
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

/// The size of a port's hit area: never too small to catch, zoomed out.
fn port_hit(z: f32) -> Vec2 {
    Vec2::splat(18.0 * z.clamp(0.8, 1.6))
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
/// there already is not doubled. An input that takes one cable gives up the one it
/// had (as in Blender): the new cable replaces it.
pub fn connect(p: &mut Patch, from: u32, output: &str, to: u32, input: &str) -> bool {
    let Some(target) = p.node(to) else {
        return false;
    };
    let kind = target.kind;
    let is_param = kind.param(input).is_some();
    let c = Cable::new(from, output, to, input, if is_param { 0.5 } else { 1.0 });
    if !p.is_valid(&c)
        || p.cables
            .iter()
            .any(|x| x.from == from && x.output == output && x.to == to && x.input == input)
    {
        return false;
    }
    if !kind.accepts_many(input) {
        p.cables.retain(|x| !(x.to == to && x.input == input));
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

/// The modules `ids` with the cables between them (a clipboard's worth).
pub fn copy_nodes(p: &Patch, ids: &[u32]) -> Option<Patch> {
    let nodes: Vec<Node> = p
        .nodes
        .iter()
        .filter(|n| ids.contains(&n.id))
        .cloned()
        .collect();
    if nodes.is_empty() {
        return None;
    }
    let cables = p
        .cables
        .iter()
        .filter(|c| ids.contains(&c.from) && ids.contains(&c.to))
        .cloned()
        .collect();
    Some(Patch::new(nodes, cables, 1))
}

/// Paste copied modules (and their cables) with their top left corner at `at`; the
/// new modules' ids.
pub fn paste_nodes(p: &mut Patch, clip: &Patch, at: [f32; 2]) -> Vec<u32> {
    let min = clip.nodes.iter().fold([f32::MAX; 2], |m, n| {
        [m[0].min(n.pos[0]), m[1].min(n.pos[1])]
    });
    let mut map = Vec::new();
    for (next, n) in (p.free_id()..).zip(clip.nodes.iter()) {
        let mut c = n.clone();
        map.push((n.id, next));
        c.id = next;
        c.pos = [at[0] + n.pos[0] - min[0], at[1] + n.pos[1] - min[1]];
        p.nodes.push(c);
    }
    let new = |old: u32| map.iter().find(|(o, _)| *o == old).map(|(_, n)| *n);
    for c in &clip.cables {
        if let (Some(from), Some(to)) = (new(c.from), new(c.to)) {
            p.cables.push(Cable {
                from,
                to,
                ..c.clone()
            });
        }
    }
    map.into_iter().map(|(_, n)| n).collect()
}

/// A copy of modules with their parameters and the cables between them, a little
/// below and to the right; the copies' ids.
pub fn duplicate_nodes(p: &mut Patch, ids: &[u32]) -> Vec<u32> {
    let Some(clip) = copy_nodes(p, ids) else {
        return Vec::new();
    };
    let min = clip.nodes.iter().fold([f32::MAX; 2], |m, n| {
        [m[0].min(n.pos[0]), m[1].min(n.pos[1])]
    });
    paste_nodes(p, &clip, [min[0] + 30.0, min[1] + 30.0])
}

/// Whether a module can go into a cable: it has an input and an output and no cables.
fn insertable(p: &Patch, id: u32) -> bool {
    p.node(id).is_some_and(|n| {
        let s = n.kind.spec();
        !s.inputs.is_empty() && !s.outputs.is_empty()
    }) && !p.cables.iter().any(|c| c.from == id || c.to == id)
}

/// Put module `id` into a cable: the cable's source into the module's first input,
/// the module's first output into where the cable went (with its amount).
pub fn insert_into_cable(p: &mut Patch, id: u32, key: &CableKey) -> bool {
    if !insertable(p, id) || key.0 == id || key.2 == id {
        return false;
    }
    let Some(ci) = p.cables.iter().position(|c| cable_key(c) == *key) else {
        return false;
    };
    let Some(s) = p.node(id).map(|n| n.kind.spec()) else {
        return false;
    };
    let old = p.cables.remove(ci);
    p.cables
        .push(Cable::new(old.from, &old.output, id, s.inputs[0].name, 1.0));
    p.cables
        .push(Cable::new(id, s.outputs[0], old.to, &old.input, old.amount));
    true
}

/// Take module `id` out of its chain: what came into its first input goes straight to
/// where its first output went; its cables come out.
pub fn dissolve(p: &mut Patch, id: u32) {
    let Some(s) = p.node(id).map(|n| n.kind.spec()) else {
        return;
    };
    let source = s.inputs.first().and_then(|i| {
        p.cables
            .iter()
            .find(|c| c.to == id && c.input == i.name)
            .map(|c| (c.from, c.output.clone()))
    });
    let dests: Vec<(u32, String, f64)> = s
        .outputs
        .first()
        .map(|o| {
            p.cables
                .iter()
                .filter(|c| c.from == id && c.output == *o)
                .map(|c| (c.to, c.input.clone(), c.amount))
                .collect()
        })
        .unwrap_or_default();
    p.cables.retain(|c| c.from != id && c.to != id);
    if let Some((from, output)) = source {
        for (to, input, amount) in dests {
            if from != to && connect(p, from, &output, to, &input) {
                if let Some(c) = p.cables.last_mut() {
                    c.amount = amount;
                }
            }
        }
    }
}

/// Connect two modules by their best matching ports (Node Wrangler's lazy connect):
/// `a`'s first output into `b`'s first plain input that is free (or sums), else its
/// first plain input (the old cable is replaced).
pub fn lazy_connect(p: &mut Patch, a: u32, b: u32) -> bool {
    if a == b {
        return false;
    }
    let (Some(na), Some(nb)) = (p.node(a), p.node(b)) else {
        return false;
    };
    let (sa, sb) = (na.kind.spec(), nb.kind.spec());
    let Some(out) = sa.outputs.first().copied() else {
        return false;
    };
    let kb = nb.kind;
    let free = sb.inputs.iter().find(|i| {
        kb.accepts_many(i.name) || !p.cables.iter().any(|c| c.to == b && c.input == i.name)
    });
    let Some(input) = free.or(sb.inputs.first()).map(|i| i.name) else {
        return false;
    };
    connect(p, a, out, b, input)
}

/// Whether segments `a`-`b` and `c`-`d` cross.
fn crosses(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> bool {
    let cross = |o: Pos2, p: Pos2, q: Pos2| (p - o).x * (q - o).y - (p - o).y * (q - o).x;
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    // touching counts (a knife through a curve's own point)
    d1 * d2 <= 0.0 && d3 * d4 <= 0.0 && (d1 != d2 || d3 != d4)
}

/// The points of a cable's curve, for hit tests.
fn cable_points(a: Pos2, b: Pos2, z: f32) -> Vec<Pos2> {
    let [p0, p1, p2, p3] = bezier(a, b, z);
    (0..=32)
        .map(|i| {
            let t = i as f32 / 32.0;
            let u = 1.0 - t;
            (p0.to_vec2() * (u * u * u)
                + p1.to_vec2() * (3.0 * u * u * t)
                + p2.to_vec2() * (3.0 * u * t * t)
                + p3.to_vec2() * (t * t * t))
                .to_pos2()
        })
        .collect()
}

/// Cut every cable the knife line from `a` to `b` crosses (screen points); how many.
pub fn cut_cables(p: &mut Patch, origin: Pos2, z: f32, a: Pos2, b: Pos2) -> usize {
    let before = p.cables.len();
    let cut: Vec<CableKey> = p
        .cables
        .iter()
        .filter(|c| {
            let (Some(na), Some(nb)) = (p.node(c.from), p.node(c.to)) else {
                return false;
            };
            let (Some(o), Some(i)) = (
                na.kind.output_index(&c.output),
                nb.kind.input_index(&c.input),
            ) else {
                return false;
            };
            let pts = cable_points(out_pos(na, origin, o, z), in_pos(nb, origin, i, z), z);
            pts.windows(2).any(|w| crosses(w[0], w[1], a, b))
        })
        .map(cable_key)
        .collect();
    p.cables.retain(|c| !cut.contains(&cable_key(c)));
    before - p.cables.len()
}

/// Swap what two modules feed (Node Wrangler's Alt+S): the cables out of `a` come
/// from `b` and the other way round, output by output where both have it.
pub fn swap_outputs(p: &mut Patch, a: u32, b: u32) -> bool {
    let (Some(ka), Some(kb)) = (p.node(a).map(|n| n.kind), p.node(b).map(|n| n.kind)) else {
        return false;
    };
    if a == b {
        return false;
    }
    let (oa, ob) = (ka.spec().outputs, kb.spec().outputs);
    let mut changed = false;
    for c in &mut p.cables {
        let (from, other, mine, theirs) = if c.from == a {
            (a, b, oa, ob)
        } else if c.from == b {
            (b, a, ob, oa)
        } else {
            continue;
        };
        let _ = from;
        if let Some(o) = mine.iter().position(|x| *x == c.output) {
            if let Some(t) = theirs.get(o) {
                c.from = other;
                c.output = (*t).to_string();
                changed = true;
            }
        }
    }
    // a module fed by itself after the swap is not kept
    p.cables.retain(|c| c.from != c.to);
    changed
}

/// Line the modules `ids` up: in a row (same top) when they spread more across than
/// down, else in a column (same left).
pub fn align_nodes(p: &mut Patch, ids: &[u32]) -> bool {
    let pos: Vec<[f32; 2]> = p
        .nodes
        .iter()
        .filter(|n| ids.contains(&n.id))
        .map(|n| n.pos)
        .collect();
    if pos.len() < 2 {
        return false;
    }
    let span = |k: usize| {
        let (lo, hi) = pos.iter().fold((f32::MAX, f32::MIN), |(lo, hi), q| {
            (lo.min(q[k]), hi.max(q[k]))
        });
        (lo, hi - lo)
    };
    let ((x0, w), (y0, h)) = (span(0), span(1));
    for n in p.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
        if w >= h {
            n.pos[1] = y0;
        } else {
            n.pos[0] = x0;
        }
    }
    true
}

/// Put a reroute dot into a cable with its dot at `at` (canvas units); its id.
pub fn add_reroute(p: &mut Patch, key: &CableKey, at: Pos2) -> Option<u32> {
    // the dot of a reroute's input is at the left of its first row after the output
    let off = vec2(0.0, TITLE_H + ROW_H * 1.5);
    let id = add_node(p, Kind::Reroute, [at.x - off.x, at.y - off.y]);
    if insert_into_cable(p, id, key) {
        Some(id)
    } else {
        remove_node(p, id);
        None
    }
}

/// Put the modules `ids` in a new frame (out of any frame they were in).
pub fn frame_nodes(p: &mut Patch, ids: &[u32], label: &str) -> bool {
    let ids: Vec<u32> = ids
        .iter()
        .copied()
        .filter(|id| p.node(*id).is_some())
        .collect();
    if ids.is_empty() {
        return false;
    }
    for f in &mut p.frames {
        f.nodes.retain(|n| !ids.contains(n));
    }
    p.frames.retain(|f| !f.nodes.is_empty());
    p.frames.push(las_dsp::modular::Frame {
        label: label.to_string(),
        nodes: ids,
    });
    true
}

/// Where a frame is drawn: round its modules, with room for its label above.
fn frame_rect(p: &Patch, f: &las_dsp::modular::Frame, origin: Pos2, z: f32) -> Option<Rect> {
    let r = p
        .nodes
        .iter()
        .filter(|n| f.nodes.contains(&n.id))
        .map(|n| node_rect(n, origin, z))
        .reduce(|a, b| a.union(b))?;
    Some(Rect::from_min_max(
        r.min - vec2(10.0, 10.0 + FRAME_H) * z,
        r.max + vec2(10.0, 10.0) * z,
    ))
}

/// Hear one module alone (Node Wrangler's viewer): its outputs go straight to the
/// output's two sides in place of what was there, which is given back to put back.
pub fn view_node(p: &mut Patch, id: u32) -> Option<Vec<Cable>> {
    let out = p.nodes.iter().find(|n| n.kind == Kind::Output)?.id;
    let outs = p.node(id)?.kind.spec().outputs;
    let first = *outs.first()?;
    if id == out {
        return None;
    }
    let old: Vec<Cable> = p.cables.iter().filter(|c| c.to == out).cloned().collect();
    p.cables.retain(|c| c.to != out);
    p.cables.push(Cable::new(id, first, out, "left", 1.0));
    p.cables.push(Cable::new(
        id,
        outs.get(1).copied().unwrap_or(first),
        out,
        "right",
        1.0,
    ));
    Some(old)
}

/// Take the viewer out again: the output gets back the cables it had.
pub fn unview(p: &mut Patch, old: Vec<Cable>) {
    let Some(out) = p
        .nodes
        .iter()
        .find(|n| n.kind == Kind::Output)
        .map(|n| n.id)
    else {
        return;
    };
    p.cables.retain(|c| c.to != out);
    let back: Vec<Cable> = old
        .into_iter()
        .filter(|c| p.node(c.from).is_some())
        .collect();
    p.cables.extend(back);
}

/// A sample let go at `at` (canvas units): the sampler `hit` (when it is one) takes it,
/// else a new sampler there plays it into the output. The sampler's id.
pub fn place_sample(p: &mut Patch, sample: &str, at: Pos2, hit: Option<u32>) -> u32 {
    if let Some(id) = hit.filter(|id| p.node(*id).is_some_and(|n| n.kind == Kind::Sampler)) {
        if let Some(n) = p.nodes.iter_mut().find(|n| n.id == id) {
            n.sample = Some(sample.to_string());
        }
        return id;
    }
    let id = add_node(p, Kind::Sampler, [at.x, at.y]);
    if let Some(n) = p.nodes.iter_mut().find(|n| n.id == id) {
        n.sample = Some(sample.to_string());
    }
    if let Some(out) = p
        .nodes
        .iter()
        .find(|n| n.kind == Kind::Output)
        .map(|n| n.id)
    {
        connect(p, id, "left", out, "left");
        connect(p, id, "right", out, "right");
    }
    id
}

/// Every Weave shortcut, for the help and the tooltips.
pub const SHORTCUTS: &str = "Click a module: pick it. Shift or Ctrl click: pick more. Drag the empty canvas: pick in a box.\n\
Ctrl+C / Ctrl+V: copy, paste at the pointer. Ctrl+D: duplicate. Delete: remove. Ctrl+A: pick all.\n\
Drag a module onto a cable: put it in. Alt+drag a module: pull it out of its chain.\n\
Ctrl+Shift+click a module: hear it alone (again to stop).\n\
Ctrl+right drag (or Alt+right drag) from one module to another: connect them.\n\
Shift+right drag across cables: cut them.\n\
Ctrl+J: put the picked modules in a frame (drag its label to move them).\n\
Alt+S: swap what two picked modules feed. Shift+A: line up the picked modules.\n\
Shift+click a cable's dot: add a reroute dot. Double click it: pull the cable out.\n\
Wheel: scroll, or turn the control under the pointer. Ctrl+wheel: zoom. Middle or right drag: scroll.\n\
Drop a sample (from the browser, your files or the playlist's picker) on the canvas: a sampler plays it.";

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

/// How far `q` is from a cable drawn from `a` to `b`.
fn cable_dist(a: Pos2, b: Pos2, z: f32, q: Pos2) -> f32 {
    let [p0, p1, p2, p3] = bezier(a, b, z);
    let at = |t: f32| {
        let u = 1.0 - t;
        (p0.to_vec2() * (u * u * u)
            + p1.to_vec2() * (3.0 * u * u * t)
            + p2.to_vec2() * (3.0 * u * t * t)
            + p3.to_vec2() * (t * t * t))
            .to_pos2()
    };
    let mut best = f32::MAX;
    let mut last = at(0.0);
    for i in 1..=32 {
        let next = at(i as f32 / 32.0);
        let seg = next - last;
        let t = ((q - last).dot(seg) / seg.length_sq().max(1e-6)).clamp(0.0, 1.0);
        best = best.min((last + seg * t).distance(q));
        last = next;
    }
    best
}

/// The cable nearest `q` (within [`INSERT_DIST`]) that does not touch module `skip`.
fn cable_near(p: &Patch, origin: Pos2, z: f32, q: Pos2, skip: u32) -> Option<CableKey> {
    p.cables
        .iter()
        .filter(|c| c.from != skip && c.to != skip)
        .filter_map(|c| {
            let (a, b) = (p.node(c.from)?, p.node(c.to)?);
            let (o, i) = (
                a.kind.output_index(&c.output)?,
                b.kind.input_index(&c.input)?,
            );
            let d = cable_dist(out_pos(a, origin, o, z), in_pos(b, origin, i, z), z, q);
            (d <= INSERT_DIST).then(|| (d, cable_key(c)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, k)| k)
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

/// A small picture of a wave (by its name) in `r`.
fn draw_wave(painter: &egui::Painter, r: Rect, wave: &str, stroke: Stroke) {
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let y = match wave {
                "sine" => (t * std::f32::consts::TAU).sin(),
                "triangle" => 1.0 - 4.0 * (t - 0.5).abs(),
                "saw" => 1.0 - 2.0 * t,
                "square" => {
                    if t < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                "random" => [0.6, -0.3, 0.9, -0.8, 0.2, -0.5][((t * 5.99) as usize).min(5)],
                // morph: a sine leaning into a saw
                _ => (t * std::f32::consts::TAU).sin() * 0.5 + (1.0 - 2.0 * t) * 0.5,
            };
            pos2(r.min.x + t * r.width(), r.center().y - y * r.height() * 0.5)
        })
        .collect();
    painter.add(egui::Shape::line(pts, stroke));
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

/// The kinds the add menu lists for what is typed in its search box (by name or
/// category; all of them for an empty search).
pub fn search_kinds(text: &str) -> Vec<Kind> {
    let t = text.trim().to_lowercase();
    Kind::ALL
        .into_iter()
        .filter(|k| {
            t.is_empty()
                || kind_name(*k).to_lowercase().contains(&t)
                || CATEGORIES[k.category()].to_lowercase().contains(&t)
        })
        .collect()
}

/// The add menu: a search box, then the kinds by category (or those found). The
/// kind picked.
fn add_menu(ui: &mut Ui, search: &mut String) -> Option<Kind> {
    let mut picked = None;
    let r = ui.add(
        egui::TextEdit::singleline(search)
            .hint_text("Search modules")
            .desired_width(160.0),
    );
    // typing finds a module at once: the box takes the keys while the menu is open
    // and no other text box has them
    if !r.has_focus() && !ui.ctx().egui_wants_keyboard_input() {
        r.request_focus();
    }
    if search.trim().is_empty() {
        for (ci, cat) in CATEGORIES.iter().enumerate() {
            ui.menu_button(*cat, |ui| {
                for k in Kind::ALL.into_iter().filter(|k| k.category() == ci) {
                    if ui.button(kind_name(k)).clicked() {
                        picked = Some(k);
                        ui.close();
                    }
                }
            });
        }
    } else {
        let found = search_kinds(search);
        if found.is_empty() {
            ui.label("Nothing found");
        }
        for k in found {
            if ui.button(kind_name(k)).clicked() {
                picked = Some(k);
                ui.close();
            }
        }
    }
    if picked.is_some() {
        search.clear();
    }
    picked
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

    /// The Weave channel whose canvas is under a drop at `at`, drawn a moment ago.
    pub(crate) fn weave_drop_target(
        &self,
        at: Option<Pos2>,
        over: crate::shell::Over,
    ) -> Option<usize> {
        let (r, ch, t) = self.weave.drop_zone?;
        (over == crate::shell::Over::Other
            && t.elapsed().as_secs_f32() < 1.0
            && at.is_some_and(|q| r.contains(q)))
        .then_some(ch)
    }

    /// A sound let go on Weave's canvas: a sampler under it takes it, or a new sampler
    /// there plays it into the output. One undo step.
    pub(crate) fn drop_on_weave(
        &mut self,
        item: &crate::shell::Dragged,
        channel: usize,
        at: Option<Pos2>,
    ) {
        let Some((id, _)) = self.add_dropped_sample(item) else {
            return;
        };
        let Some(state) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(channel))
            .map(|c| c.state.clone())
        else {
            return;
        };
        let mut patch = patch_of(&state);
        let (origin, z) = (self.weave.origin, self.weave.z());
        let q = at.unwrap_or(origin + self.weave.view * 0.5);
        let hit = patch
            .nodes
            .iter()
            .rev()
            .find(|n| node_rect(n, origin, z).contains(q))
            .map(|n| n.id);
        let world = ((q - origin) / z).to_pos2();
        let node = place_sample(&mut patch, &id, world, hit);
        self.weave.picked = vec![node];
        let value = patch.to_json();
        self.project.modify(false, |p| {
            if let Some(c) = p.channels.get_mut(channel) {
                if !c.state.is_object() {
                    c.state = json!({});
                }
                c.state["patch"] = value;
            }
        });
    }

    /// Tell the player about new values of a Weave patch, so they are heard at once.
    pub(crate) fn send_weave_live(&mut self, channel: usize, patch: &Patch) {
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
        // the project's samples (to pick one for a sampler) and the waveforms of the
        // ones the samplers play
        let sample_list: Vec<(String, String)> = self
            .project
            .loaded
            .as_ref()
            .map(|l| {
                l.project
                    .samples
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // the audio clip channels' samples, for a clip dragged from the playlist's picker
        let clip_samples: Vec<(String, Option<String>)> = self
            .project
            .loaded
            .as_ref()
            .map(|l| {
                l.project
                    .channels
                    .iter()
                    .map(|c| (c.id.clone(), c.sample_id.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for n in &patch.nodes {
            if let Some(id) = n
                .sample
                .as_ref()
                .filter(|id| !self.weave.peaks.contains_key(*id))
            {
                let audio = las_engine::factory::render_factory(id, 44_100.0).or_else(|| {
                    let l = self.project.loaded.as_ref()?;
                    las_engine::library::decode_wav(&l.samples.get(id)?.data)
                });
                let peaks = audio
                    .map(|a| crate::samplerui::peaks(&a, 80))
                    .unwrap_or_default();
                self.weave.peaks.insert(id.clone(), peaks);
            }
        }
        let ed = &mut self.weave;
        let p = pal();
        let skin = crate::skin::current();
        let (down, now, fine, ctrl, alt, pressed, released) = ui.input(|i| {
            (
                i.pointer.any_down(),
                i.time,
                i.modifiers.shift,
                i.modifiers.command,
                i.modifiers.alt,
                i.pointer.any_pressed(),
                i.pointer.any_released(),
            )
        });
        let add_key = fine || ctrl;
        if !down {
            ed.gesture = false;
        }
        // a press while a list or a menu is open only closes it: the canvas under it
        // does not take the same click
        if pressed && egui::Popup::is_any_open(ui.ctx()) {
            ed.swallow = true;
        } else if !down && !released {
            ed.swallow = false;
        }
        let swallow = ed.swallow;
        let sense = |s: Sense| if swallow { Sense::hover() } else { s };
        if ed.seen != Some(channel) {
            ed.seen = Some(channel);
            ed.fit = true;
            ed.forget();
        } else if ed.last.as_ref().is_some_and(|l| *l != patch) {
            // changed from outside (undo, redo, a preset, automation clips): what was
            // being dragged may be gone
            ed.knob = None;
            ed.dragging = None;
            ed.moving = None;
            ed.insert_on = None;
            ed.picked.retain(|id| patch.node(*id).is_some());
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
                if let Some(k) = add_menu(ui, &mut ed.search) {
                    // in the middle of the view
                    let z = ed.z();
                    let at = (ed.view * 0.5 - ed.pan) / z - node_size(k) * 0.5;
                    let id = add_node(&mut patch, k, [at.x, at.y]);
                    ed.pick_only(id);
                    discrete = true;
                }
            });
            if ui
                .button("Fit")
                .on_hover_text("Zoom and scroll to show every module")
                .clicked()
            {
                ed.fit = true;
            }
            ui.label(RichText::new(" ? ").strong())
                .on_hover_text(SHORTCUTS);
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

        let (canvas, bg) =
            ui.allocate_exact_size(ui.available_size(), sense(Sense::click_and_drag()));
        ed.view = canvas.size();
        ed.drop_zone = Some((canvas, channel, std::time::Instant::now()));
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
        // the background: the main button picks modules in a box, the middle or right
        // button scrolls
        let busy = ed.dragging.is_some()
            || ed.knob.is_some()
            || ed.moving.is_some()
            || ed.rgest.is_some()
            || ed.frame_drag.is_some();
        if (bg.dragged_by(PointerButton::Middle) || bg.dragged_by(PointerButton::Secondary))
            && !busy
        {
            ed.pan += bg.drag_delta();
        }
        if bg.drag_started_by(PointerButton::Primary) && !busy {
            ed.boxing = bg.interact_pointer_pos().map(|q| q - bg.drag_delta());
        }
        if bg.clicked() {
            ed.selected = None;
            ed.picked.clear();
        }
        let origin = canvas.min + ed.pan;
        ed.origin = origin;
        let world = |q: Pos2| ((q - origin) / z).to_pos2();
        if bg.secondary_clicked() {
            ed.menu_at = bg.interact_pointer_pos().map(world);
        }
        if let Some(start) = ed.boxing {
            let end = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, end);
            painter.rect_filled(r, 2.0, p.accent_strong.gamma_multiply(0.12));
            painter.rect_stroke(
                r,
                2.0,
                Stroke::new(1.0, p.accent_strong),
                egui::StrokeKind::Inside,
            );
            if !down {
                let inside: Vec<u32> = patch
                    .nodes
                    .iter()
                    .filter(|n| r.intersects(node_rect(n, origin, z)))
                    .map(|n| n.id)
                    .collect();
                if !add_key {
                    ed.picked.clear();
                }
                for id in inside {
                    if !ed.picked.contains(&id) {
                        ed.picked.push(id);
                    }
                }
                ed.selected = None;
                ed.boxing = None;
            }
            ui.ctx().request_repaint();
        }

        // what the background's menu and the keys ask for
        let mut copy = false;
        let mut paste_at: Option<Pos2> = None;
        let mut duplicate = false;
        let mut delete = false;
        let mut add_at: Option<(Kind, Pos2)> = None;
        let can_paste = ed.clip.is_some();
        let has_picked = !ed.picked.is_empty();
        let menu_at = ed.menu_at;
        let mut select_all = false;
        let mut fit = false;
        let mut frame_now = false;
        let mut swap_now = false;
        let mut align_now = false;
        let picked_two = ed.picked.len() == 2;
        bg.context_menu(|ui| {
            ui.menu_button("Add module here", |ui| {
                if let Some(k) = add_menu(ui, &mut ed.search) {
                    add_at = Some((k, menu_at.unwrap_or_default()));
                }
            });
            ui.separator();
            if ui
                .add_enabled(
                    has_picked,
                    egui::Button::new("Copy").shortcut_text("Ctrl+C"),
                )
                .clicked()
            {
                copy = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    can_paste,
                    egui::Button::new("Paste").shortcut_text("Ctrl+V"),
                )
                .clicked()
            {
                paste_at = Some(menu_at.unwrap_or_default());
                ui.close();
            }
            if ui
                .add_enabled(
                    has_picked,
                    egui::Button::new("Duplicate").shortcut_text("Ctrl+D"),
                )
                .clicked()
            {
                duplicate = true;
                ui.close();
            }
            if ui
                .add_enabled(has_picked, egui::Button::new("Delete").shortcut_text("Del"))
                .clicked()
            {
                delete = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    has_picked,
                    egui::Button::new("Frame").shortcut_text("Ctrl+J"),
                )
                .on_hover_text(
                    "Put the picked modules in a labelled frame that moves them together",
                )
                .clicked()
            {
                frame_now = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    has_picked,
                    egui::Button::new("Align").shortcut_text("Shift+A"),
                )
                .on_hover_text("Line the picked modules up in a row or a column")
                .clicked()
            {
                align_now = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    picked_two,
                    egui::Button::new("Swap outputs").shortcut_text("Alt+S"),
                )
                .on_hover_text("The two picked modules swap what they feed")
                .clicked()
            {
                swap_now = true;
                ui.close();
            }
            ui.separator();
            if ui
                .add(egui::Button::new("Select all").shortcut_text("Ctrl+A"))
                .clicked()
            {
                select_all = true;
                ui.close();
            }
            if ui.button("Fit view").clicked() {
                fit = true;
                ui.close();
            }
            ui.menu_button("Shortcuts", |ui| {
                ui.label(SHORTCUTS);
            });
        });
        // the keys, while the pointer is over the canvas and nothing is being typed
        let typing = ui.ctx().egui_wants_keyboard_input();
        if over_canvas && !typing {
            let (j, swap, align) = ui.input(|i| {
                let cmd = i.modifiers.command;
                (
                    cmd && i.key_pressed(egui::Key::J),
                    i.modifiers.alt && i.key_pressed(egui::Key::S),
                    i.modifiers.shift && !cmd && i.key_pressed(egui::Key::A),
                )
            });
            frame_now |= j;
            swap_now |= swap;
            align_now |= align;
            let (c, v, d, a, del, esc) = ui.input(|i| {
                let k = |key| i.key_pressed(key);
                let ev = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
                let cmd = i.modifiers.command;
                (
                    ev(|e| matches!(e, egui::Event::Copy)) || cmd && k(egui::Key::C),
                    ev(|e| matches!(e, egui::Event::Paste(_))) || cmd && k(egui::Key::V),
                    cmd && k(egui::Key::D),
                    cmd && k(egui::Key::A),
                    k(egui::Key::Delete) || k(egui::Key::Backspace),
                    k(egui::Key::Escape),
                )
            });
            copy |= c;
            if v {
                paste_at = pointer.map(world);
            }
            duplicate |= d;
            select_all |= a;
            if del {
                // a picked cable goes first, then the picked modules
                if let Some(k) = ed.selected.take() {
                    patch.cables.retain(|c| cable_key(c) != k);
                    discrete = true;
                } else {
                    delete = true;
                }
            }
            if esc && ed.dragging.is_none() {
                ed.picked.clear();
                ed.selected = None;
            }
        }
        if select_all {
            ed.picked = patch.nodes.iter().map(|n| n.id).collect();
        }
        if frame_now && frame_nodes(&mut patch, &ed.picked, "Frame") {
            discrete = true;
        }
        if align_now && align_nodes(&mut patch, &ed.picked) {
            discrete = true;
        }
        if swap_now && ed.picked.len() == 2 && swap_outputs(&mut patch, ed.picked[0], ed.picked[1])
        {
            discrete = true;
        }
        if fit {
            ed.fit = true;
        }
        if copy {
            if let Some(clip) = copy_nodes(&patch, &ed.picked) {
                ed.clip = Some(clip);
            }
        }
        if let (Some(at), Some(clip)) = (paste_at, ed.clip.clone()) {
            ed.picked = paste_nodes(&mut patch, &clip, [at.x, at.y]);
            ed.selected = None;
            discrete = true;
        }
        if duplicate && !ed.picked.is_empty() {
            ed.picked = duplicate_nodes(&mut patch, &ed.picked);
            discrete = true;
        }
        if delete && !ed.picked.is_empty() {
            for id in std::mem::take(&mut ed.picked) {
                remove_node(&mut patch, id);
            }
            discrete = true;
        }
        if let Some((k, at)) = add_at {
            let id = add_node(&mut patch, k, [at.x, at.y]);
            ed.pick_only(id);
            discrete = true;
        }

        // Node Wrangler's right drags: Shift cuts cables, Ctrl or Alt connects modules
        let (sec_pressed, sec_released) = ui.input(|i| {
            (
                i.pointer.button_pressed(PointerButton::Secondary),
                i.pointer.button_released(PointerButton::Secondary),
            )
        });
        let node_at = |p: &Patch, q: Pos2| {
            p.nodes
                .iter()
                .rev()
                .find(|n| node_rect(n, origin, z).contains(q))
                .map(|n| n.id)
        };
        if sec_pressed && over_canvas && !swallow {
            if let Some(q) = pointer {
                if fine {
                    ed.rgest = Some(RGesture::Knife(q));
                } else if ctrl || alt {
                    ed.rgest = node_at(&patch, q).map(|id| RGesture::Lazy(id, q));
                }
            }
        }
        if let (Some(g), Some(q)) = (ed.rgest, pointer) {
            let (from, col) = match g {
                RGesture::Knife(a) => (a, Color32::from_rgb(230, 80, 80)),
                RGesture::Lazy(_, a) => (a, p.accent_strong),
            };
            painter.line_segment([from, q], Stroke::new(2.0, col));
            if sec_released {
                match g {
                    RGesture::Knife(a) => {
                        if cut_cables(&mut patch, origin, z, a, q) > 0 {
                            ed.selected = None;
                            discrete = true;
                        }
                    }
                    RGesture::Lazy(a, _) => {
                        if let Some(b) = node_at(&patch, q) {
                            if lazy_connect(&mut patch, a, b) {
                                discrete = true;
                            }
                        }
                    }
                }
                ed.rgest = None;
                // the right button let go after a drag opens no menu
                egui::Popup::close_all(ui.ctx());
            }
            ui.ctx().request_repaint();
        }

        // frames under everything: a strip with the label, dragged to move its modules
        let mut frame_move: Option<(usize, Vec2)> = None;
        let mut frame_remove: Option<usize> = None;
        let mut frame_rename: Option<(usize, String)> = None;
        for (fi, f) in patch.frames.iter().enumerate() {
            let Some(r) = frame_rect(&patch, f, origin, z) else {
                continue;
            };
            painter.rect_filled(r, 8.0 * z, p.accent_strong.gamma_multiply(0.07));
            painter.rect_stroke(
                r,
                8.0 * z,
                Stroke::new(1.0, p.accent_strong.gamma_multiply(0.5)),
                egui::StrokeKind::Inside,
            );
            let strip = Rect::from_min_size(r.min, vec2(r.width(), FRAME_H * z));
            if z >= 0.45 {
                painter.text(
                    strip.left_center() + vec2(8.0 * z, 0.0),
                    egui::Align2::LEFT_CENTER,
                    if f.label.is_empty() {
                        "Frame"
                    } else {
                        &f.label
                    },
                    egui::FontId::proportional(12.0 * z),
                    p.text,
                );
            }
            let fr = ui.interact(
                strip,
                ui.scope_id().with(("frame", fi)),
                sense(Sense::click_and_drag()),
            );
            if fr.drag_started_by(PointerButton::Primary) {
                ed.frame_drag = Some(fi);
            }
            if fr.dragged_by(PointerButton::Primary) && ed.frame_drag == Some(fi) {
                frame_move = Some((fi, fr.drag_delta() / z));
            }
            if fr.drag_stopped() {
                ed.frame_drag = None;
            }
            let mut label = f.label.clone();
            fr.on_hover_text(
                "A frame: drag its label to move its modules, right click to rename or remove it",
            )
            .context_menu(|ui| {
                ui.label("Label");
                if ui.text_edit_singleline(&mut label).changed() {
                    frame_rename = Some((fi, label.clone()));
                }
                if ui.button("Remove frame").clicked() {
                    frame_remove = Some(fi);
                    ui.close();
                }
            });
        }
        if let Some((fi, d)) = frame_move {
            let ids = patch.frames[fi].nodes.clone();
            for n in patch.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.pos[0] += d.x;
                n.pos[1] += d.y;
            }
        }
        if !down {
            ed.frame_drag = None;
        }
        if let Some((fi, l)) = frame_rename {
            patch.frames[fi].label = l;
        }
        if let Some(fi) = frame_remove {
            patch.frames.remove(fi);
            discrete = true;
        }

        // a sample from the playlist's picker (an audio clip's) let go on the canvas
        let pick_released = ui.input(|i| i.pointer.any_released())
            && pointer.is_some_and(|q| canvas.contains(q))
            && egui::DragAndDrop::has_payload_of_type::<crate::playlist::PickDrag>(ui.ctx());
        let mut dropped_sample: Option<String> = None;
        if pick_released {
            if let Some(d) = egui::DragAndDrop::take_payload::<crate::playlist::PickDrag>(ui.ctx())
            {
                if let crate::playlist::Source::Audio(ch_id) = &d.source {
                    dropped_sample = clip_samples
                        .iter()
                        .find(|(c, _)| c == ch_id)
                        .and_then(|(_, s)| s.clone());
                }
            }
        }
        let dragging_sound =
            egui::DragAndDrop::has_payload_of_type::<crate::playlist::PickDrag>(ui.ctx())
                || egui::DragAndDrop::has_payload_of_type::<crate::shell::Dragged>(ui.ctx())
                || ui.input(|i| !i.raw.hovered_files.is_empty());
        if dragging_sound && pointer.is_some_and(|q| canvas.contains(q)) {
            painter.rect_stroke(
                canvas.shrink(2.0),
                4.0,
                Stroke::new(2.0, p.accent_strong),
                egui::StrokeKind::Inside,
            );
            ui.ctx().request_repaint();
        }
        if let (Some(id), Some(q)) = (dropped_sample, pointer) {
            let hit = node_at(&patch, q);
            ed.picked = vec![place_sample(&mut patch, &id, world(q), hit)];
            discrete = true;
        }

        let font = |size: f32| egui::FontId::proportional(size * z);
        let text_col = skin.map_or(p.text, |s| s.text);
        // text this small is not drawn
        let words = z >= 0.45;

        // the cable a moved module would go into
        ed.insert_on = match (ed.moving, pointer) {
            (Some(id), Some(q)) if insertable(&patch, id) && !alt => {
                cable_near(&patch, origin, z, q, id)
            }
            _ => None,
        };

        // cables under the modules
        let mut picked = None;
        let mut reroute: Option<(CableKey, Pos2)> = None;
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
            let lit_insert = ed.insert_on.as_ref() == Some(&key);
            let m = mid(pa, pb, z);
            let handle = Rect::from_center_size(m, port_hit(z) * 0.9);
            let r = ui.interact(
                handle,
                ui.scope_id().with(("cable", &key)),
                sense(Sense::click()),
            );
            let col = if selected || r.hovered() || lit_insert {
                p.accent_strong
            } else {
                kind_color(a.kind).gamma_multiply(0.4 + 0.6 * c.amount.abs().min(1.0) as f32)
            };
            let width = if lit_insert {
                4.5
            } else if selected || r.hovered() {
                3.0
            } else {
                2.0
            };
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                bezier(pa, pb, z),
                false,
                Color32::TRANSPARENT,
                Stroke::new(width, col),
            ));
            painter.circle_filled(m, if r.hovered() { 6.0 } else { 4.0 }, col);
            if r.double_clicked() {
                pull = Some(ci);
            } else if r.clicked() && fine {
                reroute = Some((key.clone(), world(m)));
            } else if r.clicked() {
                picked = Some(key.clone());
            }
            let mut amt = c.amount;
            r.on_hover_text(format!(
                "{} {} ➡ {} {} ({:.2})\nClick to select (Delete pulls it out), double click to pull out, Shift click for a reroute dot, right click for more",
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
                if ui.button("Add reroute").on_hover_text("A dot on the cable, to lead it round (Shift click)").clicked() {
                    reroute = Some((key.clone(), world(m)));
                    ui.close();
                }
                if ui.button("Pull out cable").clicked() {
                    pull = Some(ci);
                    ui.close();
                }
            });
        }
        if let Some((k, at)) = reroute {
            if let Some(id) = add_reroute(&mut patch, &k, at) {
                ed.pick_only(id);
                discrete = true;
            }
        }
        if let Some(k) = picked {
            ed.selected = Some(k);
            ed.picked.clear();
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
        let mut remove: Option<u32> = None;
        let mut dup_one: Option<u32> = None;
        let mut copy_one: Option<u32> = None;
        let mut dissolve_one: Option<u32> = None;
        let mut view_one: Option<u32> = None;
        let mut set_sample: Option<(u32, String)> = None;
        let viewing = ed.viewer.as_ref().map(|v| v.0);
        let mut unplug: Option<(u32, usize)> = None;
        let mut automate: Option<(u32, String, f64)> = None;
        let mut drop_on: Option<(u32, usize)> = None;
        let mut bubble: Option<(Pos2, String)> = None;
        let mut move_by: Option<Vec2> = None;
        let mut moved_stop: Option<u32> = None;
        let mut start_move: Option<u32> = None;
        let mut clicked_node: Option<u32> = None;
        let picked_now = ed.picked.clone();
        for (ni, n) in patch.nodes.iter_mut().enumerate() {
            let rect = node_rect(n, origin, z);
            if !canvas.intersects(rect) {
                continue;
            }
            let spec = n.kind.spec();
            let kc = kind_color(n.kind);
            let picked_node = picked_now.contains(&n.id);
            let id = n.id;
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
            let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_H * z));
            painter.rect_filled(title, 6.0 * z, kc.gamma_multiply(0.35));
            let heading = title_text(n);
            let kind = n.kind;
            if viewing == Some(id) {
                painter.text(
                    rect.center_top() - vec2(0.0, 2.0 * z),
                    egui::Align2::CENTER_BOTTOM,
                    "HEARD ALONE",
                    egui::FontId::proportional(10.0 * z.max(0.6)),
                    p.accent_strong,
                );
            }
            if words {
                // the mode at the right of the title: a wave drawn, or a short mark
                let mut right = title.max.x - 8.0 * z;
                let wave = mode_param(n.kind)
                    .filter(|m| *m == "wave")
                    .and_then(|m| n.kind.choices(m)?.get(n.param(m) as usize).copied());
                if let Some(w) = wave {
                    let r = Rect::from_min_max(
                        pos2(right - 20.0 * z, title.center().y - 5.0 * z),
                        pos2(right, title.center().y + 5.0 * z),
                    );
                    draw_wave(&painter, r, w, Stroke::new(1.5 * z.max(0.7), text_col));
                    right = r.min.x - 6.0 * z;
                } else if let Some(b) = mode_badge(n) {
                    let g = painter.layout_no_wrap(b, egui::FontId::monospace(13.0 * z), text_col);
                    painter.galley(
                        pos2(right - g.size().x, title.center().y - g.size().y * 0.5),
                        g.clone(),
                        text_col,
                    );
                    right -= g.size().x + 6.0 * z;
                }
                let g = painter.layout_no_wrap(heading.clone(), font(13.0), text_col);
                let left = title.left_center() + vec2(8.0 * z, 0.0);
                // a long title is shortened rather than running into the mark
                let g = if left.x + g.size().x > right {
                    painter.layout_no_wrap(kind_name(n.kind).to_string(), font(13.0), text_col)
                } else {
                    g
                };
                painter.galley(left - vec2(0.0, g.size().y * 0.5), g, text_col);
            }
            // the whole module takes a drag (rows on it take their own)
            let t = ui.interact(
                rect,
                ui.scope_id().with(("node", id)),
                sense(Sense::click_and_drag()),
            );
            if t.drag_started_by(PointerButton::Primary) {
                start_move = Some(id);
            }
            if t.clicked() {
                clicked_node = Some(id);
            }
            if t.dragged_by(PointerButton::Primary) && ed.moving == Some(id) {
                move_by = Some(t.drag_delta() / z);
            }
            if t.drag_stopped() && ed.moving == Some(id) {
                moved_stop = Some(id);
            }
            if t.hovered() || t.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            let many = picked_now.len() > 1 && picked_node;
            t.on_hover_text(format!(
                "{heading} ({})\nDrag to move (Alt and a drag pulls it out of its chain), drop it on a cable to put it in; Shift or Ctrl click to pick more, right click for more",
                CATEGORIES[n.kind.category()]
            ))
            .context_menu(|ui| {
                if ui
                    .add(egui::Button::new(if many { "Copy modules" } else { "Copy module" }).shortcut_text("Ctrl+C"))
                    .clicked()
                {
                    copy_one = Some(id);
                    ui.close();
                }
                if ui
                    .add(egui::Button::new(if many { "Duplicate modules" } else { "Duplicate module" }).shortcut_text("Ctrl+D"))
                    .clicked()
                {
                    dup_one = Some(id);
                    ui.close();
                }
                if ui.button("Pull out of chain").clicked() {
                    dissolve_one = Some(id);
                    ui.close();
                }
                if ui
                    .add(egui::Button::new(if viewing == Some(id) { "Stop hearing alone" } else { "Hear alone" }).shortcut_text("Ctrl+Shift+click"))
                    .on_hover_text("Its output goes straight to the output, in place of what is there, until you do it again")
                    .clicked()
                {
                    view_one = Some(id);
                    ui.close();
                }
                if kind == Kind::Sampler {
                    ui.menu_button("Sample", |ui| {
                        if sample_list.is_empty() {
                            ui.label("The project has no samples yet: drop one on the canvas");
                        }
                        for (sid, sname) in &sample_list {
                            if ui.button(sname).clicked() {
                                set_sample = Some((id, sid.clone()));
                                ui.close();
                            }
                        }
                    });
                }
                if ui
                    .add(egui::Button::new(if many { "Remove modules" } else { "Remove module" }).shortcut_text("Del"))
                    .clicked()
                {
                    remove = Some(id);
                    ui.close();
                }
            });

            // a sampler's waveform, with its start and end; a click picks its sample
            if kind == Kind::Sampler {
                let top = rect.min.y + (TITLE_H + kind.rows() as f32 * ROW_H + 2.0) * z;
                let wr = Rect::from_min_max(
                    pos2(rect.min.x + 10.0 * z, top),
                    pos2(rect.max.x - 10.0 * z, top + WAVE_H * z),
                );
                painter.rect_filled(wr, 3.0 * z, skin.map_or(p.well_deep, |s| s.track));
                let peaks = n.sample.as_ref().and_then(|s| ed.peaks.get(s));
                match peaks.filter(|pk| !pk.is_empty()) {
                    Some(pk) => {
                        let w = wr.width() / pk.len() as f32;
                        for (ci, (lo, hi)) in pk.iter().enumerate() {
                            let x = wr.min.x + (ci as f32 + 0.5) * w;
                            let cy = wr.center().y;
                            let h = wr.height() * 0.5;
                            painter.line_segment(
                                [
                                    pos2(x, cy - hi.clamp(-1.0, 1.0) * h),
                                    pos2(x, cy - lo.clamp(-1.0, 1.0) * h - 0.5),
                                ],
                                Stroke::new(w.max(1.0), kc.gamma_multiply(0.8)),
                            );
                        }
                        for (pn, col) in [("start", p.accent_strong), ("end", p.accent_strong)] {
                            let x = wr.min.x + wr.width() * n.param(pn) as f32;
                            painter.line_segment(
                                [pos2(x, wr.min.y), pos2(x, wr.max.y)],
                                Stroke::new(1.5, col),
                            );
                        }
                    }
                    None if words => {
                        painter.text(
                            wr.center(),
                            egui::Align2::CENTER_CENTER,
                            "Drop a sample here, or click",
                            font(10.5),
                            skin.map_or(p.text2, |s| s.dim),
                        );
                    }
                    None => {}
                }
                let sname = n
                    .sample
                    .as_ref()
                    .and_then(|s| sample_list.iter().find(|(i, _)| i == s))
                    .map(|(_, nm)| nm.clone());
                if let (Some(nm), true) = (&sname, words) {
                    painter.text(
                        wr.left_top() + vec2(3.0 * z, 1.0 * z),
                        egui::Align2::LEFT_TOP,
                        nm,
                        font(9.5),
                        text_col,
                    );
                }
                let wresp =
                    ui.interact(wr, ui.scope_id().with(("wave", id)), sense(Sense::click()));
                egui::Popup::menu(&wresp)
                    .id(wresp.id.with("samples"))
                    .show(|ui| {
                        if sample_list.is_empty() {
                            ui.label("The project has no samples yet: drop one here");
                        }
                        for (sid, nm) in &sample_list {
                            if ui
                                .selectable_label(n.sample.as_deref() == Some(sid), nm)
                                .clicked()
                            {
                                set_sample = Some((id, sid.clone()));
                                ui.close();
                            }
                        }
                    });
                wresp.on_hover_text(format!(
                    "{}\nClick to pick a sample of the project, or drop one from the browser, your files or the playlist",
                    sname.as_deref().unwrap_or("No sample yet")
                ));
            }
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
                    Rect::from_center_size(at, port_hit(z)),
                    ui.scope_id().with(("out", id, o)),
                    sense(Sense::drag()),
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
                r.on_hover_text(port_tip(n.kind, oname, true));
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
                let iname = input_name(n.kind, k);
                // the dot: its tooltip, and from a plugged input a drag takes its cable
                // out to plug it in elsewhere
                let pr = ui.interact(
                    Rect::from_center_size(at, port_hit(z)),
                    ui.scope_id().with(("in", id, k)),
                    sense(if plugged {
                        Sense::drag()
                    } else {
                        Sense::hover()
                    }),
                );
                if plugged && pr.drag_started() {
                    unplug = Some((id, k));
                }
                let tip = port_tip(n.kind, iname, false);
                pr.on_hover_text(if plugged {
                    format!("{tip}\nDrag to take this cable out and plug it in elsewhere")
                } else {
                    tip.clone()
                });
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
                    sense(Sense::click_and_drag()),
                );
                let choices = n.kind.choices(ps.name);
                let turning = ed
                    .knob
                    .as_ref()
                    .is_some_and(|kn| kn.node == id && kn.param == pi);
                let why = idle[ni][pi];
                // the bar: how far along its range the value is
                let at_n = ps.normalize(v) as f32;
                painter.rect_filled(row, 3.0 * z, skin.map_or(p.well_deep, |s| s.track));
                let lit = if turning {
                    0.95
                } else if r.hovered() {
                    0.75
                } else if why.is_some() {
                    0.25
                } else {
                    0.5
                };
                let arrow = ARROW_W * z;
                if choices.is_some() {
                    // a choice is a box with arrows, not a bar
                    painter.rect_stroke(
                        row,
                        3.0 * z,
                        Stroke::new(1.0, kc.gamma_multiply(lit)),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    let fill = Rect::from_min_max(
                        row.min,
                        pos2(row.min.x + row.width() * at_n, row.max.y),
                    );
                    painter.rect_filled(fill, 3.0 * z, kc.gamma_multiply(lit));
                }
                let shown = value_text(n.kind, ps, v);
                if words {
                    let tc = if why.is_some() { label } else { text_col };
                    let pad = 4.0 * z;
                    let (lo, hi) = if choices.is_some() {
                        for (x, s) in [
                            (row.min.x + arrow * 0.5, "◀"),
                            (row.max.x - arrow * 0.5, "▶"),
                        ] {
                            painter.text(
                                pos2(x, row.center().y),
                                egui::Align2::CENTER_CENTER,
                                s,
                                font(8.0),
                                kc,
                            );
                        }
                        (row.min.x + arrow, row.max.x - arrow)
                    } else {
                        (row.min.x, row.max.x)
                    };
                    let val = painter.layout_no_wrap(shown.clone(), font(10.5), tc);
                    let nm = painter.layout_no_wrap(ps.name.replace('_', " "), font(10.5), tc);
                    painter.galley(
                        pos2(hi - pad - val.size().x, row.center().y - val.size().y * 0.5),
                        val.clone(),
                        tc,
                    );
                    // the name only where it does not run into the value
                    if nm.size().x + val.size().x + 3.0 * pad <= hi - lo {
                        painter.galley(pos2(lo + pad, row.center().y - nm.size().y * 0.5), nm, tc);
                    }
                }
                let mut changed = false;
                // a drag turns any control (a choice steps through its choices)
                if r.drag_started_by(PointerButton::Primary) {
                    ed.knob = Some(Knob {
                        node: id,
                        param: pi,
                        at: ps.normalize(v),
                    });
                }
                if r.dragged_by(PointerButton::Primary) {
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
                if let Some(choices) = choices {
                    // a click on an arrow steps; elsewhere it opens the list
                    let popup = r.id.with("choices");
                    let mut open = false;
                    if r.clicked() {
                        let x = r.interact_pointer_pos().map_or(row.center().x, |q| q.x);
                        if x < row.min.x + arrow {
                            v = step_choice(v, -1, choices.len());
                            changed = true;
                        } else if x > row.max.x - arrow {
                            v = step_choice(v, 1, choices.len());
                            changed = true;
                        } else {
                            open = true;
                        }
                    }
                    egui::Popup::menu(&r)
                        .id(popup)
                        .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                        .width(row.width().max(120.0))
                        .show(|ui| {
                            for (ci, nm) in choices.iter().enumerate() {
                                if ui.selectable_label(v as usize == ci, *nm).clicked() {
                                    v = ci as f64;
                                    changed = true;
                                    ui.close();
                                }
                            }
                        });
                } else if r.double_clicked() || (r.clicked() && ctrl) {
                    v = ps.default;
                    changed = true;
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
                let how = if choices.is_some() {
                    "Click to choose from the list, the arrows or the wheel to step, right click to automate or link to a MIDI knob"
                } else {
                    "Drag up or down to set (shift for fine), wheel to nudge, double click to reset, right click to automate or link to a MIDI knob"
                };
                let note = why
                    .map(|w| format!("\nNot heard now: {w}"))
                    .unwrap_or_default();
                r.on_hover_text(format!(
                    "{}: {}\n{range}, default {}{note}\n{how}\nA cable can move it too: {}",
                    ps.name,
                    shown,
                    value_text(n.kind, ps, ps.default),
                    "drop one on this row (several add up)"
                ))
                .context_menu(|ui| {
                    if ui.button("Create automation clip").clicked() {
                        automate = Some((pid, alabel.clone(), at));
                        ui.close();
                    }
                    crate::midilearn::menu_item(ui, || crate::auto::AutoRequest {
                        target: crate::auto::plugin_target(&channel_id, &format!("p.{pid}")),
                        name: format!("{name} {alabel}"),
                        start: at,
                    });
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
        // a click on a module picks it (Shift or Ctrl add it to the picked ones)
        if let Some(id) = clicked_node.filter(|_| ctrl && fine) {
            view_one = Some(id);
        } else if let Some(id) = clicked_node {
            if add_key {
                ed.pick_toggle(id);
            } else {
                ed.pick_only(id);
            }
        }
        // a drag of a module moves it with the others picked; Alt pulls it out first
        if let Some(id) = start_move {
            if !ed.picked.contains(&id) {
                if add_key {
                    ed.picked.push(id);
                } else {
                    ed.pick_only(id);
                }
            }
            ed.selected = None;
            ed.moving = Some(id);
            ed.knob = None;
            if alt {
                dissolve(&mut patch, id);
                discrete = true;
            }
        }
        if let Some(d) = move_by {
            let ids = ed.picked.clone();
            for n in patch.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.pos[0] += d.x;
                n.pos[1] += d.y;
            }
        }
        if let Some(id) = moved_stop {
            if let Some(key) = ed.insert_on.take() {
                if ed.picked.len() <= 1 && insert_into_cable(&mut patch, id, &key) {
                    discrete = true;
                }
            }
            ed.moving = None;
        }
        if !down && ed.moving.is_some() && moved_stop.is_none() {
            ed.moving = None;
            ed.insert_on = None;
        }
        // the module menu works on the picked ones when the module is one of them
        let group = |ed: &WeaveEditor, id: u32| {
            if ed.picked.contains(&id) {
                ed.picked.clone()
            } else {
                vec![id]
            }
        };
        if let Some(id) = copy_one {
            if let Some(clip) = copy_nodes(&patch, &group(ed, id)) {
                ed.clip = Some(clip);
            }
        }
        if let Some(id) = dup_one {
            let ids = group(ed, id);
            ed.picked = duplicate_nodes(&mut patch, &ids);
            discrete = true;
        }
        if let Some(id) = view_one {
            match ed.viewer.take() {
                // the same module again: back as it was
                Some((v, old)) if v == id => unview(&mut patch, old),
                other => {
                    if let Some((_, old)) = other {
                        unview(&mut patch, old);
                    }
                    ed.viewer = view_node(&mut patch, id).map(|old| (id, old));
                }
            }
            discrete = true;
        }
        if let Some((id, smp)) = set_sample {
            place_sample(&mut patch, &smp, Pos2::ZERO, Some(id));
            discrete = true;
        }
        if let Some(id) = dissolve_one {
            dissolve(&mut patch, id);
            discrete = true;
        }
        if let Some(id) = remove {
            for id in group(ed, id) {
                remove_node(&mut patch, id);
            }
            ed.selected = None;
            ed.picked.clear();
            discrete = true;
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

        // the cable being drawn, over the modules
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
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
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
                    !self.weave.gesture
                } else {
                    now - self.weave.last_edit > 1.0
                };
            self.weave.gesture = down;
            self.weave.last_edit = now;
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
    use las_dsp::modular::MATH_OPS;

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
    fn a_duplicate_keeps_the_parameters_and_the_cables_between_the_copies() {
        let mut p = Patch::default();
        let cables = p.cables.len();
        // the filter alone: no cable between copies
        let ids = duplicate_nodes(&mut p, &[4]);
        let (a, b) = (p.node(4).unwrap(), p.node(ids[0]).unwrap());
        assert_eq!((a.kind, &a.params), (b.kind, &b.params));
        assert_ne!(a.pos, b.pos);
        assert_eq!(p.cables.len(), cables);
        assert!(duplicate_nodes(&mut p, &[99]).is_empty());
        // the oscillator and the filter: the cable between them comes too
        let ids = duplicate_nodes(&mut p, &[2, 4]);
        assert_eq!(ids.len(), 2);
        assert_eq!(p.cables.len(), cables + 1);
        let c = p.cables.last().unwrap();
        assert_eq!((c.from, c.to), (ids[0], ids[1]));
    }

    #[test]
    fn paste_puts_the_copy_at_the_place_given() {
        let mut p = Patch::default();
        let clip = copy_nodes(&p, &[2, 3, 4]).unwrap();
        assert_eq!(clip.nodes.len(), 3);
        // osc to filter and env to filter cutoff
        assert_eq!(clip.cables.len(), 2);
        let ids = paste_nodes(&mut p, &clip, [1000.0, 500.0]);
        let min_x = ids
            .iter()
            .map(|id| p.node(*id).unwrap().pos[0])
            .fold(f32::MAX, f32::min);
        let min_y = ids
            .iter()
            .map(|id| p.node(*id).unwrap().pos[1])
            .fold(f32::MAX, f32::min);
        assert_eq!((min_x, min_y), (1000.0, 500.0));
        assert!(copy_nodes(&p, &[]).is_none());
    }

    #[test]
    fn a_single_input_takes_one_cable_and_a_new_one_replaces_it() {
        let mut p = Patch::default();
        let osc = add_node(&mut p, Kind::Osc, [0.0, 0.0]);
        // the filter's input had the first oscillator
        assert!(connect(&mut p, osc, "out", 4, "in"));
        let into: Vec<u32> = p
            .cables
            .iter()
            .filter(|c| c.to == 4 && c.input == "in")
            .map(|c| c.from)
            .collect();
        assert_eq!(into, vec![osc]);
        // the output's sides add up what comes in
        assert!(connect(&mut p, osc, "out", 6, "left"));
        assert_eq!(
            p.cables
                .iter()
                .filter(|c| c.to == 6 && c.input == "left")
                .count(),
            2
        );
    }

    #[test]
    fn a_module_goes_into_a_cable_and_comes_out_again() {
        let mut p = Patch::default();
        let n = p.cables.len();
        let fold = add_node(&mut p, Kind::Shaper, [0.0, 0.0]);
        let key = cable_key(p.cables.iter().find(|c| c.from == 4 && c.to == 5).unwrap());
        assert!(insert_into_cable(&mut p, fold, &key));
        assert!(p
            .cables
            .iter()
            .any(|c| c.from == 4 && c.to == fold && c.input == "in"));
        assert!(p
            .cables
            .iter()
            .any(|c| c.from == fold && c.to == 5 && c.input == "in"));
        assert_eq!(p.cables.len(), n + 1);
        // not twice: it has cables now
        let other = cable_key(&p.cables[0]);
        assert!(!insert_into_cable(&mut p, fold, &other));
        // pulled out, its neighbours are joined again
        dissolve(&mut p, fold);
        assert!(p.cables.iter().all(|c| c.from != fold && c.to != fold));
        assert!(p
            .cables
            .iter()
            .any(|c| c.from == 4 && c.to == 5 && c.input == "in"));
        assert_eq!(p.cables.len(), n);
        // a module with no input (keys) cannot go in
        let keys = add_node(&mut p, Kind::Keys, [0.0, 0.0]);
        let key = cable_key(&p.cables[0]);
        assert!(!insert_into_cable(&mut p, keys, &key));
    }

    #[test]
    fn titles_say_the_mode() {
        let math = Node::new(1, Kind::Math, [0.0; 2]).with("op", 2.0);
        assert_eq!(title_text(&math), "Math: Multiply");
        assert_eq!(mode_badge(&math).as_deref(), Some("\u{d7}"));
        let f = Node::new(1, Kind::Filter, [0.0; 2]).with("mode", 2.0);
        assert_eq!(title_text(&f), "Filter: Highpass");
        let o = Node::new(1, Kind::Osc, [0.0; 2]);
        assert_eq!(title_text(&o), "Oscillator: Saw");
        assert_eq!(title_text(&Node::new(1, Kind::Vca, [0.0; 2])), "VCA");
        // every op has a name and a symbol
        assert_eq!(MATH_OPS.len(), MATH_SYMBOLS.len());
    }

    #[test]
    fn choices_step_round_and_search_finds_modules() {
        assert_eq!(step_choice(0.0, -1, 5), 4.0);
        assert_eq!(step_choice(4.0, 1, 5), 0.0);
        assert_eq!(step_choice(2.0, 1, 5), 3.0);
        assert_eq!(search_kinds("filt"), vec![Kind::Filter, Kind::Ladder]);
        assert_eq!(search_kinds("").len(), Kind::ALL.len());
        assert!(search_kinds("effects").contains(&Kind::Reverb));
        // every port says what it is for
        for k in Kind::ALL {
            let s = k.spec();
            for o in s.outputs {
                assert!(port_tip(k, o, true).len() > 10);
            }
            for i in 0..s.inputs.len() {
                assert!(port_tip(k, input_name(k, i), false).contains("cable"));
            }
        }
    }

    #[test]
    fn a_knife_cuts_the_cables_it_crosses() {
        let mut p = Patch::default();
        let ci = p
            .cables
            .iter()
            .position(|c| c.from == 2 && c.to == 4)
            .unwrap();
        let c = &p.cables[ci];
        let (a, b) = (p.node(c.from).unwrap(), p.node(c.to).unwrap());
        let pa = out_pos(a, Pos2::ZERO, 0, 1.0);
        let pb = in_pos(b, Pos2::ZERO, b.kind.input_index(&c.input).unwrap(), 1.0);
        let m = mid(pa, pb, 1.0);
        let n = p.cables.len();
        assert_eq!(
            cut_cables(
                &mut p,
                Pos2::ZERO,
                1.0,
                m - vec2(0.0, 30.0),
                m + vec2(0.0, 30.0)
            ),
            1,
            "{pa:?} {pb:?} {m:?}"
        );
        assert_eq!(p.cables.len(), n - 1);
    }

    #[test]
    fn a_cable_is_found_near_its_curve() {
        let p = Patch::default();
        let c = &p.cables[2];
        let (a, b) = (p.node(c.from).unwrap(), p.node(c.to).unwrap());
        let pa = out_pos(a, Pos2::ZERO, 0, 1.0);
        let pb = in_pos(b, Pos2::ZERO, b.kind.input_index(&c.input).unwrap(), 1.0);
        assert!(cable_dist(pa, pb, 1.0, mid(pa, pb, 1.0)) < 0.5);
        assert!(cable_dist(pa, pb, 1.0, mid(pa, pb, 1.0) + vec2(0.0, 100.0)) > 50.0);
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
