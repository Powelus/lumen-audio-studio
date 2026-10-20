// SPDX-License-Identifier: GPL-3.0-or-later
//! The channel rack: every channel of the project with its notes in the chosen
//! pattern drawn as a strip, and the playhead while that pattern plays.

use crate::app::App;
use crate::widgets::ACCENT;
use eframe::egui::{self, Color32, Rect, RichText, Sense, Stroke, Ui};
use las_engine::timeline::{pattern_length, ticks_per_bar};
use las_project::{Channel, Note, Pattern, Project, TICKS_PER_STEP};

const ROW: f32 = 34.0;
const NAME: f32 = 170.0;

/// A channel's colour (`#rrggbb`), or the accent when it has none.
pub fn channel_colour(c: &Channel) -> Color32 {
    let h = c.color.trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    ACCENT
}

/// The keys a channel's notes span, widened to at least an octave.
fn key_range(notes: &[Note]) -> (i32, i32) {
    let lo = notes.iter().map(|n| n.key).min().unwrap_or(60);
    let hi = notes.iter().map(|n| n.key).max().unwrap_or(72);
    if hi - lo < 11 {
        let mid = (lo + hi) / 2;
        (mid - 6, mid + 6)
    } else {
        (lo, hi)
    }
}

/// Paint the notes of one channel into `rect`.
fn strip(ui: &Ui, rect: Rect, notes: &[Note], ticks: f64, colour: Color32, bar: f64) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_gray(26));
    let x = |t: f64| rect.left() + (t / ticks) as f32 * rect.width();
    // beat lines, bar lines stronger
    let beat = ticks_per_bar((1, 4));
    let mut t = 0.0;
    while t < ticks {
        let strong = (t / bar).fract().abs() < 1e-9;
        let g = if strong { 70 } else { 40 };
        p.line_segment(
            [
                egui::pos2(x(t), rect.top()),
                egui::pos2(x(t), rect.bottom()),
            ],
            Stroke::new(1.0, Color32::from_gray(g)),
        );
        t += beat;
    }
    let (lo, hi) = key_range(notes);
    let span = (hi - lo + 1) as f32;
    for n in notes {
        let w = if n.step == Some(true) {
            f64::from(TICKS_PER_STEP)
        } else {
            n.len.max(f64::from(TICKS_PER_STEP) * 0.5)
        };
        let (y0, h) = if n.step == Some(true) {
            (rect.top() + 3.0, rect.height() - 6.0)
        } else {
            let row = rect.height() / span;
            (rect.bottom() - (n.key - lo + 1) as f32 * row, row.max(2.0))
        };
        let a = (0.45 + 0.55 * n.vel.clamp(0.0, 1.0)) as f32;
        let r = Rect::from_min_max(
            egui::pos2(x(n.pos), y0),
            egui::pos2((x(n.pos + w) - 1.0).max(x(n.pos) + 2.0), y0 + h),
        );
        p.rect_filled(r, 1.5, colour.gamma_multiply(a));
    }
}

impl App {
    pub(crate) fn rack(&mut self, ui: &mut Ui, playhead: Option<f64>) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        draw(
            ui,
            &project,
            self.project.pattern,
            &mut self.project.channel,
            playhead,
        );
    }
}

/// Draw the rack of `project` for one pattern; a click on a channel selects it.
pub fn draw(
    ui: &mut Ui,
    project: &Project,
    pattern: usize,
    selected: &mut usize,
    playhead: Option<f64>,
) {
    {
        let pattern: Option<&Pattern> = project.patterns.get(pattern);
        let ticks = pattern_length(pattern, project.time_sig);
        let bar = ticks_per_bar(project.time_sig);
        ui.label(RichText::new("Channel rack").strong());
        let empty: Vec<Note> = Vec::new();
        for (i, c) in project.channels.iter().enumerate() {
            let notes = pattern.and_then(|p| p.notes.get(&c.id)).unwrap_or(&empty);
            ui.horizontal(|ui| {
                let (nr, resp) = ui.allocate_exact_size(egui::vec2(NAME, ROW), Sense::click());
                let is_selected = *selected == i;
                let col = channel_colour(c);
                let p = ui.painter_at(nr);
                let bg = if is_selected {
                    Color32::from_gray(52)
                } else {
                    Color32::from_gray(34)
                };
                p.rect_filled(nr, 3.0, bg);
                p.rect_filled(
                    Rect::from_min_size(nr.min, egui::vec2(4.0, nr.height())),
                    2.0,
                    col,
                );
                p.text(
                    nr.left_center() + egui::vec2(12.0, -7.0),
                    egui::Align2::LEFT_CENTER,
                    &c.name,
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(225),
                );
                p.text(
                    nr.left_center() + egui::vec2(12.0, 8.0),
                    egui::Align2::LEFT_CENTER,
                    &c.plugin_id,
                    egui::FontId::proportional(10.0),
                    Color32::from_gray(140),
                );
                if resp.clicked() {
                    *selected = i;
                }
                let w = (ui.available_width() - 8.0).max(120.0);
                let (sr, _) = ui.allocate_exact_size(egui::vec2(w, ROW), Sense::hover());
                strip(ui, sr, notes, ticks, col, bar);
                if let Some(t) = playhead {
                    let x = sr.left() + (t / ticks).clamp(0.0, 1.0) as f32 * sr.width();
                    ui.painter_at(sr).line_segment(
                        [egui::pos2(x, sr.top()), egui::pos2(x, sr.bottom())],
                        Stroke::new(1.5, Color32::WHITE),
                    );
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rack_draws_every_channel_of_a_project_and_selects_on_click() {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/engines.las");
        let project = las_project::decode(bytes).unwrap().project;
        let ctx = egui::Context::default();
        let mut selected = 0usize;
        let mut seen = 0;
        for pattern in 0..project.patterns.len().max(1) {
            crate::widgets::run_frame(&ctx, egui::RawInput::default(), |ui| {
                draw(ui, &project, pattern, &mut selected, Some(100.0));
                seen += 1;
            });
        }
        assert!(seen >= 1);
        assert_eq!(selected, 0);
    }

    #[test]
    fn colours_parse_and_fall_back() {
        let with = |color: &str| Channel {
            color: color.into(),
            ..Default::default()
        };
        assert_eq!(
            channel_colour(&with("#10ff20")),
            Color32::from_rgb(0x10, 0xff, 0x20)
        );
        assert_eq!(channel_colour(&with("nonsense")), ACCENT);
    }

    #[test]
    fn the_key_range_is_at_least_an_octave() {
        let n = |key| Note {
            key,
            ..Default::default()
        };
        let (lo, hi) = key_range(&[n(60), n(62)]);
        assert!(hi - lo >= 11 && lo <= 60 && hi >= 62);
        assert_eq!(key_range(&[n(40), n(70)]), (40, 70));
    }
}
