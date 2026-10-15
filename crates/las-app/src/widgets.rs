// SPDX-License-Identifier: GPL-3.0-or-later
//! Hand-drawn controls: knob, level meter and piano keyboard.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui,
};
use std::f32::consts::PI;
use std::ops::RangeInclusive;

pub const ACCENT: Color32 = Color32::from_rgb(255, 138, 31);
const TRACK: Color32 = Color32::from_rgb(58, 62, 70);
const BODY: Color32 = Color32::from_rgb(34, 37, 43);

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
    let size = vec2(54.0, 76.0);
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
    let centre = pos2(rect.center().x, rect.top() + 24.0);
    let radius = 18.0;
    painter.circle_filled(centre, radius + 3.0, BODY);
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
    painter.add(Shape::line(arc(0.0, 1.0, radius), Stroke::new(3.0, TRACK)));
    let t = to_norm(*value).clamp(0.0, 1.0) as f32;
    if t > 0.002 {
        let lit = if resp.hovered() || resp.dragged() {
            Color32::from_rgb(255, 170, 80)
        } else {
            ACCENT
        };
        painter.add(Shape::line(arc(0.0, t, radius), Stroke::new(3.0, lit)));
    }
    let a = a0 + sweep * t;
    painter.line_segment(
        [
            pos2(centre.x + 6.0 * a.cos(), centre.y + 6.0 * a.sin()),
            pos2(
                centre.x + (radius - 3.0) * a.cos(),
                centre.y + (radius - 3.0) * a.sin(),
            ),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 49.0),
        Align2::CENTER_CENTER,
        show(*value),
        FontId::proportional(11.0),
        Color32::from_gray(210),
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 64.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        Color32::from_gray(140),
    );
    resp.on_hover_text(format!(
        "{label}: {}\nDrag, Shift for fine, double-click to reset",
        show(*value)
    ));
    changed
}

/// A level meter for a peak value (0 to 1 and a little above).
pub fn meter(ui: &mut Ui, peak: f32, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, BODY);
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
            ACCENT
        } else {
            Color32::from_gray(232)
        };
        painter.rect_filled(r, 2.0, fill);
        if k % 12 == 0 {
            painter.text(
                pos2(r.center().x, r.bottom() - 9.0),
                Align2::CENTER_CENTER,
                format!("C{}", k / 12 - 1),
                FontId::proportional(10.0),
                Color32::from_gray(90),
            );
        }
    }
    for (k, r) in &blacks {
        let fill = if held.contains(k) {
            ACCENT
        } else {
            Color32::from_gray(28)
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
