// SPDX-License-Identifier: GPL-3.0-or-later
//! The live player plays what the offline render renders.

use las_engine::{render_mix_with, Mode, PlayMode, Player, RenderOptions};
use las_project::decode;
use std::sync::Arc;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Play `name` from the start in callbacks of an awkward size and compare it with the
/// offline render.
fn compare(name: &str) -> (f64, f64) {
    let bundle = decode(&fixture(&format!("{name}.las"))).expect("a project");
    let opts = RenderOptions {
        sample_rate: 24000.0,
        tail: 0.5,
        mode: Mode::Song,
    };
    let want = render_mix_with(&bundle.project, &bundle.samples, &opts).unwrap();
    let mut player = Player::new(
        Arc::new(bundle.project.clone()),
        &bundle.samples,
        24000.0,
        PlayMode::Song,
    )
    .unwrap();
    assert!(player.play(0.0));
    let total = want.left.len();
    // the player's output is late by the plugin delay; the render trims it
    let skip = (player.output_latency() * 24000.0).round() as usize;
    let (mut gl, mut gr) = (Vec::new(), Vec::new());
    let (mut bl, mut br) = (vec![0f32; 441], vec![0f32; 441]);
    while gl.len() < total + skip {
        player.process(&mut bl, &mut br);
        gl.extend_from_slice(&bl);
        gr.extend_from_slice(&br);
    }
    let (mut e2, mut s2, mut worst) = (0f64, 0f64, 0f64);
    for i in 0..total {
        for (g, w) in [(gl[i + skip], want.left[i]), (gr[i + skip], want.right[i])] {
            let (g, w) = (f64::from(g), f64::from(w));
            e2 += (g - w) * (g - w);
            s2 += w * w;
            worst = worst.max((g - w).abs());
        }
    }
    if std::env::var("PLAYER_DEBUG").is_ok() {
        for i in 0..total {
            if (gl[i + skip] - want.left[i]).abs() > 1e-5 {
                eprintln!(
                    "{name}: first difference at frame {i}: {} vs {}",
                    gl[i + skip],
                    want.left[i]
                );
                for k in i.saturating_sub(3)..i + 12 {
                    eprintln!("  {k}: {} vs {}", gl[k + skip], want.left[k]);
                }
                let bad = (i..total)
                    .filter(|&k| (gl[k + skip] - want.left[k]).abs() > 1e-3)
                    .count();
                eprintln!("  {bad} frames of {} differ by more than 1e-3", total - i);
                let last = (i..total)
                    .rev()
                    .find(|&k| (gl[k + skip] - want.left[k]).abs() > 1e-3);
                eprintln!("  last such frame {last:?}");
                break;
            }
        }
    }
    (10.0 * (e2.max(1e-30) / s2.max(1e-30)).log10(), worst)
}

#[test]
fn the_player_matches_the_offline_render() {
    // (A choke in the browser's offline render also silences the voices of later notes
    // that are already scheduled, when the notes are not in time order; the player has
    // not scheduled them yet. So the pad case is left out.)
    for (name, limit) in [
        ("single", -100.0),
        ("automation", -100.0),
        ("tempo", -100.0),
        ("engines", -100.0),
        ("nebula", -100.0),
        ("fxa_ramped", -100.0),
        ("sampler", -100.0),
        ("clips", -100.0),
        // state updates on a 30 ms grid also fall at the points where the song is
        // scheduled in pieces, so the player makes a few more of them
        ("ipa_rack", -35.0),
        ("fxa_timed", -35.0),
    ] {
        let (db, worst) = compare(name);
        assert!(
            db < limit,
            "{name}: the player is only {:.1} dB below the render (worst {worst})",
            -db
        );
    }
}

#[test]
#[ignore]
fn which_channel() {
    let name = std::env::var("CASE").unwrap_or("sampler".into());
    let bundle = decode(&fixture(&format!("{name}.las"))).expect("a project");
    for i in 0..bundle.project.channels.len() {
        let mut p = bundle.project.clone();
        let ch = p.channels[i].clone();
        p.channels = vec![ch];
        let opts = RenderOptions {
            sample_rate: 24000.0,
            tail: 0.5,
            mode: Mode::Song,
        };
        let want = render_mix_with(&p, &bundle.samples, &opts).unwrap();
        let mut player = Player::new(
            Arc::new(p.clone()),
            &bundle.samples,
            24000.0,
            PlayMode::Song,
        )
        .unwrap();
        player.play(0.0);
        let skip = (player.output_latency() * 24000.0).round() as usize;
        let total = want.left.len();
        let mut gl = Vec::new();
        let (mut bl, mut br) = (vec![0f32; 441], vec![0f32; 441]);
        while gl.len() < total + skip {
            player.process(&mut bl, &mut br);
            gl.extend_from_slice(&bl);
        }
        let first = (0..total).find(|&k| (gl[k + skip] - want.left[k]).abs() > 1e-5);
        println!(
            "channel {i} {}: first difference {:?}",
            p.channels[0].name, first
        );
    }
}

#[test]
fn the_default_project_renders_a_beat() {
    let p = las_project::create::default_project();
    let r = las_engine::render_mix_with(
        &p,
        &Default::default(),
        &las_engine::RenderOptions {
            sample_rate: 24000.0,
            tail: 0.2,
            ..Default::default()
        },
    )
    .unwrap();
    let peak = r.left.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "the starter beat is audible, peak {peak}");
    assert!(r.seconds() > 1.5, "a bar at 128 bpm is about 1.9 s");
}

#[test]
fn the_metronome_clicks_on_the_beats_while_playing() {
    use las_engine::{PlayMode, Player};
    use las_project::create::{empty_project, pattern};
    use las_project::{Clip, ClipKind};
    use std::sync::Arc;
    let mut p = empty_project("click");
    p.bpm = 120.0;
    let pat = pattern(0, None);
    p.playlist.clips.push(Clip {
        id: "c".into(),
        kind: ClipKind::Pattern,
        ref_id: pat.id.clone(),
        length: 384.0,
        ..Default::default()
    });
    p.patterns = vec![pat];
    let p = Arc::new(p);
    let run = |on: bool| {
        let mut pl = Player::new(p.clone(), &Default::default(), 24000.0, PlayMode::Song).unwrap();
        pl.set_metronome(on);
        assert!(pl.play(0.0));
        let mut l = vec![0f32; 24000];
        let mut r = vec![0f32; 24000];
        pl.process(&mut l, &mut r);
        // the loudest stretch near each half second (a beat at 120 bpm), and the gaps
        let peak = |a: usize, b: usize| l[a..b].iter().fold(0f32, |m, s| m.max(s.abs()));
        (peak(0, 600), peak(12000, 12600), peak(3000, 9000))
    };
    let (first, second, between) = run(true);
    assert!(
        first > 0.1 && second > 0.1,
        "clicks on beats: {first} {second}"
    );
    assert!(between < 1e-4, "silence between the beats: {between}");
    let (a, b, c) = run(false);
    assert_eq!((a, b, c), (0.0, 0.0, 0.0));
}

/// A channel added from the browser has no patch yet: every instrument plays its
/// default sound instead of the project failing to load.
#[test]
fn a_new_channel_of_every_instrument_plays_its_default_sound() {
    use las_project::create::{add_channel, empty_project, step_note};
    for id in [
        "polaris", "vortex", "monolith", "zenith", "triosc", "facet", "mist", "nebula", "lumapad",
    ] {
        let mut p = empty_project("x");
        let c = add_channel(&mut p, id);
        let ch = p.channels[c].id.clone();
        // LumaPad's first pad sits on C2
        let key = if id == "lumapad" { 36 } else { 60 };
        let mut n = step_note(0, key, 0.9);
        n.len = 96.0;
        p.patterns[0].notes.insert(ch, vec![n]);
        let opts = RenderOptions {
            sample_rate: 24000.0,
            tail: 0.3,
            mode: Mode::Pattern { id: None, loops: 1 },
        };
        let out =
            render_mix_with(&p, &Default::default(), &opts).unwrap_or_else(|e| panic!("{id}: {e}"));
        let peak = out.left.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "{id} is silent ({peak})");
        let player = Player::new(Arc::new(p), &Default::default(), 24000.0, PlayMode::Song);
        assert!(player.is_ok(), "{id}: the live player builds");
    }
}
