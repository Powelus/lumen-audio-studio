// SPDX-License-Identifier: GPL-3.0-or-later
//! The pan and fine pitch a note carries (set in the piano roll's event lanes) change
//! what is heard, and a soloed playlist track silences the others.

use las_engine::{render_mix_with, Mode, RenderOptions};
use las_project::create::{add_channel, empty_project, playlist_track, step_note};
use las_project::{Clip, ClipKind, Project};
use std::collections::BTreeMap;

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

/// Zero crossings per second of `x` between `a` and `b` seconds.
fn crossings(x: &[f32], sr: f64, a: f64, b: f64) -> f64 {
    let s = &x[(a * sr) as usize..(b * sr) as usize];
    let n = s
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    n as f64 / (b - a)
}

/// Hold the pattern's note for 8 beats while the channel pitch holds, then rises an
/// octave (beats 2 to 3); the zero crossings per second before and after.
fn bent(mut p: Project, samples: &BTreeMap<String, las_project::Sample>) -> (f64, f64) {
    use las_project::{AutomationClip, AutomationPoint};
    let ch = p.channels.last().unwrap().id.clone();
    p.patterns[0].notes.get_mut(&ch).unwrap()[0].len = 768.0;
    let pat = p.patterns[0].id.clone();
    let pt = |t: f64, v: f64| AutomationPoint {
        t,
        v,
        ..Default::default()
    };
    p.automation.push(AutomationClip {
        id: "pa".into(),
        target: serde_json::json!({"kind": "channel", "channelId": ch, "param": "pitch"}),
        points: vec![pt(0.0, 0.5), pt(192.0, 0.5), pt(288.0, 1.0), pt(768.0, 1.0)],
        length: 768.0,
        ..Default::default()
    });
    p.playlist.tracks = vec![playlist_track(0), playlist_track(1)];
    let clip = |id: &str, kind, ref_id: String, track| Clip {
        id: id.into(),
        kind,
        ref_id,
        track,
        start: 0.0,
        length: 768.0,
        ..Default::default()
    };
    p.playlist.clips = vec![
        clip("c", ClipKind::Pattern, pat, 0),
        clip("a", ClipKind::Automation, "pa".into(), 1),
    ];
    let sr = 24000.0;
    let opts = RenderOptions {
        sample_rate: sr,
        tail: 0.2,
        mode: Mode::Song,
    };
    let l = render_mix_with(&p, samples, &opts).unwrap().left;
    let spb = 60.0 / p.bpm;
    (
        crossings(&l, sr, 0.2 * spb, 1.8 * spb),
        crossings(&l, sr, 3.3 * spb, 4.8 * spb),
    )
}

#[test]
fn channel_pitch_automation_bends_a_synth_note_that_already_sounds() {
    let (before, after) = bent(one_note(0.0, None), &BTreeMap::new());
    assert!(
        after > before * 1.7,
        "the held note went up an octave: {before} -> {after}"
    );
}

#[test]
fn channel_pitch_automation_bends_a_sampler_note_that_already_sounds() {
    let sr = 24000.0;
    let sine: Vec<f32> = (0..(sr as usize * 6))
        .map(|i| (i as f64 * 220.0 * std::f64::consts::TAU / sr).sin() as f32 * 0.5)
        .collect();
    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_sine".to_string(),
        las_project::Sample {
            name: "Sine".into(),
            mime: "audio/wav".into(),
            data: las_engine::library::encode_wav(&las_engine::factory::Audio {
                channels: vec![sine],
                sample_rate: sr,
            }),
        },
    );
    let mut p = one_note(0.0, None);
    p.samples.push(las_project::SampleRef {
        id: "smp_sine".into(),
        name: "Sine".into(),
        duration: Some(6.0),
        ..Default::default()
    });
    let c = p.channels.last_mut().unwrap();
    c.plugin_id = "sampler".into();
    c.kind = "sampler".into();
    c.sample_id = Some("smp_sine".into());
    c.state = serde_json::json!({ "velToVol": 0.0, "attack": 0.001, "sustain": 1.0 });
    let (before, after) = bent(p, &samples);
    assert!(before > 400.0, "the sample sounds: {before}");
    assert!(
        (after / before - 2.0).abs() < 0.05,
        "the held sample went up an octave: {before} -> {after}"
    );
}
