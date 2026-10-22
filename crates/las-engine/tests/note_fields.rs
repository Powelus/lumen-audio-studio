// SPDX-License-Identifier: GPL-3.0-or-later
//! The pan and fine pitch a note carries (set in the piano roll's event lanes) change
//! what is heard, and a soloed playlist track silences the others.

use las_engine::{render_mix_with, Mode, RenderOptions};
use las_project::create::{add_channel, empty_project, playlist_track, step_note};
use las_project::{Clip, ClipKind, Project};

fn one_note(pan: f64, fine: Option<f64>) -> Project {
    let mut p = empty_project("x");
    let c = add_channel(&mut p, "polaris");
    let ch = p.channels[c].id.clone();
    let mut n = step_note(0, 60, 0.9);
    n.len = 96.0;
    n.step = None;
    n.pan = pan;
    n.fine = fine;
    p.patterns[0].notes.insert(ch, vec![n]);
    p
}

fn render(p: &Project) -> (Vec<f32>, Vec<f32>) {
    let opts = RenderOptions {
        sample_rate: 24000.0,
        tail: 0.2,
        mode: Mode::Pattern { id: None, loops: 1 },
    };
    let r = render_mix_with(p, &Default::default(), &opts).unwrap();
    (r.left, r.right)
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
}

#[test]
fn a_notes_pan_moves_it_between_the_speakers() {
    let (l, r) = render(&one_note(-1.0, None));
    assert!(energy(&l) > energy(&r) * 4.0, "panned left");
    let (l, r) = render(&one_note(1.0, None));
    assert!(energy(&r) > energy(&l) * 4.0, "panned right");
}

#[test]
fn a_notes_fine_pitch_changes_the_sound() {
    let (a, _) = render(&one_note(0.0, None));
    let (b, _) = render(&one_note(0.0, Some(100.0)));
    let diff: f64 = a.iter().zip(&b).map(|(x, y)| f64::from(x - y).abs()).sum();
    assert!(diff > 1.0, "a semitone up sounds different: {diff}");
}

#[test]
fn a_soloed_track_silences_the_others() {
    let mut p = one_note(0.0, None);
    let pat = p.patterns[0].id.clone();
    p.playlist.clips = vec![Clip {
        id: "c".into(),
        kind: ClipKind::Pattern,
        ref_id: pat,
        track: 0,
        start: 0.0,
        length: 384.0,
        ..Default::default()
    }];
    p.playlist.tracks = vec![playlist_track(0), playlist_track(1)];
    let song = |p: &Project| {
        let opts = RenderOptions {
            sample_rate: 24000.0,
            tail: 0.2,
            mode: Mode::Song,
        };
        energy(&render_mix_with(p, &Default::default(), &opts).unwrap().left)
    };
    assert!(song(&p) > 1e-3);
    p.playlist.tracks[1].solo = true;
    assert_eq!(p.playlist.silent_tracks(), vec![true, false]);
    assert!(song(&p) < 1e-9, "track 1 is not soloed");
    p.playlist.tracks[0].solo = true;
    assert!(song(&p) > 1e-3);
}
