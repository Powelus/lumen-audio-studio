// SPDX-License-Identifier: GPL-3.0-or-later
//! The window of a sampler channel: its sample as a waveform with the start and end
//! markers to drag, and knobs for pitch, envelope and filter. Edits go to the channel's
//! settings through undo, like the other windows.

use crate::app::App;
use crate::theme::pal;
use crate::widgets::{knob, note_name};
use eframe::egui::{self, pos2, vec2, Rect, RichText, Sense, Stroke, Ui};
use las_engine::factory::{render_factory, Audio};
use las_engine::library::decode_wav;
use serde_json::{json, Value};

/// Minimum and maximum of the sample in each of `cols` columns, for drawing.
pub fn peaks(a: &Audio, cols: usize) -> Vec<(f32, f32)> {
    let n = a.frames();
    if n == 0 || cols == 0 {
        return vec![(0.0, 0.0); cols];
    }
    (0..cols)
        .map(|c| {
            let (from, to) = (
                c * n / cols,
                ((c + 1) * n / cols).max(c * n / cols + 1).min(n),
            );
            let mut lo = 0f32;
            let mut hi = 0f32;
            for ch in &a.channels {
                for &v in &ch[from..to] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            (lo, hi)
        })
        .collect()
}

/// A number of a state, or a default.
pub fn num(state: &Value, key: &str, default: f64) -> f64 {
    state.get(key).and_then(Value::as_f64).unwrap_or(default)
}

/// Set a key of a channel's state, making the state an object if it is not one.
pub fn set_key(state: &mut Value, key: &str, v: Value) {
    if !state.is_object() {
        *state = json!({});
    }
    state[key] = v;
}

/// One knob of the window: what it sets, its range, and how its value is shown.
struct Knob {
    key: &'static str,
    label: &'static str,
    lo: f64,
    hi: f64,
    default: f64,
    value: f64,
    log: bool,
    show: fn(f64) -> String,
}

impl App {
    /// The channel's sample as audio, if it has one the app can decode.
    fn sample_audio(&self, channel: usize) -> Option<Audio> {
        let l = self.project.loaded.as_ref()?;
        let id = l.project.channels.get(channel)?.sample_id.clone()?;
        if let Some(a) = render_factory(&id, 44100.0) {
            return Some(a);
        }
        decode_wav(&l.samples.get(&id)?.data)
    }

    pub(crate) fn sampler_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            ui.label("This channel no longer exists.");
            return;
        };
        let state = c.state.clone();
        let name = c.name.clone();
        let sample = c.sample_id.clone().unwrap_or_default();
        let p = pal();
        ui.label(
            RichText::new(&name)
                .strong()
                .size(16.0)
                .color(p.accent_text),
        );
        ui.label(
            RichText::new(if sample.is_empty() {
                "No sample".to_string()
            } else {
                format!("Sample: {sample}")
            })
            .weak()
            .size(11.0),
        );
        let mut changes: Vec<(&'static str, Value)> = Vec::new();

        // the waveform with its start and end markers
        let audio = self.sample_audio(channel);
        let (rect, resp) = ui.allocate_exact_size(
            vec2(ui.available_width().max(360.0), 120.0),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, p.well_deep);
        let (mut start, mut end) = (num(&state, "start", 0.0), num(&state, "end", 1.0));
        if let Some(a) = &audio {
            let cols = (rect.width() as usize).max(1);
            let mid = rect.center().y;
            let half = rect.height() * 0.45;
            for (i, (lo, hi)) in peaks(a, cols).into_iter().enumerate() {
                let x = rect.left() + i as f32;
                painter.line_segment(
                    [pos2(x, mid - hi * half), pos2(x, mid - lo * half)],
                    Stroke::new(1.0, p.accent.gamma_multiply(0.9)),
                );
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "no waveform",
                egui::FontId::proportional(12.0),
                p.muted,
            );
        }
        let x_of = |f: f64| rect.left() + f as f32 * rect.width();
        // what is cut off at either end is dimmed
        painter.rect_filled(
            Rect::from_min_max(rect.min, pos2(x_of(start), rect.bottom())),
            0.0,
            egui::Color32::from_black_alpha(150),
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(x_of(end), rect.top()), rect.max),
            0.0,
            egui::Color32::from_black_alpha(150),
        );
        for f in [start, end] {
            painter.line_segment(
                [pos2(x_of(f), rect.top()), pos2(x_of(f), rect.bottom())],
                Stroke::new(2.0, p.text),
            );
        }
        if resp.dragged() || resp.clicked() {
            if let Some(q) = resp.interact_pointer_pos() {
                let f = f64::from(((q.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
                // the marker nearer to the pointer moves
                if (f - start).abs() <= (f - end).abs() {
                    start = f.min(end - 0.001);
                    changes.push(("start", json!(start)));
                } else {
                    end = f.max(start + 0.001);
                    changes.push(("end", json!(end)));
                }
            }
        }
        resp.on_hover_text("Drag the start and end markers");
        ui.add_space(6.0);

        // knobs
        let knobs: Vec<Knob> = vec![
            Knob {
                key: "tune",
                label: "Coarse",
                lo: -12.0,
                hi: 12.0,
                default: 0.0,
                value: num(&state, "tune", 0.0),
                log: false,
                show: |v| format!("{v:+.0} st"),
            },
            Knob {
                key: "fine",
                label: "Fine",
                lo: -100.0,
                hi: 100.0,
                default: 0.0,
                value: num(&state, "fine", 0.0),
                log: false,
                show: |v| format!("{v:+.0} ct"),
            },
            Knob {
                key: "attack",
                label: "Attack",
                lo: 0.001,
                hi: 2.0,
                default: 0.001,
                value: num(&state, "attack", 0.001),
                log: true,
                show: |v| format!("{:.0} ms", v * 1000.0),
            },
            Knob {
                key: "decay",
                label: "Decay",
                lo: 0.001,
                hi: 5.0,
                default: 0.3,
                value: num(&state, "decay", 0.3),
                log: true,
                show: |v| format!("{:.0} ms", v * 1000.0),
            },
            Knob {
                key: "sustain",
                label: "Sustain",
                lo: 0.0,
                hi: 1.0,
                default: 1.0,
                value: num(&state, "sustain", 1.0),
                log: false,
                show: |v| format!("{:.0}%", v * 100.0),
            },
            Knob {
                key: "release",
                label: "Release",
                lo: 0.002,
                hi: 5.0,
                default: 0.08,
                value: num(&state, "release", 0.08),
                log: true,
                show: |v| format!("{:.0} ms", v * 1000.0),
            },
            Knob {
                key: "velToVol",
                label: "Velocity",
                lo: 0.0,
                hi: 1.0,
                default: 0.7,
                value: num(&state, "velToVol", 0.7),
                log: false,
                show: |v| format!("{:.0}%", v * 100.0),
            },
            Knob {
                key: "cutoff",
                label: "Cutoff",
                lo: 20.0,
                hi: 20000.0,
                default: 20000.0,
                value: num(&state, "cutoff", 20000.0),
                log: true,
                show: |v| format!("{v:.0} Hz"),
            },
            Knob {
                key: "res",
                label: "Resonance",
                lo: 0.1,
                hi: 18.0,
                default: 0.7,
                value: num(&state, "res", 0.7),
                log: true,
                show: |v| format!("{v:.1}"),
            },
            Knob {
                key: "boost",
                label: "Boost",
                lo: -12.0,
                hi: 12.0,
                default: 0.0,
                value: num(&state, "boost", 0.0),
                log: false,
                show: |v| format!("{v:+.1} dB"),
            },
        ];
        ui.horizontal_wrapped(|ui| {
            for k in knobs {
                let mut v = k.value;
                if knob(ui, &mut v, k.lo..=k.hi, k.default, k.label, k.log, &k.show) {
                    changes.push((k.key, json!(v)));
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let root = num(&state, "root", 60.0).round().clamp(0.0, 127.0) as i32;
            let mut r = root;
            ui.label("Root key");
            if ui
                .add(
                    egui::DragValue::new(&mut r)
                        .range(0..=127)
                        .custom_formatter(|n, _| note_name(n as u8)),
                )
                .changed()
            {
                changes.push(("root", json!(r)));
            }
            for (key, label) in [
                ("loop", "Loop"),
                ("reverse", "Reverse"),
                ("cut", "Cut itself"),
            ] {
                let mut on = state.get(key).and_then(Value::as_bool).unwrap_or(false);
                if ui.checkbox(&mut on, label).changed() {
                    changes.push((key, json!(on)));
                }
            }
            let mut kind = state
                .get("filterType")
                .and_then(Value::as_str)
                .unwrap_or("lowpass")
                .to_string();
            egui::ComboBox::from_id_salt("sampler-filter")
                .selected_text(match kind.as_str() {
                    "highpass" => "High pass",
                    "bandpass" => "Band pass",
                    _ => "Low pass",
                })
                .show_ui(ui, |ui| {
                    for (k, n) in [
                        ("lowpass", "Low pass"),
                        ("highpass", "High pass"),
                        ("bandpass", "Band pass"),
                    ] {
                        if ui.selectable_value(&mut kind, k.to_string(), n).changed() {
                            changes.push(("filterType", json!(k)));
                        }
                    }
                });
        });
        if !changes.is_empty() {
            self.project.modify_coalesced("sampler", move |pr| {
                if let Some(ch) = pr.channels.get_mut(channel) {
                    for (k, v) in changes {
                        set_key(&mut ch.state, k, v);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_cover_the_sample_in_columns() {
        let a = Audio {
            channels: vec![vec![0.0, 0.5, -0.25, 1.0, 0.0, 0.0, -1.0, 0.0]],
            sample_rate: 8000.0,
        };
        let p = peaks(&a, 4);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], (0.0, 0.5));
        assert_eq!(p[1], (-0.25, 1.0));
        assert_eq!(p[3], (-1.0, 0.0));
        // more columns than frames never reads past the end
        assert_eq!(peaks(&a, 20).len(), 20);
        assert_eq!(
            peaks(
                &Audio {
                    channels: vec![],
                    sample_rate: 1.0
                },
                3
            ),
            vec![(0.0, 0.0); 3]
        );
    }

    #[test]
    fn settings_are_read_and_written_by_key() {
        let mut s = Value::Null;
        set_key(&mut s, "cutoff", json!(800.0));
        assert_eq!(num(&s, "cutoff", 1.0), 800.0);
        assert_eq!(num(&s, "res", 0.7), 0.7);
        set_key(&mut s, "loop", json!(true));
        assert_eq!(s["loop"], true);
    }

    #[test]
    fn a_sampler_with_the_settings_of_the_window_sounds_different() {
        // the keys the window writes are the ones the engine reads: a low cutoff takes the
        // top off a hat, and a later start skips its attack
        use las_project::create::default_project;
        let render = |state: Value| {
            let mut p = default_project();
            let hat = p.channels[2].id.clone();
            p.channels[2].state = state;
            p.patterns[0].notes.retain(|k, _| *k == hat);
            las_engine::render_mix_with(
                &p,
                &Default::default(),
                &las_engine::RenderOptions {
                    sample_rate: 24000.0,
                    tail: 0.1,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let energy = |r: &las_engine::Rendered| r.left.iter().map(|s| s * s).sum::<f32>();
        let bright = render(json!({}));
        let dull = render(json!({ "cutoff": 300.0 }));
        let late = render(json!({ "start": 0.8 }));
        assert!(
            energy(&dull) < energy(&bright) * 0.5,
            "{} vs {}",
            energy(&dull),
            energy(&bright)
        );
        assert!(energy(&late) < energy(&bright) * 0.5);
    }
}
