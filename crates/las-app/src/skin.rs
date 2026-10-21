// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin faces. Each instrument window is painted like the hardware-style synth it
//! is modelled on (a dark wavetable synth with blue screens, an FM synth in black and
//! amber, a three oscillator synth in FL grey and green), instead of the app's own
//! theme: its own background, section panels, knob colours, screens and header bar.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Stroke, Ui};
use std::cell::Cell;

/// The colours of one plugin face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skin {
    pub name: &'static str,
    pub tagline: &'static str,
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    pub section: Color32,
    pub edge: Color32,
    pub header: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub knob: Color32,
    pub track: Color32,
    pub screen: Color32,
    pub trace: Color32,
    /// Flat panels and knobs, with no shadows or glows.
    pub flat: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The face of an instrument (by its plugin id).
pub fn skin_for(plugin: &str) -> Skin {
    match plugin {
        // a modern wavetable synth: graphite panels, blue screens and arcs
        "polaris" | "vortex" => Skin {
            name: if plugin == "vortex" {
                "VORTEX"
            } else {
                "POLARIS"
            },
            tagline: "advanced wavetable synthesizer",
            bg_top: rgb(44, 49, 56),
            bg_bottom: rgb(27, 30, 35),
            section: rgb(34, 38, 44),
            edge: rgb(62, 70, 80),
            header: rgb(20, 22, 26),
            text: rgb(214, 222, 232),
            dim: rgb(130, 142, 156),
            accent: rgb(80, 170, 255),
            knob: rgb(58, 64, 73),
            track: rgb(22, 25, 29),
            screen: rgb(14, 20, 28),
            trace: rgb(120, 205, 255),
            flat: false,
        },
        // a classic analogue synth: a black panel between wooden cheeks, cream print
        "monolith" => Skin {
            name: "MONOLITH",
            tagline: "virtual analogue synthesizer",
            bg_top: rgb(30, 29, 28),
            bg_bottom: rgb(18, 17, 17),
            section: rgb(24, 23, 22),
            edge: rgb(70, 66, 60),
            header: rgb(12, 12, 12),
            text: rgb(240, 232, 212),
            dim: rgb(170, 160, 140),
            accent: rgb(236, 112, 48),
            knob: rgb(16, 16, 16),
            track: rgb(44, 42, 40),
            screen: rgb(14, 13, 12),
            trace: rgb(250, 170, 110),
            flat: false,
        },
        // a supersaw synth: neon lines on a deep violet night
        "zenith" => Skin {
            name: "ZENITH",
            tagline: "supersaw and pad synthesizer",
            bg_top: rgb(26, 16, 44),
            bg_bottom: rgb(8, 6, 18),
            section: rgb(18, 12, 34),
            edge: rgb(110, 50, 170),
            header: rgb(8, 4, 16),
            text: rgb(236, 230, 255),
            dim: rgb(170, 150, 210),
            accent: rgb(255, 60, 200),
            knob: rgb(34, 24, 58),
            track: rgb(12, 8, 24),
            screen: rgb(6, 4, 14),
            trace: rgb(60, 235, 255),
            flat: false,
        },
        // FL's three oscillator synth: mid grey and green
        "triosc" => Skin {
            name: "TRIOSC",
            tagline: "three oscillator synthesizer",
            bg_top: rgb(84, 92, 96),
            bg_bottom: rgb(64, 70, 74),
            section: rgb(74, 81, 85),
            edge: rgb(110, 120, 124),
            header: rgb(46, 51, 54),
            text: rgb(232, 240, 236),
            dim: rgb(170, 184, 178),
            accent: rgb(140, 220, 90),
            knob: rgb(52, 57, 60),
            track: rgb(44, 48, 50),
            screen: rgb(26, 34, 26),
            trace: rgb(160, 240, 110),
            flat: true,
        },
        // an FM synth: black and amber
        "facet" => Skin {
            name: "FACET",
            tagline: "four operator FM synthesizer",
            bg_top: rgb(34, 34, 36),
            bg_bottom: rgb(16, 16, 18),
            section: rgb(28, 28, 30),
            edge: rgb(66, 62, 54),
            header: rgb(10, 10, 12),
            text: rgb(240, 232, 214),
            dim: rgb(150, 142, 124),
            accent: rgb(255, 190, 60),
            knob: rgb(46, 46, 48),
            track: rgb(14, 14, 16),
            screen: rgb(24, 18, 8),
            trace: rgb(255, 205, 100),
            flat: false,
        },
        // a granular synth: deep teal and violet
        "mist" => Skin {
            name: "MIST",
            tagline: "granular texture synthesizer",
            bg_top: rgb(30, 42, 48),
            bg_bottom: rgb(18, 24, 30),
            section: rgb(24, 34, 40),
            edge: rgb(52, 76, 86),
            header: rgb(12, 18, 22),
            text: rgb(214, 238, 240),
            dim: rgb(120, 160, 168),
            accent: rgb(90, 230, 210),
            knob: rgb(40, 54, 62),
            track: rgb(14, 20, 24),
            screen: rgb(10, 20, 24),
            trace: rgb(150, 255, 236),
            flat: false,
        },
        // a ROM preset player: deep indigo panels around a violet blue display
        "nebula" => Skin {
            name: "NEBULA",
            tagline: "layered rom preset player",
            bg_top: rgb(30, 26, 62),
            bg_bottom: rgb(12, 10, 30),
            section: rgb(24, 21, 52),
            edge: rgb(72, 64, 140),
            header: rgb(10, 8, 26),
            text: rgb(228, 226, 255),
            dim: rgb(140, 138, 200),
            accent: rgb(150, 110, 255),
            knob: rgb(44, 38, 88),
            track: rgb(14, 12, 34),
            screen: rgb(14, 18, 58),
            trace: rgb(140, 200, 255),
            flat: false,
        },
        // drum pads: black with red
        "lumapad" => Skin {
            name: "LUMAPAD",
            tagline: "drum pad sampler",
            bg_top: rgb(38, 38, 40),
            bg_bottom: rgb(22, 22, 24),
            section: rgb(30, 30, 32),
            edge: rgb(70, 70, 74),
            header: rgb(14, 14, 16),
            text: rgb(240, 236, 236),
            dim: rgb(160, 150, 150),
            accent: rgb(255, 80, 90),
            knob: rgb(50, 50, 54),
            track: rgb(16, 16, 18),
            screen: rgb(20, 12, 14),
            trace: rgb(255, 130, 140),
            flat: false,
        },
        // the modular synth: a deep indigo case with patch cable magenta
        "weave" => Skin {
            name: "WEAVE",
            tagline: "modular synthesizer",
            bg_top: rgb(36, 32, 52),
            bg_bottom: rgb(20, 18, 30),
            section: rgb(30, 27, 44),
            edge: rgb(70, 62, 98),
            header: rgb(16, 14, 24),
            text: rgb(230, 224, 244),
            dim: rgb(150, 140, 176),
            accent: rgb(232, 84, 200),
            knob: rgb(56, 50, 78),
            track: rgb(18, 16, 28),
            screen: rgb(14, 12, 22),
            trace: rgb(255, 140, 230),
            flat: false,
        },
        // effect windows: a rack unit in brushed dark steel with a green LED accent
        "fx" => Skin {
            name: "EFFECT",
            tagline: "mixer insert effect",
            bg_top: rgb(52, 56, 60),
            bg_bottom: rgb(32, 35, 38),
            section: rgb(42, 46, 50),
            edge: rgb(76, 82, 88),
            header: rgb(22, 24, 26),
            text: rgb(228, 234, 236),
            dim: rgb(150, 160, 164),
            accent: rgb(70, 230, 140),
            knob: rgb(60, 65, 70),
            track: rgb(24, 26, 28),
            screen: rgb(12, 22, 16),
            trace: rgb(110, 255, 170),
            flat: false,
        },
        _ => Skin {
            name: "SAMPLER",
            tagline: "sample player",
            bg_top: rgb(48, 52, 56),
            bg_bottom: rgb(30, 33, 36),
            section: rgb(38, 42, 46),
            edge: rgb(70, 76, 82),
            header: rgb(22, 24, 26),
            text: rgb(226, 230, 234),
            dim: rgb(146, 154, 160),
            accent: rgb(255, 160, 60),
            knob: rgb(56, 60, 66),
            track: rgb(24, 26, 28),
            screen: rgb(16, 18, 20),
            trace: rgb(255, 190, 110),
            flat: false,
        },
    }
}

thread_local! {
    static CURRENT: Cell<Option<Skin>> = const { Cell::new(None) };
}

/// The face being painted, if any.
pub fn current() -> Option<Skin> {
    CURRENT.with(Cell::get)
}

/// Paint a window's content with a plugin face: the background, the header bar with
/// the instrument's name, and widget colours; `add` draws the controls.
pub fn face<R>(ui: &mut Ui, skin: Skin, add: impl FnOnce(&mut Ui) -> R) -> R {
    // the background goes under everything, sized to what was drawn
    let bg = ui.painter().add(egui::Shape::Noop);
    let before = CURRENT.with(|c| c.replace(Some(skin)));
    let saved = ui.visuals().clone();
    {
        let v = ui.visuals_mut();
        v.override_text_color = Some(skin.text);
        v.extreme_bg_color = skin.screen;
        v.faint_bg_color = skin.section;
        v.selection.bg_fill = skin.accent.gamma_multiply(0.6);
        v.widgets.inactive.weak_bg_fill = skin.knob;
        v.widgets.inactive.bg_fill = skin.knob;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, skin.edge);
        v.widgets.hovered.weak_bg_fill = skin.edge;
        v.widgets.hovered.bg_fill = skin.edge;
        v.widgets.active.weak_bg_fill = skin.accent.gamma_multiply(0.5);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, skin.edge);
    }
    header(ui, skin);
    let r = add(ui);
    let full = ui.min_rect().expand(8.0).intersect(ui.clip_rect());
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(full.left_top(), skin.bg_top);
    mesh.colored_vertex(full.right_top(), skin.bg_top);
    mesh.colored_vertex(full.right_bottom(), skin.bg_bottom);
    mesh.colored_vertex(full.left_bottom(), skin.bg_bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().set(bg, egui::Shape::mesh(mesh));
    *ui.visuals_mut() = saved;
    CURRENT.with(|c| c.set(before));
    r
}

/// The bar at the top: the instrument's name in large spaced capitals.
fn header(ui: &mut Ui, skin: Skin) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 46.0), egui::Sense::hover());
    let p = ui.painter();
    let mut mesh = egui::Mesh::default();
    let lit = skin.header.lerp_to_gamma(skin.accent, 0.18);
    mesh.colored_vertex(r.left_top(), lit);
    mesh.colored_vertex(r.right_top(), skin.header);
    mesh.colored_vertex(r.right_bottom(), skin.header);
    mesh.colored_vertex(r.left_bottom(), skin.header);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
    // the accent line under the bar, with a glow
    p.line_segment(
        [
            r.left_bottom() + vec2(0.0, 1.0),
            r.right_bottom() + vec2(0.0, 1.0),
        ],
        Stroke::new(6.0, skin.accent.gamma_multiply(0.18)),
    );
    p.line_segment(
        [r.left_bottom(), r.right_bottom()],
        Stroke::new(2.0, skin.accent),
    );
    let spaced: String = skin.name.chars().flat_map(|c| [c, '\u{2009}']).collect();
    // a glow behind the name, then the name
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        p.text(
            r.left_center() + vec2(16.0 + dx * 1.5, dy * 1.5),
            Align2::LEFT_CENTER,
            &spaced,
            FontId::proportional(26.0),
            skin.accent.gamma_multiply(0.25),
        );
    }
    p.text(
        r.left_center() + vec2(16.0, 0.0),
        Align2::LEFT_CENTER,
        spaced,
        FontId::proportional(26.0),
        skin.text,
    );
    p.text(
        r.right_center() - vec2(14.0, 0.0),
        Align2::RIGHT_CENTER,
        skin.tagline.to_uppercase(),
        FontId::proportional(10.5),
        skin.dim,
    );
    ui.add_space(4.0);
}

/// A section of controls: a filled panel with a thin edge (the plain group without a
/// face).
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    match current() {
        Some(s) if s.flat => egui::Frame::new()
            .fill(s.section)
            .stroke(Stroke::new(1.0, s.edge))
            .corner_radius(2.0)
            .inner_margin(6.0)
            .show(ui, add),
        Some(s) => {
            let r = egui::Frame::new()
                .fill(s.section)
                .stroke(Stroke::new(1.0, s.edge))
                .corner_radius(6.0)
                .inner_margin(8.0)
                .shadow(egui::epaint::Shadow {
                    offset: [0, 3],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(70),
                })
                .show(ui, add);
            // a highlight along the top edge, as on a bevelled panel
            let rect = r.response.rect;
            ui.painter().line_segment(
                [
                    rect.left_top() + vec2(6.0, 1.0),
                    rect.right_top() + vec2(-6.0, 1.0),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(22)),
            );
            r
        }
        None => ui.group(add),
    }
}

/// A screen with one cycle of a waveform (`samples` run over the cycle).
pub fn wave_screen(ui: &mut Ui, size: egui::Vec2, samples: &[f32], on: bool) {
    let s = current().unwrap_or_else(|| skin_for("polaris"));
    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, s.screen);
    // a faint grid, as on the screens of hardware
    for i in 1..4 {
        let x = r.left() + r.width() * i as f32 / 4.0;
        p.line_segment(
            [pos2(x, r.top()), pos2(x, r.bottom())],
            Stroke::new(1.0, s.trace.gamma_multiply(0.08)),
        );
    }
    p.line_segment(
        [r.left_center(), r.right_center()],
        Stroke::new(1.0, s.trace.gamma_multiply(0.18)),
    );
    if samples.is_empty() {
        return;
    }
    let peak = samples.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let n = samples.len();
    let line: Vec<egui::Pos2> = (0..=120)
        .map(|i| {
            let t = i as f32 / 120.0;
            let v = samples[((t * n as f32) as usize).min(n - 1)] / peak;
            pos2(
                r.left() + 4.0 + t * (r.width() - 8.0),
                r.center().y - v * (r.height() / 2.0 - 6.0),
            )
        })
        .collect();
    let col = if on {
        s.trace
    } else {
        s.trace.gamma_multiply(0.3)
    };
    // a glow under the line, then the line
    p.add(egui::Shape::line(
        line.clone(),
        Stroke::new(5.0, col.gamma_multiply(0.18)),
    ));
    p.add(egui::Shape::line(line, Stroke::new(1.6, col)));
}

/// A wavetable in 3D, the way modern wavetable synths draw it: every frame of the table
/// stacked back to front in perspective, each one a filled shape that hides the ones
/// behind, over a floor grid; the frame the oscillator plays glows. A table of one frame
/// is drawn as a ribbon of copies, so the picture always has depth.
pub fn table_3d(ui: &mut Ui, size: egui::Vec2, frames: &[Vec<f32>], pos: f32) {
    let s = current().unwrap_or_else(|| skin_for("vortex"));
    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(r);
    // the screen: a dark well with a vertical gradient
    let mut mesh = egui::Mesh::default();
    let top = s.screen.gamma_multiply(1.6);
    mesh.colored_vertex(r.left_top(), top);
    mesh.colored_vertex(r.right_top(), top);
    mesh.colored_vertex(r.right_bottom(), s.screen);
    mesh.colored_vertex(r.left_bottom(), s.screen);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
    if frames.is_empty() {
        return;
    }
    let n = frames.len();
    let shown = if n > 1 { n.min(40) } else { 14 };
    // the front frame at the bottom left, the back one up and to the right, smaller
    let front_w = r.width() * 0.66;
    let front_h = r.height() * 0.42;
    let front = pos2(r.left() + 10.0, r.bottom() - front_h * 0.62 - 6.0);
    let back = pos2(
        r.right() - 10.0 - front_w * 0.78,
        r.top() + front_h * 0.5 + 6.0,
    );
    let place = |k: usize| {
        let d = k as f32 / (shown - 1).max(1) as f32;
        let o = front + (back - front) * d;
        let scale = 1.0 - 0.22 * d;
        (o, front_w * scale, front_h * scale, d)
    };
    // floor grid between the front and back lines
    for i in 0..=6 {
        let t = i as f32 / 6.0;
        let (o0, w0, _, _) = place(0);
        let (o1, w1, _, _) = place(shown - 1);
        let a = pos2(o0.x + w0 * t, o0.y + front_h * 0.55);
        let b2 = pos2(o1.x + w1 * t, o1.y + front_h * 0.45);
        p.line_segment([a, b2], Stroke::new(1.0, s.trace.gamma_multiply(0.07)));
    }
    let sel = (pos.clamp(0.0, 1.0) * (shown - 1) as f32).round() as usize;
    let frame_of = |k: usize| -> &Vec<f32> {
        if n > 1 {
            &frames[k * (n - 1) / (shown - 1).max(1)]
        } else {
            &frames[0]
        }
    };
    let peak = frames
        .iter()
        .flat_map(|f| f.iter())
        .fold(0f32, |m, v| m.max(v.abs()))
        .max(1e-6);
    for k in (0..shown).rev() {
        let f = frame_of(k);
        let (o, w, h, d) = place(k);
        let pts: Vec<egui::Pos2> = (0..=72)
            .map(|i| {
                let t = i as f32 / 72.0;
                let v = f[((t * f.len() as f32) as usize).min(f.len() - 1)] / peak;
                pos2(o.x + t * w, o.y - v * h * 0.5)
            })
            .collect();
        // fill down to the frame's base line, so nearer frames hide farther ones
        let base = o.y + h * 0.55;
        let shade = s.screen.gamma_multiply(1.25 + 0.4 * (1.0 - d));
        // a filled band under the curve, drawn as thin quads (the curve is not convex)
        for win in pts.windows(2) {
            let quad = vec![win[0], win[1], pos2(win[1].x, base), pos2(win[0].x, base)];
            p.add(egui::Shape::convex_polygon(quad, shade, Stroke::NONE));
        }
        let (width, col) = if k == sel {
            (2.2, s.trace)
        } else {
            (1.0, s.trace.gamma_multiply(0.18 + 0.32 * (1.0 - d)))
        };
        if k == sel {
            p.add(egui::Shape::line(
                pts.clone(),
                Stroke::new(7.0, s.trace.gamma_multiply(0.18)),
            ));
        }
        p.add(egui::Shape::line(pts, Stroke::new(width, col)));
    }
    p.text(
        r.right_top() + vec2(-8.0, 6.0),
        Align2::RIGHT_TOP,
        if n > 1 {
            format!(
                "{} / {}",
                (pos.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize + 1,
                n
            )
        } else {
            "1 / 1".to_string()
        },
        FontId::monospace(10.0),
        s.trace.gamma_multiply(0.7),
    );
}

/// Mist's screen: the source as a band of texture, and the grains around the play
/// position (spread by the spray, sized by the grain length, as many as the density).
pub fn grain_cloud(
    ui: &mut Ui,
    size: egui::Vec2,
    position: f32,
    spray: f32,
    grain_ms: f32,
    density: f32,
) {
    let s = current().unwrap_or_else(|| skin_for("mist"));
    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, s.screen);
    // a steady pseudo-random texture, so the picture does not flicker
    let hash = |i: u32| {
        let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
        x ^= x >> 15;
        x = x.wrapping_mul(0x2C1B_3C6D);
        x ^= x >> 12;
        (x & 0xFFFF) as f32 / 65535.0
    };
    let mid = r.center().y;
    let cols = r.width() as u32 / 2;
    for i in 0..cols {
        let x = r.left() + i as f32 * 2.0;
        let env = 0.35 + 0.65 * (i as f32 * 0.031).sin().abs();
        let h = (hash(i) * 0.7 + 0.3) * env * (r.height() * 0.38);
        p.line_segment(
            [pos2(x, mid - h), pos2(x, mid + h)],
            Stroke::new(1.0, s.trace.gamma_multiply(0.16)),
        );
    }
    // the grains: a cloud around the position
    let at = r.left() + position.clamp(0.0, 1.0) * r.width();
    let spread = (spray / 0.5).clamp(0.0, 1.0) * r.width() * 0.25 + 2.0;
    let count = (density.clamp(1.0, 100.0) * 0.6) as u32 + 4;
    let radius = (grain_ms.clamp(5.0, 500.0) / 500.0).sqrt() * 10.0 + 2.0;
    for g in 0..count {
        let dx = (hash(g * 2 + 1) - 0.5) * 2.0 * spread;
        let dy = (hash(g * 2 + 7) - 0.5) * r.height() * 0.7;
        p.circle_filled(
            pos2(at + dx, mid + dy),
            radius,
            s.trace.gamma_multiply(0.25),
        );
        p.circle_filled(
            pos2(at + dx, mid + dy),
            radius * 0.4,
            s.trace.gamma_multiply(0.8),
        );
    }
    p.line_segment(
        [pos2(at, r.top() + 4.0), pos2(at, r.bottom() - 4.0)],
        Stroke::new(1.5, s.accent),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_instrument_has_its_own_face() {
        let ids = [
            "polaris", "vortex", "monolith", "zenith", "triosc", "facet", "mist", "nebula",
            "lumapad", "sampler", "weave",
        ];
        let accents: std::collections::HashSet<_> =
            ids.iter().map(|i| skin_for(i).accent).collect();
        assert!(accents.len() >= 8, "faces are told apart by colour");
        assert_eq!(skin_for("vortex").name, "VORTEX");
        assert_eq!(skin_for("weave").name, "WEAVE");
        // text stands out from its background on every face
        for i in ids {
            let s = skin_for(i);
            let lum = |c: Color32| c.r() as i32 + c.g() as i32 + c.b() as i32;
            assert!(lum(s.text) - lum(s.section) > 300, "{i}");
        }
    }
}
