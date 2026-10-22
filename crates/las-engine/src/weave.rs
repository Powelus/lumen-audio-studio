// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave, the modular synth, as a channel instrument. It takes its notes like a hosted
//! plugin does (see [`crate::external::Hosted`]), so a block is played in pieces that
//! end at each note. Every parameter of every module can be automated: its id is
//! [`las_dsp::modular::param_id`].

use crate::external::{Instrument, Note, ParamLines};
use las_dsp::engine::Cc;
use las_dsp::modular::{split_param_id, Controls, Modular, Patch};
use serde_json::Value;

/// The `pluginId` of a Weave channel.
pub const WEAVE_ID: &str = "weave";

/// The patch a channel's state holds (the default patch when it holds none, or one
/// that cannot be read).
pub fn patch_of(state: &Value) -> Patch {
    state
        .get("patch")
        .and_then(|p| Patch::from_json(p).ok())
        .unwrap_or_default()
}

pub(crate) struct Weave {
    synth: Modular,
    lines: ParamLines,
    controls: Controls,
}

impl Weave {
    pub fn new(state: &Value, sr: f64) -> Self {
        Self {
            synth: Modular::new(&patch_of(state), sr),
            lines: ParamLines::default(),
            controls: Controls::default(),
        }
    }

    /// Move the automated parameters to where their lines are at `t` seconds.
    fn follow(&mut self, t: f64) {
        for (id, v) in self.lines.due(t) {
            let (node, index) = split_param_id(id);
            let Some(spec) = self
                .synth
                .patch()
                .node(node)
                .and_then(|n| n.kind.spec().params.get(index))
            else {
                continue;
            };
            self.synth.set_param(node, spec.name, spec.denormalize(v));
        }
    }
}

impl Instrument for Weave {
    fn process(&mut self, t0: f64, notes: &[Note], l: &mut [f32], r: &mut [f32]) {
        self.follow(t0);
        let n = l.len().min(r.len());
        let mut at = 0;
        for note in notes {
            let off = (note.offset() as usize).min(n);
            if off > at {
                self.synth.render(&mut l[at..off], &mut r[at..off]);
                at = off;
            }
            match *note {
                Note::On { key, vel, .. } => self.synth.note_on(key, vel),
                Note::Off { key, .. } => self.synth.note_off(key),
            }
        }
        if n > at {
            self.synth.render(&mut l[at..n], &mut r[at..n]);
        }
    }

    fn automate(&mut self, id: u32, points: &[(f64, f64)], first: bool) -> bool {
        let (node, index) = split_param_id(id);
        let known = self
            .synth
            .patch()
            .node(node)
            .is_some_and(|n| index < n.kind.spec().params.len());
        if known {
            self.lines.automate(id, points, first);
        }
        known
    }

    fn controller(&mut self, cc: Cc, value: f64) {
        let v = value as f32;
        match cc {
            Cc::Mw => self.controls.wheel = v.clamp(0.0, 1.0),
            Cc::Pb => self.controls.bend = v.clamp(-1.0, 1.0),
            Cc::At => self.controls.pressure = v.clamp(0.0, 1.0),
        }
        self.synth.set_controls(self.controls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use las_dsp::modular::{param_id, Kind};

    const SR: f64 = 48_000.0;

    #[test]
    fn notes_start_where_their_offset_is() {
        let mut w = Weave::new(&Value::Null, SR);
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        w.process(
            0.0,
            &[Note::On {
                offset: 300,
                key: 60,
                vel: 1.0,
            }],
            &mut l,
            &mut r,
        );
        assert!(l[..300].iter().all(|&v| v == 0.0));
        assert!(l[300..].iter().any(|&v| v != 0.0));
    }

    #[test]
    fn a_parameter_follows_its_automation() {
        let mut w = Weave::new(&Value::Null, SR);
        // the default patch's output level (node 6, its only parameter)
        let out = w.synth.patch().node(6).unwrap();
        assert_eq!(out.kind, Kind::Output);
        let id = param_id(6, 0);
        assert!(w.automate(id, &[(0.0, 0.0), (1.0, 0.0)], true));
        assert!(!w.automate(param_id(99, 0), &[(0.0, 0.0)], true));
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        w.process(
            0.0,
            &[Note::On {
                offset: 0,
                key: 60,
                vel: 1.0,
            }],
            &mut l,
            &mut r,
        );
        // the level is at 0, so it is silent
        assert!(l.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn a_state_without_a_patch_plays_the_default() {
        assert_eq!(patch_of(&Value::Null), Patch::default());
        let state = serde_json::json!({ "patch": { "nodes": "bad" } });
        assert_eq!(patch_of(&state), Patch::default());
        let state = serde_json::json!({ "patch": Patch::default().to_json() });
        assert_eq!(patch_of(&state), Patch::default());
    }

    #[test]
    fn the_mod_wheel_bend_and_pressure_reach_the_keys_module() {
        use las_dsp::modular::{Cable, Node};
        // a patch that puts the mod wheel (and the pressure) straight out
        let patch = Patch::new(
            vec![
                Node::new(1, Kind::Keys, [0.0; 2]),
                Node::new(100, Kind::Output, [0.0; 2]).with("level", 1.0),
            ],
            vec![
                Cable::new(1, "wheel", 100, "left", 1.0),
                Cable::new(1, "pressure", 100, "right", 1.0),
            ],
            4,
        );
        let state = serde_json::json!({ "patch": patch.to_json() });
        let mut tables = las_dsp::wavetable::TableLibrary::new();
        let mut inst =
            crate::instrument::Instrument::new(WEAVE_ID, &state, SR, &mut tables, None, Vec::new())
                .unwrap();
        inst.note_on(60.0, 1.0, 0.0);
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        inst.render(0.0, &mut l, &mut r);
        let still = l[200].abs();
        inst.controller(Cc::Mw, 0.75, 0.0);
        inst.controller(Cc::At, 0.5, 0.0);
        inst.render(256.0 / SR, &mut l, &mut r);
        assert!(still < 1e-6, "{still}");
        assert!(l[200] > 0.1, "{}", l[200]);
        assert!(r[200] > 0.05, "{}", r[200]);
    }

    #[test]
    fn a_weave_channel_plays_and_takes_automation_by_path() {
        let mut tables = las_dsp::wavetable::TableLibrary::new();
        let mut inst = crate::instrument::Instrument::new(
            WEAVE_ID,
            &Value::Null,
            SR,
            &mut tables,
            None,
            Vec::new(),
        )
        .unwrap();
        assert!(!inst.is_silent());
        // the default patch's filter cutoff: node 4, parameter 1
        let path = format!("p.{}", param_id(4, 1));
        assert!(inst.automate(&path, &[(0.0, 0.2)], true, 0));
        assert!(!inst.automate("p.99999", &[(0.0, 0.2)], true, 0));
        inst.note_on(60.0, 1.0, 0.0);
        let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
        inst.render(0.0, &mut l, &mut r);
        assert!(l.iter().any(|&v| v != 0.0));
    }
}
