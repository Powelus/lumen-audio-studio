// SPDX-License-Identifier: GPL-3.0-or-later
//! A Weave channel takes its patch's values live while it plays, and any voice count
//! at any time is safe.

use las_dsp::modular::{factory, Patch};
use las_engine::{PlayMode, Player};
use las_project::create::{add_channel, default_project};
use std::sync::Arc;

const SR: f64 = 48_000.0;

fn player(patch: &Patch) -> (Player, usize) {
    let mut p = default_project();
    let ch = add_channel(&mut p, "weave");
    p.channels[ch].state = serde_json::json!({ "patch": patch.to_json() });
    let pl = Player::new(Arc::new(p), &Default::default(), SR, PlayMode::Song).unwrap();
    (pl, ch)
}

fn rms(pl: &mut Player, n: usize) -> f32 {
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    pl.process(&mut l, &mut r);
    (l.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt()
}

#[test]
fn a_moved_parameter_is_heard_at_once_and_the_note_carries_on() {
    let patch = Patch::default();
    let (mut pl, ch) = player(&patch);
    pl.note_on(ch, 60.0, 1.0);
    rms(&mut pl, 4_096);
    let loud = rms(&mut pl, 4_096);
    assert!(loud > 1e-3, "{loud}");
    let mut quiet = patch.clone();
    quiet.volume = 0.1;
    let back = pl.weave_patch(ch, Box::new(quiet));
    assert_eq!(*back, patch, "the old patch comes back to be freed");
    let after = rms(&mut pl, 4_096);
    assert!(after < loud * 0.3, "{after} vs {loud}");
    assert!(after > 0.0, "the note still sounds");
    // another shape is not taken: it comes back as it was
    let mut other = patch.clone();
    other.cables.clear();
    let back = pl.weave_patch(ch, Box::new(other.clone()));
    assert_eq!(*back, other);
    // a channel that is not there hands it back too
    let back = pl.weave_patch(99, Box::new(other.clone()));
    assert_eq!(*back, other);
}

#[test]
fn any_voice_count_at_any_time_while_notes_play_is_safe() {
    for (_, base) in factory() {
        let (mut pl, ch) = player(&base);
        pl.play(0.0);
        for step in 0..24usize {
            let mut p = base.clone();
            p.voices = [0, 1, 32, 33, 4, 2][step % 6];
            p.mono = step % 5 == 2;
            drop(pl.weave_patch(ch, Box::new(p)));
            pl.note_on(ch, 40.0 + (step % 30) as f64, 0.9);
            if step % 3 == 0 {
                pl.note_off(ch, 40.0 + ((step + 29) % 30) as f64);
            }
            for n in [0usize, 1, 63, 256] {
                let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
                pl.process(&mut l, &mut r);
                assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            }
        }
    }
}
