// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation curves (FL's point modes) and tempo automation, played live and rendered.

use las_engine::timeline::{automation_curve, automation_value_at, seconds_between, tempo_at};
use las_engine::{render_mix_with, Mode, PlayMode, Player, RenderOptions};
use las_project::create::{default_project, step_note};
use las_project::{decode, AutomationClip, AutomationPoint, Clip, ClipKind, CurveKind, Project};
use serde_json::json;
use std::sync::Arc;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn pt(t: f64, v: f64, curve: CurveKind, tension: f64) -> AutomationPoint {
    AutomationPoint {
        t,
        v,
        curve: Some(curve),
        tension: Some(tension),
        ..Default::default()
    }
}

fn clip(points: Vec<AutomationPoint>) -> AutomationClip {
    AutomationClip {
        id: "a".into(),
        length: points.last().map_or(1.0, |p| p.t),
        points,
        ..Default::default()
    }
}

#[test]
fn every_curve_runs_from_one_point_to_the_next() {
    for kind in CurveKind::ALL {
        for tension in [-1.0, -0.4, 0.0, 0.5, 1.0] {
            assert!(kind.shape(tension, 0.0).abs() < 1e-9, "{kind:?} {tension}");
            assert!((kind.shape(tension, 1.0) - 1.0).abs() < 1e-9, "{kind:?}");
            // never goes back
            let mut last = 0.0;
            for i in 0..=100 {
                let v = kind.shape(tension, f64::from(i) / 100.0);
                assert!(v + 1e-9 >= last && (0.0..=1.0).contains(&v), "{kind:?}");
                last = v;
            }
        }
    }
    // tension bends a single curve: up starts slowly, down quickly
    assert!(CurveKind::Single.shape(0.8, 0.5) < 0.3);
    assert!(CurveKind::Single.shape(-0.8, 0.5) > 0.7);
    assert_eq!(CurveKind::Single.shape(0.0, 0.25), 0.25);
    assert_eq!(CurveKind::Hold.shape(0.7, 0.99), 0.0);
    assert!((CurveKind::Smooth.shape(0.0, 0.5) - 0.5).abs() < 1e-9);
    assert_eq!(CurveKind::stairs(-1.0), 2);
    assert_eq!(CurveKind::stairs(1.0), 16);
}

#[test]
fn the_lines_played_follow_the_curve() {
    for kind in CurveKind::ALL {
        let c = clip(vec![pt(0.0, 0.1, kind, 0.6), pt(384.0, 0.9, kind, 0.0)]);
        let line = automation_curve(&c);
        assert!(
            line.windows(2).all(|w| w[0].0 <= w[1].0),
            "{kind:?} in order"
        );
        // between the line's points, ramping linearly stays on the curve
        for i in 0..384 {
            let t = f64::from(i) + 0.5;
            let j = line.partition_point(|p| p.0 <= t);
            let (a, b) = (line[j - 1], line[j]);
            let lin = a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0);
            let want = automation_value_at(&c, t);
            assert!(
                (lin - want).abs() < 0.01,
                "{kind:?} at {t}: {lin} vs {want}"
            );
        }
    }
    // a straight line needs no extra points
    let c = clip(vec![
        pt(0.0, 0.0, CurveKind::Single, 0.0),
        pt(96.0, 1.0, CurveKind::Single, 0.0),
    ]);
    assert_eq!(automation_curve(&c).len(), 2);
}

/// A song of a steady kick with an automation clip on the master volume.
fn kick_song(points: Vec<AutomationPoint>) -> Project {
    let mut p = default_project();
    let len = 384.0 * 4.0;
    p.patterns[0].length = len;
    let kick = p.channels[0].id.clone();
    p.patterns[0].notes.clear();
    p.patterns[0]
        .notes
        .insert(kick, (0..64).map(|s| step_note(s, 60, 0.9)).collect());
    p.playlist.clips.retain(|c| c.kind == ClipKind::Pattern);
    p.playlist.clips.truncate(1);
    p.playlist.clips[0].start = 0.0;
    p.playlist.clips[0].length = len;
    p.automation = vec![AutomationClip {
        id: "vol".into(),
        name: "Master volume".into(),
        target: json!({ "kind": "master", "param": "volume" }),
        points,
        length: len,
        ..Default::default()
    }];
    p.playlist.clips.push(Clip {
        id: "cv".into(),
        kind: ClipKind::Automation,
        ref_id: "vol".into(),
        track: 1,
        start: 0.0,
        length: len,
        ..Default::default()
    });
    p
}

fn render(p: &Project) -> las_engine::Rendered {
    render_mix_with(
        p,
        &Default::default(),
        &RenderOptions {
            sample_rate: 24000.0,
            tail: 0.0,
            ..Default::default()
        },
    )
    .unwrap()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0f32, |m, s| m.max(s.abs()))
}

#[test]
fn a_hold_point_keeps_its_value_until_the_next_point() {
    let len = 384.0 * 4.0;
    let points = |kind| vec![pt(0.0, 0.0, kind, 0.0), pt(len, 0.8, kind, 0.0)];
    let held = render(&kick_song(points(CurveKind::Hold)));
    let ramp = render(&kick_song(points(CurveKind::Single)));
    let n = held.left.len();
    // in the third quarter the straight ramp is loud, the hold still silent
    let q = |r: &las_engine::Rendered| peak(&r.left[n / 2..n * 3 / 4]);
    assert!(q(&ramp) > 0.1, "{}", q(&ramp));
    assert!(q(&held) < 1e-4, "held at zero: {}", q(&held));
}

#[test]
fn a_bent_curve_is_quieter_early_than_a_straight_one() {
    let len = 384.0 * 4.0;
    let up = |t| {
        vec![
            pt(0.0, 0.0, CurveKind::Single, t),
            pt(len, 0.8, CurveKind::Single, 0.0),
        ]
    };
    let bent = render(&kick_song(up(1.0)));
    let straight = render(&kick_song(up(0.0)));
    let n = bent.left.len();
    let (b, s) = (
        peak(&bent.left[n / 4..n / 2]),
        peak(&straight.left[n / 4..n / 2]),
    );
    assert!(b < s * 0.5, "bent {b}, straight {s}");
}

/// Play a project live in awkward callbacks and compare with the offline render.
fn live_vs_render(
    p: &Project,
    samples: &std::collections::BTreeMap<String, las_project::Sample>,
) -> f64 {
    let want = render_mix_with(
        p,
        samples,
        &RenderOptions {
            sample_rate: 24000.0,
            tail: 0.3,
            mode: Mode::Song,
        },
    )
    .unwrap();
    let mut player = Player::new(Arc::new(p.clone()), samples, 24000.0, PlayMode::Song).unwrap();
    assert!(player.play(0.0));
    let skip = (player.output_latency() * 24000.0).round() as usize;
    let total = want.left.len();
    let (mut got, mut bl, mut br) = (Vec::new(), vec![0f32; 441], vec![0f32; 441]);
    while got.len() < total + skip {
        player.process(&mut bl, &mut br);
        got.extend_from_slice(&bl);
    }
    let (mut e2, mut s2) = (0f64, 0f64);
    for i in 0..total {
        let (g, w) = (f64::from(got[i + skip]), f64::from(want.left[i]));
        e2 += (g - w) * (g - w);
        s2 += w * w;
    }
    10.0 * (e2.max(1e-30) / s2.max(1e-30)).log10()
}

#[test]
fn curves_play_live_as_they_render() {
    for kind in CurveKind::ALL {
        let mut b = decode(&fixture("automation.las")).unwrap();
        for a in &mut b.project.automation {
            for q in &mut a.points {
                q.curve = Some(kind);
                q.tension = Some(0.5);
            }
        }
        let db = live_vs_render(&b.project, &b.samples);
        assert!(
            db < -80.0,
            "{kind:?}: live is only {:.1} dB from the render",
            -db
        );
    }
}

/// The kick song with a tempo clip from `a` to `b` (0 to 1 of 60 to 200 BPM).
fn tempo_song(a: f64, b: f64, kind: CurveKind) -> Project {
    let mut p = kick_song(Vec::new());
    let len = 384.0 * 4.0;
    p.automation[0].target = json!({ "kind": "master", "param": "tempo" });
    p.automation[0].points = vec![pt(0.0, a, kind, 0.5), pt(len, b, kind, 0.0)];
    p
}

#[test]
fn tempo_automation_changes_the_length_of_the_render() {
    let len = 384.0 * 4.0;
    let slow = tempo_song(0.0, 0.0, CurveKind::Single);
    let faster = tempo_song(0.0, 1.0, CurveKind::Single);
    assert_eq!(tempo_at(&slow, 100.0, true), 60.0);
    assert!(tempo_at(&faster, len - 1.0, true) > 190.0);
    let (rs, rf) = (render(&slow), render(&faster));
    let secs = |r: &las_engine::Rendered| r.left.len() as f64 / 24000.0;
    // four bars at 60 BPM are 16 s; speeding up to 200 makes them much shorter
    assert!((secs(&rs) - 16.0).abs() < 0.1, "{}", secs(&rs));
    assert!(secs(&rf) < 11.0, "{}", secs(&rf));
    assert!((secs(&rf) - seconds_between(&faster, 0.0, len, true)).abs() < 0.05);
    // a curve that stays slow longer takes longer than the straight ramp
    let bent = tempo_song(0.0, 1.0, CurveKind::Hold);
    assert!(seconds_between(&bent, 0.0, len, true) > secs(&rf) + 1.0);
    // and the song plays live as it renders
    for p in [&faster, &bent] {
        let db = live_vs_render(p, &Default::default());
        assert!(
            db < -80.0,
            "tempo: live is only {:.1} dB from the render",
            -db
        );
    }
}
