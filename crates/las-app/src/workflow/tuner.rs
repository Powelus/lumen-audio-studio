// SPDX-License-Identifier: GPL-3.0-or-later
//! A tuner for a mixer insert: what the insert plays is sent here by the audio thread,
//! and its pitch is shown as a note and how many cents sharp or flat it is.

use eframe::egui::{self, pos2, vec2, Align2, FontId, RichText, Sense, Stroke, Ui};
use las_dsp::pitch::{detect, key_name, reading, Reading};

/// Samples analysed at a time.
pub const WINDOW: usize = 4096;

#[derive(Default)]
pub struct Tuner {
    /// The insert listened to, while the tuner is open.
    pub insert: Option<usize>,
    /// The latest samples, oldest first.
    buf: Vec<f32>,
    /// The last pitch found, kept for a moment when the sound stops.
    pub reading: Option<Reading>,
    held: u32,
    /// Tuning reference, Hz.
    pub a4: f64,
}

impl Tuner {
    /// Take new samples and update the reading.
    pub fn feed(&mut self, samples: impl Iterator<Item = f32>, sr: f64) {
        self.buf.extend(samples);
        if self.buf.len() > WINDOW {
            let cut = self.buf.len() - WINDOW;
            self.buf.drain(..cut);
        }
        if self.buf.len() < WINDOW {
            return;
        }
        let a4 = if self.a4 > 0.0 { self.a4 } else { 440.0 };
        match detect(&self.buf, sr, 30.0, 2000.0) {
            Some((hz, clarity)) if clarity > 0.6 => {
                let r = reading(hz, clarity, a4);
                // a little smoothing of the cents on the same note
                self.reading = Some(match self.reading {
                    Some(old) if old.key == r.key => Reading {
                        cents: old.cents * 0.6 + r.cents * 0.4,
                        ..r
                    },
                    _ => r,
                });
                self.held = 0;
            }
            _ => {
                self.held += 1;
                if self.held > 30 {
                    self.reading = None;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.reading = None;
    }
}

/// Draw the tuner: the note, the cents and a needle.
pub fn draw(ui: &mut Ui, t: &mut Tuner, insert_name: &str) {
    let p = crate::theme::pal();
    ui.label(RichText::new(format!("Listening to {insert_name}")).color(p.muted))
        .on_hover_text("Right-click another insert's name in the mixer and choose Tuner to switch");
    let (rect, _) = ui.allocate_exact_size(vec2(260.0, 110.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, p.panel2);
    let (note, cents, colour) = match t.reading {
        Some(r) => {
            let good = r.cents.abs() < 5.0;
            (
                key_name(r.key),
                r.cents,
                if good { p.accent } else { p.text },
            )
        }
        None => ("-".to_string(), 0.0, p.muted),
    };
    painter.text(
        pos2(rect.center().x, rect.top() + 34.0),
        Align2::CENTER_CENTER,
        note,
        FontId::proportional(34.0),
        colour,
    );
    // the scale: -50 to +50 cents
    let y = rect.bottom() - 26.0;
    let (x0, x1) = (rect.left() + 16.0, rect.right() - 16.0);
    painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.0, p.border));
    for c in [-50.0f32, -25.0, 0.0, 25.0, 50.0] {
        let x = x0 + (c + 50.0) / 100.0 * (x1 - x0);
        let h = if c == 0.0 { 8.0 } else { 4.0 };
        painter.line_segment([pos2(x, y - h), pos2(x, y + h)], Stroke::new(1.0, p.muted));
    }
    if t.reading.is_some() {
        let x = x0 + (cents as f32 + 50.0) / 100.0 * (x1 - x0);
        painter.line_segment(
            [pos2(x, y - 14.0), pos2(x, y + 6.0)],
            Stroke::new(3.0, colour),
        );
    }
    let text = match t.reading {
        Some(r) => format!("{:+.0} cents   {:.1} Hz", r.cents, r.hz),
        None => "Play a single note".into(),
    };
    painter.text(
        pos2(rect.center().x, rect.bottom() - 9.0),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        p.text2,
    );
    ui.horizontal(|ui| {
        ui.label("A4 =");
        if t.a4 <= 0.0 {
            t.a4 = 440.0;
        }
        ui.add(
            egui::DragValue::new(&mut t.a4)
                .range(400.0..=480.0)
                .suffix(" Hz"),
        )
        .on_hover_text("The tuning reference");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tuner_reads_a_slightly_sharp_a_and_forgets_it_in_silence() {
        let sr = 48000.0;
        let hz = 440.0 * 2f64.powf(10.0 / 1200.0);
        let mut t = Tuner::default();
        let tone: Vec<f32> = (0..WINDOW * 2)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / sr).sin() as f32 * 0.5)
            .collect();
        t.feed(tone.iter().copied(), sr);
        let r = t.reading.expect("a note");
        assert_eq!(key_name(r.key), "A4");
        assert!((r.cents - 10.0).abs() < 2.0, "{}", r.cents);
        for _ in 0..40 {
            t.feed(std::iter::repeat_n(0.0, 512), sr);
        }
        assert!(t.reading.is_none());
    }
}
