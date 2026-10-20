// SPDX-License-Identifier: GPL-3.0-or-later
//! A channel's own swing moves only that channel's steps.

use las_engine::timeline::{channel_swing, pattern_notes_in_range, STEP_F};
use las_project::create::default_project;
use las_project::Note;

#[test]
fn a_channel_swing_moves_only_its_own_steps() {
    let mut p = default_project();
    p.swing = 0.0;
    assert!(p.channels.len() >= 2);
    let (a, b) = (p.channels[0].id.clone(), p.channels[1].id.clone());
    p.channels[0].swing = Some(0.5);
    assert_eq!(channel_swing(&p, &a), 0.5);
    assert_eq!(channel_swing(&p, &b), 0.0);
    // the second step of each channel, entered with the step sequencer
    let step = |id: &str| Note {
        id: id.into(),
        key: 60,
        pos: STEP_F,
        len: STEP_F,
        vel: 0.8,
        step: Some(true),
        ..Default::default()
    };
    let mut pat = p.patterns[0].clone();
    pat.notes.clear();
    pat.notes.insert(a.clone(), vec![step("x")]);
    pat.notes.insert(b.clone(), vec![step("y")]);
    let mut out = Vec::new();
    pattern_notes_in_range(&pat, 0.0, 0.0, f64::INFINITY, 0.0, 1e9, &p, &mut out);
    let at = |ch: &str| out.iter().find(|s| s.channel == ch).unwrap().tick;
    assert!((at(&a) - STEP_F * 1.25).abs() < 1e-9, "{}", at(&a));
    assert!((at(&b) - STEP_F).abs() < 1e-9, "{}", at(&b));
    // NaN or out of range swing is held to 0..1
    p.channels[0].swing = Some(f64::NAN);
    assert_eq!(channel_swing(&p, &a), 0.0);
    p.channels[0].swing = Some(3.0);
    assert_eq!(channel_swing(&p, &a), 1.0);
}
