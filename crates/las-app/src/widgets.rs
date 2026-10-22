// SPDX-License-Identifier: GPL-3.0-or-later
//! Hand-drawn controls: knob, level meter and piano keyboard.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui,
};
use std::f32::consts::PI;
use std::ops::RangeInclusive;

fn to_unit(v: f64, r: &RangeInclusive<f64>, log: bool) -> f64 {
    let (lo, hi) = (*r.start(), *r.end());
    let t = if log {
        (v.max(lo).ln() - lo.ln()) / (hi.ln() - lo.ln())
    } else {
        (v - lo) / (hi - lo)
    };
    t.clamp(0.0, 1.0)
}

fn from_unit(t: f64, r: &RangeInclusive<f64>, log: bool) -> f64 {
    let (lo, hi) = (*r.start(), *r.end());
    let t = t.clamp(0.0, 1.0);
    if log {
        (lo.ln() + t * (hi.ln() - lo.ln())).exp()
    } else {
        lo + t * (hi - lo)
    }
}

thread_local! {
    static KNOB_SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    /// What the next knob drawn automates, if anything: its right click menu then
    /// offers "Create automation clip".
    static AUTOMATE_NEXT: std::cell::RefCell<Option<crate::auto::AutoRequest>> =
        const { std::cell::RefCell::new(None) };
    /// The automation clip a knob's menu asked for, until the app makes it.
    static AUTOMATE_ASKED: std::cell::RefCell<Option<crate::auto::AutoRequest>> =
        const { std::cell::RefCell::new(None) };
}

/// Let the next knob drawn offer to automate `req` on a right click.
pub fn automate_next(req: crate::auto::AutoRequest) {
    AUTOMATE_NEXT.with(|c| *c.borrow_mut() = Some(req));
}

/// The automation clip a knob's menu asked for since the last call.
pub fn take_automation_request() -> Option<crate::auto::AutoRequest> {
    AUTOMATE_ASKED.with(|c| c.borrow_mut().take())
}

/// Draw the knobs that `add` makes at `scale` times their usual size.
pub fn knob_scale<R>(scale: f32, add: impl FnOnce() -> R) -> R {
    let before = KNOB_SCALE.with(|c| c.replace(scale));
    let r = add();
    KNOB_SCALE.with(|c| c.set(before));
    r
}

/// A rotary knob. Drag up and down to change it (hold Shift for fine steps),
/// double-click to reset. Returns true when the value changed.
pub fn knob(
    ui: &mut Ui,
    value: &mut f64,
    range: RangeInclusive<f64>,
    default: f64,
    label: &str,
    log: bool,
    show: &dyn Fn(f64) -> String,
) -> bool {
    knob_mapped(
        ui,
        value,
        &|v| to_unit(v, &range, log),
        &|t| from_unit(t, &range, log),
        default,
        label,
        show,
    )
}

/// A knob whose travel is given by two functions: value to position (0 to 1)
/// and back. For curves other than linear and logarithmic.
pub fn knob_mapped(
    ui: &mut Ui,
    value: &mut f64,
    to_norm: &dyn Fn(f64) -> f64,
    from_norm: &dyn Fn(f64) -> f64,
    default: f64,
    label: &str,
    show: &dyn Fn(f64) -> String,
) -> bool {
    knob_scaled(ui, value, to_norm, from_norm, default, label, show, 1.0)
}

/// A larger knob, for the main controls of a plugin face.
pub fn big_knob(
    ui: &mut Ui,
    value: &mut f64,
    range: RangeInclusive<f64>,
    default: f64,
    label: &str,
    show: &dyn Fn(f64) -> String,
) -> bool {
    knob_scaled(
        ui,
        value,
        &|v| to_unit(v, &range, false),
        &|t| from_unit(t, &range, false),
        default,
        label,
        show,
        1.45,
    )
}

#[allow(clippy::too_many_arguments)]
fn knob_scaled(
    ui: &mut Ui,
    value: &mut f64,
    to_norm: &dyn Fn(f64) -> f64,
    from_norm: &dyn Fn(f64) -> f64,
    default: f64,
    label: &str,
    show: &dyn Fn(f64) -> String,
    k: f32,
) -> bool {
    let skin = crate::skin::current();
    // the knob's own size, times the size of the knobs of the panel drawing it
    let k = k * KNOB_SCALE.with(std::cell::Cell::get);
    let size = vec2(54.0 * k.max(0.8), 30.0 + 46.0 * k);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let mut changed = false;
    if resp.double_clicked() {
        *value = default;
        changed = true;
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let step = if fine { 1.0 / 1200.0 } else { 1.0 / 180.0 };
        let t = to_norm(*value) - resp.drag_delta().y as f64 * step;
        *value = from_norm(t.clamp(0.0, 1.0));
        changed = true;
    }
    let painter = ui.painter_at(rect);
    let centre = pos2(rect.center().x, rect.top() + 24.0 * k);
    let radius = 18.0 * k;
    let (body, track, accent, text) = match skin {
        Some(s) => (s.knob, s.track, s.accent, s.dim),
        None => (
            crate::theme::pal().well_deep,
            crate::theme::pal().raised2,
            crate::theme::accent(),
            crate::theme::pal().text2,
        ),
    };
    if skin.is_some_and(|s| !s.flat) {
        // a raised cap with a soft shadow, like a hardware knob
        painter.circle_filled(
            centre + vec2(0.0, 2.0),
            radius + 3.0,
            Color32::from_black_alpha(90),
        );
        painter.circle_filled(centre, radius - 3.0, body);
        painter.circle_stroke(
            centre,
            radius - 3.0,
            Stroke::new(1.0, Color32::from_white_alpha(28)),
        );
    } else {
        painter.circle_filled(centre, radius + 3.0, body);
    }
    let (a0, sweep) = (PI * 0.75, PI * 1.5);
    let arc = |t0: f32, t1: f32, r: f32| -> Vec<Pos2> {
        let steps = 28;
        (0..=steps)
            .map(|i| {
                let a = a0 + sweep * (t0 + (t1 - t0) * i as f32 / steps as f32);
                pos2(centre.x + r * a.cos(), centre.y + r * a.sin())
            })
            .collect()
    };
    painter.add(Shape::line(arc(0.0, 1.0, radius), Stroke::new(3.0, track)));
    let t = to_norm(*value).clamp(0.0, 1.0) as f32;
    if t > 0.002 {
        let lit = if resp.hovered() || resp.dragged() {
            accent.gamma_multiply(1.3)
        } else {
            accent
        };
        if skin.is_some_and(|s| !s.flat) {
            // the lit arc glows on a plugin face
            painter.add(Shape::line(
                arc(0.0, t, radius),
                Stroke::new(7.0, lit.gamma_multiply(0.2)),
            ));
        }
        painter.add(Shape::line(arc(0.0, t, radius), Stroke::new(3.0, lit)));
    }
    let a = a0 + sweep * t;
    painter.line_segment(
        [
            pos2(centre.x + 6.0 * k * a.cos(), centre.y + 6.0 * k * a.sin()),
            pos2(
                centre.x + (radius - 3.0) * a.cos(),
                centre.y + (radius - 3.0) * a.sin(),
            ),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 49.0 * k + (1.0 - k) * 4.0),
        Align2::CENTER_CENTER,
        show(*value),
        FontId::proportional(11.0 * k.sqrt()),
        text,
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 49.0 * k + 15.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0 * k.sqrt()),
        skin.map_or(crate::theme::pal().muted, |s| s.text),
    );
    let auto = AUTOMATE_NEXT.with(|c| c.borrow_mut().take());
    let tip = if auto.is_some() {
        "Drag, Shift for fine, double-click to reset, right click to automate"
    } else {
        "Drag, Shift for fine, double-click to reset"
    };
    let resp = resp.on_hover_text(format!("{label}: {}\n{tip}", show(*value)));
    if let Some(req) = auto {
        resp.context_menu(|ui| {
            if ui.button("Create automation clip").clicked() {
                AUTOMATE_ASKED.with(|c| *c.borrow_mut() = Some(req.clone()));
                ui.close();
            }
            crate::midilearn::menu_item(ui, || req.clone());
        });
    }
    changed
}

/// A level meter for a peak value (0 to 1 and a little above).
pub fn meter(ui: &mut Ui, peak: f32, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, crate::theme::pal().well_deep);
    // dB scale: -60 at the bottom, +3 at the top
    let db = 20.0 * peak.max(1e-6).log10();
    let t = ((db + 60.0) / 63.0).clamp(0.0, 1.0);
    let colour = if db > -0.5 {
        Color32::from_rgb(235, 70, 60)
    } else if db > -9.0 {
        Color32::from_rgb(240, 200, 60)
    } else {
        Color32::from_rgb(90, 200, 110)
    };
    let fill = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - rect.height() * t),
        rect.max,
    );
    p.rect_filled(fill, 2.0, colour);
}

const WHITE_IN_OCTAVE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

/// A clickable piano keyboard over `first..=last` (MIDI keys, both white keys).
/// `held` lists the keys to show as pressed. Returns the key under the pointer
/// while the primary button is down.
pub fn piano(ui: &mut Ui, first: u8, last: u8, held: &[u8]) -> Option<u8> {
    let whites: Vec<u8> = (first..=last)
        .filter(|k| WHITE_IN_OCTAVE.contains(&(k % 12)))
        .collect();
    let width = ui.available_width();
    let key_w = (width / whites.len() as f32).clamp(14.0, 40.0);
    let height = 110.0;
    let (rect, resp) = ui.allocate_exact_size(
        vec2(key_w * whites.len() as f32, height),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let white_rect = |i: usize| {
        Rect::from_min_size(
            pos2(rect.left() + i as f32 * key_w, rect.top()),
            vec2(key_w, height),
        )
    };
    // black keys sit over the gap between two white keys
    let mut blacks: Vec<(u8, Rect)> = Vec::new();
    for (i, w) in whites.iter().enumerate() {
        let next = w + 1;
        if next <= last && !WHITE_IN_OCTAVE.contains(&(next % 12)) {
            let x = rect.left() + (i as f32 + 1.0) * key_w - key_w * 0.3;
            blacks.push((
                next,
                Rect::from_min_size(pos2(x, rect.top()), vec2(key_w * 0.6, height * 0.62)),
            ));
        }
    }
    let pointer = ui
        .input(|i| i.pointer.interact_pos())
        .filter(|p| rect.contains(*p));
    let mut hit = None;
    if let Some(p) = pointer {
        hit = blacks.iter().find(|(_, r)| r.contains(p)).map(|(k, _)| *k);
        if hit.is_none() {
            hit = whites
                .iter()
                .enumerate()
                .find(|(i, _)| white_rect(*i).contains(p))
                .map(|(_, k)| *k);
        }
    }
    for (i, k) in whites.iter().enumerate() {
        let r = white_rect(i).shrink2(vec2(0.5, 0.0));
        let fill = if held.contains(k) {
            crate::theme::accent()
        } else {
            crate::theme::pal().key_white
        };
        painter.rect_filled(r, 2.0, fill);
        if k % 12 == 0 {
            painter.text(
                pos2(r.center().x, r.bottom() - 9.0),
                Align2::CENTER_CENTER,
                format!("C{}", k / 12 - 1),
                FontId::proportional(10.0),
                crate::theme::mix(
                    crate::theme::pal().key_black,
                    crate::theme::pal().key_white,
                    0.4,
                ),
            );
        }
    }
    for (k, r) in &blacks {
        let fill = if held.contains(k) {
            crate::theme::accent()
        } else {
            crate::theme::pal().key_black
        };
        painter.rect_filled(*r, 2.0, fill);
    }
    if resp.is_pointer_button_down_on() {
        hit
    } else {
        None
    }
}

/// Name of a MIDI key, such as "C4".
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[(key % 12) as usize], key as i32 / 12 - 1)
}

/// Run one frame of `ui` headless, for tests. The frame's output is dropped the way
/// egui wants (debug builds panic on unapplied texture changes).
#[cfg(test)]
pub fn run_frame(ctx: &egui::Context, input: egui::RawInput, f: impl FnMut(&mut egui::Ui)) {
    let mut out = ctx.run_ui(input, f);
    out.textures_delta.clear();
}

/// Gain to decibels, for the fader scale.
fn gain_db(g: f64) -> f64 {
    if g <= 1e-5 {
        -100.0
    } else {
        20.0 * g.log10()
    }
}

/// Where a gain sits on a fader (0 at the bottom, 1 at the top): a curve like a
/// console's, with unity at about three quarters of the travel.
pub fn fader_pos(g: f64, max: f64) -> f64 {
    (g / max).clamp(0.0, 1.0).powf(0.5)
}

fn fader_gain(pos: f64, max: f64) -> f64 {
    pos.clamp(0.0, 1.0).powi(2) * max
}

/// A mixer fader in FL's style: a tall groove with a dB scale, a ridged cap and a
/// segmented level meter beside it. Drag to move it, Shift for fine steps,
/// double-click for unity. Returns the response; `changed` is set when it moved.
pub fn fader(ui: &mut Ui, value: &mut f64, max: f64, peak: f32, height: f32) -> egui::Response {
    let p = crate::theme::pal();
    let (r, mut resp) = ui.allocate_exact_size(vec2(44.0, height), Sense::click_and_drag());
    let groove = Rect::from_center_size(
        pos2(r.left() + 16.0, r.center().y),
        vec2(6.0, height - 16.0),
    );
    let travel = (groove.top() + 6.0, groove.bottom() - 6.0);
    if resp.double_clicked() {
        *value = 1.0;
        resp.mark_changed();
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let dy = resp.drag_delta().y as f64 / f64::from(travel.1 - travel.0);
        let pos = fader_pos(*value, max) - dy * if fine { 0.2 } else { 1.0 };
        *value = fader_gain(pos, max);
        resp.mark_changed();
    }
    let painter = ui.painter_at(r);
    // dB marks
    for db in [6.0f64, 0.0, -6.0, -12.0, -24.0, -48.0] {
        let pos = fader_pos(10f64.powf(db / 20.0), max) as f32;
        if pos <= 0.0 {
            continue;
        }
        let y = travel.1 - pos * (travel.1 - travel.0);
        painter.line_segment(
            [pos2(groove.left() - 6.0, y), pos2(groove.left() - 2.0, y)],
            Stroke::new(1.0, p.muted),
        );
        if db == 0.0 {
            painter.line_segment(
                [pos2(groove.left() - 9.0, y), pos2(groove.right() + 2.0, y)],
                Stroke::new(1.0, p.text2),
            );
        }
    }
    painter.rect_filled(groove, 3.0, p.well_deep);
    // the cap
    let pos = fader_pos(*value, max) as f32;
    let y = travel.1 - pos * (travel.1 - travel.0);
    let cap = Rect::from_center_size(pos2(groove.center().x, y), vec2(22.0, 30.0));
    painter.rect_filled(
        cap.translate(vec2(0.0, 2.0)),
        3.0,
        Color32::from_black_alpha(90),
    );
    let hot = resp.hovered() || resp.dragged();
    painter.rect_filled(cap, 3.0, if hot { p.raised2 } else { p.raised });
    for k in [-8.0, -4.0, 4.0, 8.0] {
        painter.line_segment(
            [
                pos2(cap.left() + 4.0, y + k),
                pos2(cap.right() - 4.0, y + k),
            ],
            Stroke::new(1.0, Color32::from_black_alpha(80)),
        );
    }
    painter.line_segment(
        [pos2(cap.left() + 2.0, y), pos2(cap.right() - 2.0, y)],
        Stroke::new(2.0, crate::theme::accent()),
    );
    // the meter: segments from green through yellow to red
    let m = Rect::from_min_max(
        pos2(r.right() - 12.0, groove.top()),
        pos2(r.right() - 4.0, groove.bottom()),
    );
    painter.rect_filled(m, 2.0, p.well_deep);
    let segs = 28;
    let lit = (fader_pos(f64::from(peak), max) * segs as f64).round() as usize;
    let seg_h = m.height() / segs as f32;
    for s in 0..segs.min(lit) {
        let frac = s as f32 / segs as f32;
        let col = if frac > 0.86 {
            Color32::from_rgb(240, 70, 60)
        } else if frac > 0.7 {
            Color32::from_rgb(240, 200, 60)
        } else {
            Color32::from_rgb(80, 220, 110)
        };
        let yb = m.bottom() - s as f32 * seg_h;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(m.left() + 1.0, yb - seg_h + 1.0),
                pos2(m.right() - 1.0, yb),
            ),
            0.0,
            col,
        );
    }
    resp.on_hover_text(format!(
        "{:.1} dB\nDrag, Shift for fine, double-click for 0 dB",
        gain_db(*value)
    ))
}
