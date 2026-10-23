// SPDX-License-Identifier: GPL-3.0-or-later
//! The mod wheel, pitch bend and aftertouch drawn in a pattern's event lanes reach the
//! instrument, when it renders and when it plays live, and are chased when playback
//! starts inside a pattern.

use las_dsp::modular::{Cable, Kind, Node, Patch};
use las_engine::timeline::pattern_controllers_in_range;
use las_engine::{render_mix_with, Mode, PlayMode, Player, RenderOptions};
use las_project::create::{add_channel, empty_project, step_note};
use las_project::{Controllers, CtlKind, Project};
use std::sync::Arc;

const SR: f64 = 24_000.0;

/// A Weave channel that puts the mod wheel out on the left and the pressure on the
/// right, with one long note and the wheel going up at beat 2.
fn wheel_song() -> Project {
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
    let mut p = empty_project("x");
    let c = add_channel(&mut p, "weave");
    p.channels[c].state = serde_json::json!({ "patch": patch.to_json() });
    let ch = p.channels[c].id.clone();
    let mut n = step_note(0, 60, 1.0);
    n.len = 384.0;
    n.step = None;
    p.patterns[0].notes.insert(ch.clone(), vec![n]);
    let mut ctl = Controllers::default();
    ctl.set(CtlKind::ModWheel, 0.0, 0.0);
    ctl.set(CtlKind::ModWheel, 192.0, 0.8);
    ctl.set(CtlKind::Aftertouch, 288.0, 0.5);
    p.patterns[0].controllers.insert(ch, ctl);
    p
}

fn at(x: &[f32], secs: f64) -> f32 {
    x[(secs * SR) as usize]
}

#[test]
fn drawn_controllers_reach_a_weave_channel_when_rendered() {
    let p = wheel_song();
    let opts = RenderOptions {
        sample_rate: SR,
        tail: 0.1,
        mode: Mode::Pattern { id: None, loops: 1 },
    };
    let r = render_mix_with(&p, &Default::default(), &opts).unwrap();
    // 120 BPM: beat 2 is at 1 s, beat 3 at 1.5 s
    assert!(at(&r.left, 0.5).abs() < 1e-3, "wheel down at first");
    assert!(
        at(&r.left, 1.2) > 0.1,
        "wheel up after beat 2: {}",
        at(&r.left, 1.2)
    );
    assert!(at(&r.right, 1.2).abs() < 1e-3, "no pressure yet");
    assert!(at(&r.right, 1.7) > 0.05, "pressure after beat 3");
}

#[test]
fn drawn_controllers_play_live_as_they_render() {
    let p = wheel_song();
    let mut pl = Player::new(Arc::new(p), &Default::default(), SR, PlayMode::Pattern(0)).unwrap();
    assert!(pl.play(0.0));
    let skip = (pl.output_latency() * SR).round() as usize;
    let mut got = Vec::new();
    let (mut l, mut r) = (vec![0f32; 300], vec![0f32; 300]);
    while got.len() < (SR * 1.4) as usize + skip {
        pl.process(&mut l, &mut r);
        got.extend_from_slice(&l);
    }
    assert!(got[skip + (SR * 0.5) as usize].abs() < 1e-3);
    assert!(got[skip + (SR * 1.2) as usize] > 0.1);
}

#[test]
fn starting_inside_a_pattern_chases_the_values_in_force() {
    let p = wheel_song();
    let pat = &p.patterns[0];
    let mut out = Vec::new();
    // from beat 2.5 on: the wheel is up (0.8), sent first, then the pressure point
    pattern_controllers_in_range(pat, 0.0, 0.0, f64::INFINITY, 240.0, 400.0, true, &mut out);
    let wheel: Vec<_> = out
        .iter()
        .filter(|c| c.kind == CtlKind::ModWheel)
        .map(|c| (c.tick, c.value))
        .collect();
    assert_eq!(wheel, vec![(240.0, 0.8)]);
    assert!(out
        .iter()
        .any(|c| c.kind == CtlKind::Aftertouch && c.tick == 288.0 && c.value == 0.5));
    // without chasing, only the points themselves
    out.clear();
    pattern_controllers_in_range(pat, 0.0, 0.0, f64::INFINITY, 240.0, 400.0, false, &mut out);
    assert_eq!(out.len(), 1);
}

#[test]
fn controllers_round_trip_and_old_patterns_have_none() {
    let p = wheel_song();
    let json = serde_json::to_value(&p.patterns[0]).unwrap();
    let ctl = json["controllers"].as_object().unwrap();
    let lane = ctl.values().next().unwrap();
    assert_eq!(lane["modWheel"].as_array().unwrap().len(), 2);
    assert!(lane.get("pitchBend").is_none(), "empty lanes are not saved");
    let back: las_project::Pattern = serde_json::from_value(json).unwrap();
    assert_eq!(back.controllers, p.patterns[0].controllers);
    let old: las_project::Pattern =
        serde_json::from_value(serde_json::json!({"id": "p", "name": "P", "notes": {}})).unwrap();
    assert!(old.controllers.is_empty());
    let mut c = Controllers::default();
    c.set(CtlKind::PitchBend, 10.0, -3.0);
    assert_eq!(c.value_at(CtlKind::PitchBend, 5.0), 0.0);
    assert_eq!(c.value_at(CtlKind::PitchBend, 11.0), -1.0, "clamped");
    c.set(CtlKind::PitchBend, 10.2, 0.5);
    assert_eq!(c.pitch_bend.len(), 1, "replaced, not doubled");
}
