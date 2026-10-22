// SPDX-License-Identifier: GPL-3.0-or-later
//! Editing an audio clip, as FL Studio's audio clip options do: fade in and out, gain,
//! reverse, normalize, time stretch to the project tempo and pitch shift. The edits are
//! kept in the clip (non-destructive) and the engine applies them when it plays.

use eframe::egui::{self, RichText, Ui};
use las_project::{AudioClipEdit, Stretch, StretchMode, PPQ};

/// What the panel did this frame.
#[derive(Default)]
pub struct PanelOut {
    /// The new edits, when something changed.
    pub edit: Option<AudioClipEdit>,
    /// A gesture started: the change before it can be undone.
    pub begin: bool,
}

/// Whether an edit does nothing at all (the clip then keeps no edit).
pub fn is_plain(e: &AudioClipEdit) -> bool {
    *e == AudioClipEdit::default()
}

/// The tempo a loop of `seconds` was most likely made at: a whole number of beats
/// (1, 2, 4 ... 128) that gives a tempo between 70 and 180 BPM, the one nearest the
/// project's tempo. `None` for a sound too short or too long to be a loop.
pub fn guess_bpm(seconds: f64, project_bpm: f64) -> Option<f64> {
    if seconds.is_nan() || seconds <= 0.1 {
        return None;
    }
    let mut best: Option<f64> = None;
    for beats in [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0] {
        let bpm = beats * 60.0 / seconds;
        if !(70.0..=180.0).contains(&bpm) {
            continue;
        }
        let closer = best.is_none_or(|b: f64| (bpm - project_bpm).abs() < (b - project_bpm).abs());
        if closer {
            best = Some(bpm);
        }
    }
    best.map(|b| (b * 100.0).round() / 100.0)
}

/// How long a clip plays its sample for, in ticks, once it follows the project tempo:
/// the sample's length in beats at its own tempo.
pub fn stretched_length(seconds: f64, sample_bpm: f64) -> f64 {
    seconds * sample_bpm / 60.0 * f64::from(PPQ)
}

/// Start an undo step when a widget starts changing (a drag starts, or a click).
fn begun(r: &egui::Response) -> bool {
    r.drag_started() || (r.changed() && !r.dragged())
}

/// The controls of an audio clip's edits. `seconds` is the length of its sample, `bpm`
/// the project's tempo, `clip_len` the clip's length in ticks (fades cannot be longer).
pub fn panel(
    ui: &mut Ui,
    edit: &AudioClipEdit,
    seconds: Option<f64>,
    bpm: f64,
    clip_len: f64,
) -> PanelOut {
    let mut out = PanelOut::default();
    let mut e = edit.clone();
    let beat = f64::from(PPQ);
    let max_beats = (clip_len / beat).max(0.25);
    egui::Grid::new("clip-edit")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            // fades, in beats
            for (label, tip, is_in) in [
                (
                    "Fade in",
                    "How long the clip takes to rise from silence (also: drag the small square at the clip's top left corner)",
                    true,
                ),
                (
                    "Fade out",
                    "How long the clip takes to fall to silence (also: drag the small square at the clip's top right corner)",
                    false,
                ),
            ] {
                ui.label(label).on_hover_text(tip);
                let slot = if is_in { &mut e.fade_in } else { &mut e.fade_out };
                let mut beats = slot.unwrap_or(0.0) / beat;
                let r = ui
                    .add(
                        egui::Slider::new(&mut beats, 0.0..=max_beats)
                            .suffix(" beats")
                            .max_decimals(2),
                    )
                    .on_hover_text(tip);
                out.begin |= begun(&r);
                if r.changed() {
                    *slot = (beats > 1e-3).then_some(beats * beat);
                }
                ui.end_row();
            }
            ui.label("Gain").on_hover_text("Make the clip louder or quieter");
            ui.horizontal(|ui| {
                let mut db = e.gain.unwrap_or(0.0);
                let r = ui
                    .add(egui::Slider::new(&mut db, -24.0..=24.0).suffix(" dB").max_decimals(1))
                    .on_hover_text("Double click to reset to 0 dB");
                out.begin |= begun(&r);
                if r.double_clicked() {
                    db = 0.0;
                    out.begin = true;
                }
                if r.changed() || r.double_clicked() {
                    e.gain = (db.abs() > 0.05).then_some(db);
                }
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                let mut rev = e.reverse == Some(true);
                let r = ui
                    .checkbox(&mut rev, "Reverse")
                    .on_hover_text("Play the sound backwards");
                if r.changed() {
                    out.begin = true;
                    e.reverse = rev.then_some(true);
                }
                let mut norm = e.normalize == Some(true);
                let r = ui
                    .checkbox(&mut norm, "Normalize")
                    .on_hover_text("Make the loudest moment of the sound reach 0 dB (the gain applies after)");
                if r.changed() {
                    out.begin = true;
                    e.normalize = norm.then_some(true);
                }
            });
            ui.end_row();
            ui.label("Pitch").on_hover_text(
                "Shift the pitch in semitones; the timing stays the same (time stretched)",
            );
            ui.horizontal(|ui| {
                let mut st = e.pitch.unwrap_or(0.0);
                let r = ui
                    .add(egui::Slider::new(&mut st, -24.0..=24.0).suffix(" st").step_by(1.0))
                    .on_hover_text("Double click to reset");
                out.begin |= begun(&r);
                if r.double_clicked() {
                    st = 0.0;
                    out.begin = true;
                }
                if r.changed() || r.double_clicked() {
                    e.pitch = (st.abs() > 1e-3).then_some(st);
                }
            });
            ui.end_row();
            ui.label("Tempo").on_hover_text(
                "Time stretch the sound so it follows the project's tempo, keeping its pitch",
            );
            ui.vertical(|ui| {
                let mut follow = e.stretch.is_some();
                let r = ui
                    .checkbox(&mut follow, "Stretch to the project tempo")
                    .on_hover_text("The sound plays faster or slower so its beats line up with the project's");
                if r.changed() {
                    out.begin = true;
                    e.stretch = follow.then(|| Stretch {
                        bpm: seconds.and_then(|s| guess_bpm(s, bpm)).unwrap_or(bpm),
                        mode: StretchMode::Stretch,
                    });
                }
                if let Some(s) = &mut e.stretch {
                    ui.horizontal(|ui| {
                        ui.label("The sample's own tempo");
                        let r = ui
                            .add(
                                egui::DragValue::new(&mut s.bpm)
                                    .range(20.0..=400.0)
                                    .speed(0.1)
                                    .suffix(" BPM"),
                            )
                            .on_hover_text("The tempo the sound was made at; drag or type it");
                        out.begin |= begun(&r);
                        if let Some(g) = seconds.and_then(|sec| guess_bpm(sec, bpm)) {
                            if ui
                                .small_button("Guess")
                                .on_hover_text(format!(
                                    "Assume the sound is a whole number of beats long: {g} BPM"
                                ))
                                .clicked()
                            {
                                out.begin = true;
                                s.bpm = g;
                            }
                        }
                    });
                    let mut resample = s.mode == StretchMode::Resample;
                    let r = ui
                        .checkbox(&mut resample, "Resample (pitch follows the speed)")
                        .on_hover_text("Off: the pitch stays the same. On: like a record played faster or slower.");
                    if r.changed() {
                        out.begin = true;
                        s.mode = if resample {
                            StretchMode::Resample
                        } else {
                            StretchMode::Stretch
                        };
                    }
                    ui.label(
                        RichText::new(format!("Project tempo: {bpm:.2} BPM"))
                            .weak()
                            .size(11.0),
                    );
                }
            });
            ui.end_row();
        });
    if ui
        .add_enabled(!is_plain(&e), egui::Button::new("Reset all clip edits"))
        .clicked()
    {
        out.begin = true;
        e = AudioClipEdit::default();
    }
    if e != *edit {
        out.edit = Some(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loop_tempo_is_guessed_from_its_length() {
        // two seconds is 4 beats at 120 or 2 at 60 (too slow): 120
        assert_eq!(guess_bpm(2.0, 120.0), Some(120.0));
        // 8 beats in 3.75 s is 128 BPM
        assert_eq!(guess_bpm(3.75, 128.0), Some(128.0));
        // near a project at 90, 1.5 s is 4 beats at 160 or 2 beats at 80: 80 is nearer
        assert_eq!(guess_bpm(1.5, 90.0), Some(80.0));
        assert_eq!(guess_bpm(0.0, 120.0), None);
        assert_eq!(guess_bpm(f64::NAN, 120.0), None);
    }

    #[test]
    fn a_stretched_clip_lasts_its_beats() {
        // two seconds at 120 BPM is 4 beats
        assert_eq!(stretched_length(2.0, 120.0), 4.0 * f64::from(PPQ));
    }

    #[test]
    fn an_edit_with_nothing_set_is_plain() {
        assert!(is_plain(&AudioClipEdit::default()));
        assert!(!is_plain(&AudioClipEdit {
            reverse: Some(true),
            ..Default::default()
        }));
    }
}
