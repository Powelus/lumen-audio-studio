// SPDX-License-Identifier: GPL-3.0-or-later
//! The analyser window: what the master output sounds like, drawn four ways.
//!
//! - A heat map: a scrolling spectrogram, low notes at the bottom, louder is hotter.
//! - A spectrum: the level of each frequency now, with a peak line that falls slowly.
//! - A goniometer: left against right, so a mono sound is a vertical line and a wide
//!   one a cloud; with the correlation (+1 mono, 0 wide, -1 out of phase) and balance.
//! - Loudness: momentary, short term and integrated LUFS, and the left and right peaks.
//!
//! The audio comes from the output's scope (see [`crate::audio::Scope`]); without an
//! audio device, `--analyser-demo` plays the project into it silently instead.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{
    self, pos2, vec2, Color32, ColorImage, Mesh, Pos2, Rect, RichText, Sense, Shape, Stroke,
    TextureHandle, TextureOptions, Ui,
};
use las_dsp::loudness::LoudnessMeter;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

/// Frames in one analysis.
const FFT_N: usize = 4096;
/// Columns of history in the heat map, and rows (frequency bands).
const HEAT_W: usize = 480;
const HEAT_H: usize = 240;
/// The lowest and highest frequencies shown.
const LOW: f32 = 20.0;
const HIGH: f32 = 20_000.0;
/// The quietest level shown, dB.
const FLOOR: f32 = -96.0;
/// Bands of the spectrum curve.
const BANDS: usize = 160;

/// The "magma" palette: black through purple and red to pale yellow.
const MAGMA: [(f32, [u8; 3]); 7] = [
    (0.0, [0, 0, 4]),
    (0.15, [28, 16, 68]),
    (0.35, [100, 26, 128]),
    (0.55, [183, 55, 121]),
    (0.72, [241, 96, 93]),
    (0.88, [254, 176, 120]),
    (1.0, [252, 253, 191]),
];

/// The heat map's colour for a level from 0 to 1.
pub fn magma(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let i = MAGMA
        .iter()
        .position(|(at, _)| *at >= t)
        .unwrap_or(MAGMA.len() - 1);
    if i == 0 {
        let c = MAGMA[0].1;
        return Color32::from_rgb(c[0], c[1], c[2]);
    }
    let ((a, ca), (b, cb)) = (MAGMA[i - 1], MAGMA[i]);
    let w = (t - a) / (b - a).max(1e-6);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * w).round() as u8;
    Color32::from_rgb(mix(ca[0], cb[0]), mix(ca[1], cb[1]), mix(ca[2], cb[2]))
}

/// A level in dB as 0 (the floor) to 1 (0 dBFS).
pub fn unit(db: f32) -> f32 {
    ((db - FLOOR) / -FLOOR).clamp(0.0, 1.0)
}

/// The frequency at a place from 0 (low) to 1 (high) on a log scale.
pub fn freq_at(t: f32) -> f32 {
    LOW * (HIGH / LOW).powf(t)
}

/// Where a frequency sits from 0 to 1 on a log scale.
pub fn place_of(f: f32) -> f32 {
    ((f / LOW).ln() / (HIGH / LOW).ln()).clamp(0.0, 1.0)
}

/// The spectrum of the newest `FFT_N` frames of mid (left plus right), in dBFS per
/// FFT bin: a full scale sine reads about 0 dB.
#[allow(clippy::needless_range_loop)]
pub fn spectrum(l: &[f32], r: &[f32]) -> Vec<f32> {
    let n = FFT_N;
    let (mut re, mut im) = (vec![0.0f64; n], vec![0.0f64; n]);
    let have = l.len().min(r.len()).min(n);
    let (l, r) = (&l[l.len() - have..], &r[r.len() - have..]);
    let mut wsum = 0.0;
    for i in 0..n {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
        wsum += w;
        let k = i as isize - (n - have) as isize;
        if k >= 0 {
            let k = k as usize;
            re[i] = f64::from(l[k] + r[k]) * 0.5 * w;
        }
    }
    las_dsp::fft::fft(&mut re, &mut im, false);
    (0..n / 2)
        .map(|i| {
            let mag = (re[i] * re[i] + im[i] * im[i]).sqrt() * 2.0 / wsum;
            (20.0 * mag.max(1e-12).log10()) as f32
        })
        .collect()
}

/// Bins grouped into `bands` on a log scale: the loudest bin of each band, or the
/// level between bins where a band is narrower than a bin.
pub fn bands(bins: &[f32], sr: f32, bands: usize) -> Vec<f32> {
    let hz = sr / (2.0 * bins.len() as f32);
    (0..bands)
        .map(|b| {
            let (f0, f1) = (
                freq_at(b as f32 / bands as f32),
                freq_at((b + 1) as f32 / bands as f32),
            );
            let (a, z) = (f0 / hz, f1 / hz);
            let (ai, zi) = (a.floor() as usize, (z.ceil() as usize).min(bins.len() - 1));
            if zi <= ai + 1 {
                // narrower than a bin: between the two nearest
                let x = ((a + z) * 0.5).min((bins.len() - 2) as f32);
                let (i, w) = (x.floor() as usize, x.fract());
                bins[i] + (bins[i + 1] - bins[i]) * w
            } else {
                bins[ai..=zi].iter().copied().fold(FLOOR, f32::max)
            }
        })
        .collect()
}

/// Correlation of left and right: +1 the same, 0 unrelated, -1 opposite.
pub fn correlation(l: &[f32], r: &[f32]) -> f32 {
    let (mut lr, mut ll, mut rr) = (0.0f64, 0.0f64, 0.0f64);
    for (&a, &b) in l.iter().zip(r) {
        lr += f64::from(a * b);
        ll += f64::from(a * a);
        rr += f64::from(b * b);
    }
    if ll * rr < 1e-12 {
        return 0.0;
    }
    (lr / (ll * rr).sqrt()) as f32
}

/// Where a frame is drawn in the goniometer, around (0, 0) in a unit circle: mid up,
/// side across (left to the left).
pub fn gonio(l: f32, r: f32) -> (f32, f32) {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    ((r - l) * s, (l + r) * s)
}

/// What the analyser keeps between frames.
pub struct Analyser {
    read: u64,
    l: VecDeque<f32>,
    r: VecDeque<f32>,
    heat: ColorImage,
    texture: Option<TextureHandle>,
    fresh: bool,
    curve: Vec<f32>,
    peaks: Vec<f32>,
    loudness: LoudnessMeter,
    sr: f32,
    corr: f32,
    balance: f32,
    peak_l: f32,
    peak_r: f32,
    /// A render of the project fed in at playing speed, when there is no audio device.
    pub(crate) demo: Option<(Arc<las_engine::render::Rendered>, Instant, usize)>,
    /// That render while it is being made.
    pending: Option<std::sync::mpsc::Receiver<las_engine::render::Rendered>>,
}

impl Analyser {
    pub fn new(sr: f32) -> Self {
        Self {
            read: 0,
            l: VecDeque::new(),
            r: VecDeque::new(),
            heat: ColorImage::filled([HEAT_W, HEAT_H], magma(0.0)),
            texture: None,
            fresh: true,
            curve: vec![FLOOR; BANDS],
            peaks: vec![FLOOR; BANDS],
            loudness: LoudnessMeter::new(f64::from(sr), 2),
            sr,
            corr: 0.0,
            balance: 0.0,
            peak_l: FLOOR,
            peak_r: FLOOR,
            demo: None,
            pending: None,
        }
    }

    /// Take new frames.
    pub fn feed(&mut self, l: &[f32], r: &[f32]) {
        if l.is_empty() {
            return;
        }
        self.loudness.process(&[l, r], l.len().min(r.len()));
        self.l.extend(l);
        self.r.extend(r);
        let keep = FFT_N * 2;
        while self.l.len() > keep {
            self.l.pop_front();
            self.r.pop_front();
        }
        self.fresh = true;
    }

    /// Work out the views from the frames held, once a frame.
    fn update(&mut self) {
        if !self.fresh {
            return;
        }
        self.fresh = false;
        let (l, r): (Vec<f32>, Vec<f32>) = (
            self.l.iter().copied().collect(),
            self.r.iter().copied().collect(),
        );
        let bins = spectrum(&l, &r);
        let now = bands(&bins, self.sr, BANDS);
        for ((c, p), &v) in self.curve.iter_mut().zip(&mut self.peaks).zip(&now) {
            // rise at once, fall smoothly
            *c = if v > *c { v } else { *c + (v - *c) * 0.25 };
            *p = if v > *p { v } else { *p - 0.4 };
        }
        // scroll the heat map by a column and draw the newest on the right
        let w = HEAT_W;
        for y in 0..HEAT_H {
            let row = &mut self.heat.pixels[y * w..(y + 1) * w];
            row.copy_within(1.., 0);
        }
        let column = bands(&bins, self.sr, HEAT_H);
        for (y, v) in column.iter().enumerate() {
            // low frequencies at the bottom
            self.heat.pixels[(HEAT_H - 1 - y) * w + w - 1] = magma(unit(*v).powf(1.4));
        }
        let tail = l.len().saturating_sub(2048);
        let (sl, sr) = (&l[tail..], &r[tail..]);
        self.corr += (correlation(sl, sr) - self.corr) * 0.2;
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
        let (a, b) = (rms(sl), rms(sr));
        if a + b > 1e-6 {
            self.balance += ((b - a) / (a + b) - self.balance) * 0.2;
        }
        let peak = |x: &[f32]| 20.0 * x.iter().fold(1e-9f32, |m, v| m.max(v.abs())).log10();
        let tail = l.len().saturating_sub(1024);
        self.peak_l = peak(&l[tail..]).max(self.peak_l - 0.8);
        self.peak_r = peak(&r[tail..]).max(self.peak_r - 0.8);
    }
}

fn label(ui: &Ui, at: Pos2, text: &str, align: egui::Align2) {
    ui.painter().text(
        at,
        align,
        text,
        egui::FontId::proportional(10.0),
        pal().muted,
    );
}

/// The box a view draws in, with a dark well.
fn well(ui: &mut Ui, size: egui::Vec2) -> Rect {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, 6.0, Color32::from_rgb(10, 10, 16));
    rect
}

fn freq_marks(ui: &Ui, rect: Rect, vertical: bool) {
    for (f, t) in [
        (50.0, "50"),
        (100.0, "100"),
        (500.0, "500"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (10_000.0, "10k"),
    ] {
        let p = place_of(f);
        if vertical {
            let y = rect.bottom() - p * rect.height();
            label(ui, pos2(rect.left() + 4.0, y), t, egui::Align2::LEFT_CENTER);
        } else {
            let x = rect.left() + p * rect.width();
            ui.painter().line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(14)),
            );
            label(
                ui,
                pos2(x + 3.0, rect.bottom() - 4.0),
                t,
                egui::Align2::LEFT_BOTTOM,
            );
        }
    }
}

impl Analyser {
    fn heat_view(&mut self, ui: &mut Ui, size: egui::Vec2) {
        let rect = well(ui, size);
        let tex = self.texture.get_or_insert_with(|| {
            ui.ctx()
                .load_texture("analyser-heat", self.heat.clone(), TextureOptions::LINEAR)
        });
        tex.set(self.heat.clone(), TextureOptions::LINEAR);
        let inner = rect.shrink(2.0);
        ui.painter().image(
            tex.id(),
            inner,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        freq_marks(ui, inner, true);
        label(
            ui,
            pos2(inner.right() - 6.0, inner.top() + 4.0),
            "HEAT MAP",
            egui::Align2::RIGHT_TOP,
        );
    }

    fn spectrum_view(&self, ui: &mut Ui, size: egui::Vec2) {
        let rect = well(ui, size);
        let inner = rect.shrink(4.0);
        let p = pal();
        // dB grid
        for db in [-12.0, -24.0, -48.0, -72.0] {
            let y = inner.bottom() - unit(db) * inner.height();
            ui.painter().line_segment(
                [pos2(inner.left(), y), pos2(inner.right(), y)],
                Stroke::new(1.0, Color32::from_white_alpha(10)),
            );
            label(
                ui,
                pos2(inner.right() - 2.0, y - 1.0),
                &format!("{db}"),
                egui::Align2::RIGHT_BOTTOM,
            );
        }
        freq_marks(ui, inner, false);
        let at = |i: usize, v: f32| {
            pos2(
                inner.left() + (i as f32 + 0.5) / BANDS as f32 * inner.width(),
                inner.bottom() - unit(v) * inner.height(),
            )
        };
        // a filled shape that fades towards the bottom
        let (top, bottom) = (p.accent, p.accent.gamma_multiply(0.05));
        let mut mesh = Mesh::default();
        for (i, &v) in self.curve.iter().enumerate() {
            let q = at(i, v);
            mesh.colored_vertex(q, top.gamma_multiply(0.55));
            mesh.colored_vertex(pos2(q.x, inner.bottom()), bottom);
            if i > 0 {
                let k = (i * 2) as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k + 1, k);
            }
        }
        ui.painter().add(Shape::mesh(mesh));
        let line: Vec<Pos2> = self
            .curve
            .iter()
            .enumerate()
            .map(|(i, &v)| at(i, v))
            .collect();
        ui.painter()
            .add(Shape::line(line, Stroke::new(1.8, p.accent)));
        let peaks: Vec<Pos2> = self
            .peaks
            .iter()
            .enumerate()
            .map(|(i, &v)| at(i, v))
            .collect();
        ui.painter().add(Shape::line(
            peaks,
            Stroke::new(1.0, Color32::from_white_alpha(90)),
        ));
        label(
            ui,
            pos2(inner.left() + 4.0, inner.top() + 2.0),
            "SPECTRUM",
            egui::Align2::LEFT_TOP,
        );
    }

    fn stereo_view(&self, ui: &mut Ui, side: f32) {
        let rect = well(ui, vec2(side, side + 44.0));
        let p = pal();
        let scope = Rect::from_min_size(rect.min, vec2(side, side)).shrink(10.0);
        let c = scope.center();
        let rad = scope.width() * 0.5;
        let faint = Stroke::new(1.0, Color32::from_white_alpha(18));
        ui.painter().circle_stroke(c, rad, faint);
        ui.painter().circle_stroke(c, rad * 0.5, faint);
        let d = rad * std::f32::consts::FRAC_1_SQRT_2;
        ui.painter()
            .line_segment([c + vec2(-d, -d), c + vec2(d, d)], faint);
        ui.painter()
            .line_segment([c + vec2(d, -d), c + vec2(-d, d)], faint);
        ui.painter()
            .line_segment([c + vec2(0.0, -rad), c + vec2(0.0, rad)], faint);
        label(
            ui,
            c + vec2(-d - 8.0, -d - 8.0),
            "L",
            egui::Align2::CENTER_CENTER,
        );
        label(
            ui,
            c + vec2(d + 8.0, -d - 8.0),
            "R",
            egui::Align2::CENTER_CENTER,
        );
        label(
            ui,
            c + vec2(0.0, -rad - 6.0),
            "M",
            egui::Align2::CENTER_CENTER,
        );
        // the newest frames, older ones fainter
        let n = self.l.len().min(2048);
        let start = self.l.len() - n;
        for k in (0..n).step_by(2) {
            let (x, y) = gonio(self.l[start + k], self.r[start + k]);
            let age = k as f32 / n as f32;
            let q = c + vec2(x, -y) * rad * 0.9;
            if scope.expand(4.0).contains(q) {
                let col = p.accent.gamma_multiply(0.15 + 0.6 * age);
                ui.painter().circle_filled(q, 1.1, col);
            }
        }
        // correlation and balance bars
        let bar = |ui: &Ui, y: f32, v: f32, name: &str, good: bool| {
            let r = Rect::from_min_size(pos2(rect.left() + 34.0, y), vec2(side - 44.0, 8.0));
            ui.painter()
                .rect_filled(r, 3.0, Color32::from_white_alpha(14));
            ui.painter().line_segment(
                [
                    pos2(r.center().x, r.top() - 2.0),
                    pos2(r.center().x, r.bottom() + 2.0),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(60)),
            );
            let x = r.center().x + v.clamp(-1.0, 1.0) * r.width() * 0.5;
            let col = if good { p.ok } else { p.hot };
            ui.painter().circle_filled(pos2(x, r.center().y), 5.0, col);
            label(
                ui,
                pos2(rect.left() + 6.0, r.center().y),
                name,
                egui::Align2::LEFT_CENTER,
            );
        };
        bar(
            ui,
            rect.top() + side + 6.0,
            self.corr,
            "Corr",
            self.corr >= 0.0,
        );
        bar(
            ui,
            rect.top() + side + 26.0,
            self.balance,
            "Bal",
            self.balance.abs() < 0.3,
        );
    }

    fn loudness_view(&self, ui: &mut Ui, size: egui::Vec2) {
        let rect = well(ui, size);
        let p = pal();
        let inner = rect.shrink(10.0);
        let show = |v: f64| {
            if v.is_finite() && v > -70.0 {
                format!("{v:.1}")
            } else {
                "-inf".into()
            }
        };
        let m = self.loudness.momentary();
        let painter = ui.painter();
        painter.text(
            inner.left_top(),
            egui::Align2::LEFT_TOP,
            show(m),
            egui::FontId::monospace(30.0),
            p.text,
        );
        painter.text(
            inner.left_top() + vec2(0.0, 36.0),
            egui::Align2::LEFT_TOP,
            "LUFS momentary",
            egui::FontId::proportional(10.0),
            p.muted,
        );
        for (k, (name, v)) in [
            ("Short term", self.loudness.short_term()),
            ("Integrated", self.loudness.integrated()),
            ("True peak", self.loudness.true_peak_db()),
        ]
        .iter()
        .enumerate()
        {
            let y = inner.top() + 60.0 + k as f32 * 20.0;
            painter.text(
                pos2(inner.left(), y),
                egui::Align2::LEFT_TOP,
                name,
                egui::FontId::proportional(11.0),
                p.muted,
            );
            painter.text(
                pos2(inner.left() + 110.0, y),
                egui::Align2::RIGHT_TOP,
                show(*v),
                egui::FontId::monospace(12.0),
                p.text,
            );
        }
        // peak meters: green, then yellow, then red near the top
        let mh = inner.height();
        for (k, v) in [self.peak_l, self.peak_r].iter().enumerate() {
            let x = inner.right() - 34.0 + k as f32 * 18.0;
            let r = Rect::from_min_size(pos2(x, inner.top()), vec2(12.0, mh));
            painter.rect_filled(r, 2.0, Color32::from_white_alpha(12));
            let segs = 30;
            let lit = (unit(*v).powf(2.0) * segs as f32).round() as usize;
            for s in 0..lit.min(segs) {
                let t = s as f32 / segs as f32;
                let col = if t > 0.9 {
                    p.hot
                } else if t > 0.75 {
                    p.warn
                } else {
                    p.ok
                };
                let h = mh / segs as f32;
                let seg = Rect::from_min_size(
                    pos2(x, r.bottom() - (s + 1) as f32 * h + 1.0),
                    vec2(12.0, h - 1.5),
                );
                painter.rect_filled(seg, 1.0, col);
            }
            label(
                ui,
                pos2(x + 6.0, r.bottom() + 8.0),
                ["L", "R"][k],
                egui::Align2::CENTER_CENTER,
            );
        }
    }

    /// The whole window.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.update();
        let w = ui.available_width();
        let h = ui.available_height().max(360.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        self.heat_view(ui, vec2(w, (h * 0.48).max(150.0)));
        let low = (h * 0.52 - 16.0).max(200.0);
        ui.horizontal(|ui| {
            let side = (low - 44.0).min(w * 0.3);
            let rest = w - side - 16.0;
            self.spectrum_view(ui, vec2(rest * 0.66, low));
            self.stereo_view(ui, side);
            self.loudness_view(ui, vec2((rest * 0.34).max(150.0), low));
        });
    }
}

impl App {
    /// Feed the analyser what went out since the last frame.
    pub(crate) fn feed_analyser(&mut self) {
        if !self.shell.open.analyser {
            return;
        }
        let sr = self
            .audio
            .as_ref()
            .map_or(44_100.0, |a| a.sample_rate as f32);
        let an = self.analyser.get_or_insert_with(|| Analyser::new(sr));
        let (mut l, mut r) = (Vec::new(), Vec::new());
        if let Some(song) = an.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            an.sr = song.sample_rate as f32;
            an.loudness = LoudnessMeter::new(song.sample_rate, 2);
            an.demo = Some((Arc::new(song), Instant::now(), 0));
            an.pending = None;
        }
        if let Some((song, started, at)) = &mut an.demo {
            let due = ((started.elapsed().as_secs_f64() * song.sample_rate) as usize)
                % song.left.len().max(1);
            let from = if due < *at { 0 } else { *at };
            l.extend_from_slice(&song.left[from..due]);
            r.extend_from_slice(&song.right[from..due]);
            *at = due;
        } else if let Some(a) = &self.audio {
            an.read = a.meters.scope.read_since(an.read, &mut l, &mut r);
        }
        an.feed(&l, &r);
    }

    /// Play a render of the project into the analyser, for screenshots without sound.
    pub(crate) fn analyser_demo(&mut self) {
        let Some(loaded) = &self.project.loaded else {
            return;
        };
        let (project, samples) = (loaded.project.clone(), loaded.samples.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let opts = las_engine::render::RenderOptions::default();
            if let Ok(song) = las_engine::render::render_mix_with(&project, &samples, &opts) {
                let _ = tx.send(song);
            }
        });
        let mut an = Analyser::new(44_100.0);
        an.pending = Some(rx);
        self.analyser = Some(an);
        self.shell.open.analyser = true;
    }

    pub(crate) fn analyser_window(&mut self, ctx: &egui::Context) {
        let mut open = self.shell.open.analyser;
        if !open {
            return;
        }
        let screen = ctx.content_rect();
        egui::Window::new("Analyser")
            .open(&mut open)
            .default_size([screen.width() * 0.6, screen.height() * 0.6])
            .min_size([560.0, 380.0])
            .show(ctx, |ui| {
                if let Some(a) = &mut self.analyser {
                    a.ui(ui);
                } else {
                    ui.label(RichText::new("Waiting for sound...").weak());
                }
            });
        self.shell.open.analyser = open;
        // the analyser moves while sound plays; App::next_repaint asks for that
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin() * amp)
            .collect()
    }

    #[test]
    fn a_sine_shows_at_its_frequency_and_level() {
        let x = sine(1000.0, FFT_N, 1.0);
        let bins = spectrum(&x, &x);
        let (top, at) = bins
            .iter()
            .enumerate()
            .fold((FLOOR, 0), |m, (i, &v)| if v > m.0 { (v, i) } else { m });
        let hz = at as f32 * 48_000.0 / FFT_N as f32;
        assert!((hz - 1000.0).abs() < 15.0, "{hz}");
        assert!(top.abs() < 1.5, "{top}");
        let b = bands(&bins, 48_000.0, BANDS);
        let loudest = b
            .iter()
            .enumerate()
            .fold((FLOOR, 0), |m, (i, &v)| if v > m.0 { (v, i) } else { m })
            .1;
        let f = freq_at((loudest as f32 + 0.5) / BANDS as f32);
        assert!((f / 1000.0).log2().abs() < 0.1, "{f}");
    }

    #[test]
    fn silence_is_at_the_floor_and_scales_map_both_ways() {
        let z = vec![0.0; FFT_N];
        assert!(bands(&spectrum(&z, &z), 48_000.0, 64)
            .iter()
            .all(|&v| unit(v) == 0.0));
        for f in [20.0, 440.0, 20_000.0] {
            assert!((freq_at(place_of(f)) - f).abs() < 0.01 * f);
        }
        assert_eq!(magma(0.0), Color32::from_rgb(0, 0, 4));
        assert_eq!(magma(1.0), Color32::from_rgb(252, 253, 191));
        assert_eq!(magma(2.0), magma(1.0));
    }

    #[test]
    fn mono_is_upright_and_opposite_is_across() {
        let (x, y) = gonio(0.5, 0.5);
        assert!(x.abs() < 1e-6 && y > 0.0);
        let (x, y) = gonio(0.5, -0.5);
        assert!(y.abs() < 1e-6 && x < 0.0);
        let a = sine(300.0, 4800, 0.5);
        let b: Vec<f32> = a.iter().map(|v| -v).collect();
        assert!((correlation(&a, &a) - 1.0).abs() < 1e-4);
        assert!((correlation(&a, &b) + 1.0).abs() < 1e-4);
        let c = sine(301.7, 4800, 0.5);
        let d: Vec<f32> = (0..4800)
            .map(|i| ((i * 7919 % 1000) as f32 / 500.0) - 1.0)
            .collect();
        assert!(correlation(&c, &d).abs() < 0.2);
    }

    #[test]
    fn the_analyser_keeps_a_bounded_history_and_scrolls() {
        let mut a = Analyser::new(48_000.0);
        let x = sine(440.0, 20_000, 0.5);
        a.feed(&x, &x);
        assert_eq!(a.l.len(), FFT_N * 2);
        a.update();
        let right = a.heat.pixels[(HEAT_H / 2) * HEAT_W + HEAT_W - 1];
        a.feed(&x[..512], &x[..512]);
        a.update();
        // the column moved one to the left
        assert_eq!(a.heat.pixels[(HEAT_H / 2) * HEAT_W + HEAT_W - 2], right);
        assert!(a.corr > 0.1);
        assert!(a.peak_l > -7.0);
    }
}
