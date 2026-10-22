// SPDX-License-Identifier: GPL-3.0-or-later
//! The slicer (FL Studio's Slicex): a channel that plays slice N of a loop on key
//! C5 + N, and a pattern that plays the slices in order so the loop sounds as it did,
//! at the project's tempo.

use crate::create::{add_channel, palette_color, uid};
use crate::model::*;
use serde_json::{json, Value};

/// The instrument id of slicer channels.
pub const SLICER_ID: &str = "slicer";
/// The key of the first slice (C5).
pub const FIRST_KEY: i32 = 60;
/// The most slices a slicer plays (C5 up to the top key).
pub const MAX_SLICES: usize = 68;

/// How a loop is cut.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceSpec {
    /// Where each slice after the first starts, frames, ascending (the first starts at 0).
    pub starts: Vec<usize>,
    /// The loop's length, frames.
    pub frames: usize,
    pub sample_rate: f64,
    /// The loop's own tempo, which places the slices in the pattern.
    pub bpm: f64,
}

impl SliceSpec {
    /// Every slice's start, from 0, as parts of the loop (0 to 1).
    pub fn fractions(&self) -> Vec<f64> {
        let n = self.frames.max(1) as f64;
        let mut out = vec![0.0];
        for &s in &self.starts {
            if s > 0 && s < self.frames {
                out.push(s as f64 / n);
            }
        }
        out.sort_by(f64::total_cmp);
        out.dedup();
        out.truncate(MAX_SLICES);
        out
    }

    /// The notes that play the slices in order: start and length in ticks.
    pub fn notes(&self) -> Vec<Note> {
        let f = self.fractions();
        let secs = self.frames as f64 / self.sample_rate.max(1.0);
        let ticks = |x: f64| x * secs * self.bpm.max(1.0) / 60.0 * f64::from(PPQ);
        f.iter()
            .enumerate()
            .map(|(i, &a)| {
                let z = f.get(i + 1).copied().unwrap_or(1.0);
                Note {
                    id: uid("n"),
                    key: FIRST_KEY + i as i32,
                    pos: ticks(a),
                    len: (ticks(z) - ticks(a)).max(1.0),
                    vel: 0.8,
                    ..Note::default()
                }
            })
            .collect()
    }

    /// The pattern's length: the loop rounded up to a bar.
    pub fn pattern_length(&self, time_sig: (u32, u32)) -> f64 {
        let bar = f64::from(PPQ) * 4.0 * f64::from(time_sig.0) / f64::from(time_sig.1.max(1));
        let secs = self.frames as f64 / self.sample_rate.max(1.0);
        let ticks = secs * self.bpm.max(1.0) / 60.0 * f64::from(PPQ);
        ((ticks - 0.5) / bar).ceil().max(1.0) * bar
    }
}

/// A slicer channel's settings for a cut.
pub fn slicer_state(spec: &SliceSpec, pattern_id: &str) -> Value {
    json!({
        "slices": spec.fractions(),
        "sliceBpm": spec.bpm,
        "slicePattern": pattern_id,
        "velToVol": 0.0,
        "attack": 0.001,
        "release": 0.02,
    })
}

/// Add a slicer channel playing `sample_id`, and a pattern that plays its slices in
/// order. Returns the channel's and the pattern's indexes.
pub fn add_slicer(
    p: &mut Project,
    sample_id: &str,
    name: &str,
    spec: &SliceSpec,
) -> (usize, usize) {
    let at = add_channel(p, "sampler");
    let pattern_id = uid("pat_");
    {
        let c = &mut p.channels[at];
        c.plugin_id = SLICER_ID.into();
        c.kind = "sampler".into();
        c.name = format!("{name} slices");
        c.color = "#f25fd0".into();
        c.sample_id = Some(sample_id.to_string());
        c.root_key = FIRST_KEY;
        c.state = slicer_state(spec, &pattern_id);
    }
    let insert = p.channels[at].mixer_track;
    if let Some(t) = p.mixer.get_mut(insert) {
        t.name = p.channels[at].name.clone();
    }
    let mut pat = crate::create::pattern(p.patterns.len(), Some(&format!("{name} slices")));
    pat.id = pattern_id;
    pat.color = palette_color(p.patterns.len() + 9).into();
    pat.length = spec.pattern_length(p.time_sig);
    pat.notes.insert(p.channels[at].id.clone(), spec.notes());
    p.patterns.push(pat);
    (at, p.patterns.len() - 1)
}

/// Cut a slicer channel again: its slices and the notes of its pattern follow `spec`.
pub fn reslice(p: &mut Project, channel: usize, spec: &SliceSpec) {
    let Some(c) = p.channels.get(channel) else {
        return;
    };
    let id = c.id.clone();
    let pattern_id = c
        .state
        .get("slicePattern")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut state = c.state.clone();
    let fresh = slicer_state(spec, &pattern_id);
    if let (Some(o), Some(n)) = (state.as_object_mut(), fresh.as_object()) {
        for k in ["slices", "sliceBpm", "slicePattern"] {
            o.insert(k.into(), n[k].clone());
        }
    } else {
        state = fresh;
    }
    p.channels[channel].state = state;
    let sig = p.time_sig;
    if let Some(pat) = p.patterns.iter_mut().find(|x| x.id == pattern_id) {
        pat.notes.insert(id, spec.notes());
        pat.length = spec.pattern_length(sig);
    }
}

/// The slice starts (frames, after the first) of a slicer channel.
pub fn slice_starts(c: &Channel, frames: usize) -> Vec<usize> {
    c.state
        .get("slices")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_f64)
                .map(|f| (f * frames as f64).round() as usize)
                .filter(|&s| s > 0)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SliceSpec {
        SliceSpec {
            starts: vec![22_050, 44_100, 66_150],
            frames: 88_200,
            sample_rate: 44_100.0,
            bpm: 120.0,
        }
    }

    #[test]
    fn slices_become_keys_from_c5_placed_at_the_loop_tempo() {
        let mut p = crate::create::empty_project("x");
        let (ch, pat) = add_slicer(&mut p, "smp_a", "Loop", &spec());
        let c = &p.channels[ch];
        assert_eq!(c.plugin_id, SLICER_ID);
        assert_eq!(c.sample_id.as_deref(), Some("smp_a"));
        let notes = &p.patterns[pat].notes[&c.id];
        assert_eq!(notes.len(), 4);
        // half a second at 120 BPM is a beat
        for (i, n) in notes.iter().enumerate() {
            assert_eq!(n.key, 60 + i as i32);
            assert!((n.pos - i as f64 * 96.0).abs() < 1e-6);
            assert!((n.len - 96.0).abs() < 1e-6);
        }
        assert_eq!(p.patterns[pat].length, 384.0);
        assert_eq!(slice_starts(c, 88_200), vec![22_050, 44_100, 66_150]);
    }

    #[test]
    fn reslicing_rewrites_the_slices_and_the_pattern() {
        let mut p = crate::create::empty_project("x");
        let (ch, pat) = add_slicer(&mut p, "smp_a", "Loop", &spec());
        let mut s = spec();
        s.starts = vec![44_100];
        reslice(&mut p, ch, &s);
        let id = p.channels[ch].id.clone();
        assert_eq!(p.patterns[pat].notes[&id].len(), 2);
        assert_eq!(p.channels[ch].state["slices"], json!([0.0, 0.5]));
    }
}
