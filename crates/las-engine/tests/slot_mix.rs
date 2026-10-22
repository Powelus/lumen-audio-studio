// SPDX-License-Identifier: GPL-3.0-or-later
//! The mix knob of an effect slot (FL's slot volume): 0% plays the dry signal, 100% the
//! effect alone, between is a blend; an automation clip moves it over time, and a live
//! change glides there.

use las_engine::library::encode_wav;
use las_engine::mixer::{slot_key, LiveControl};
use las_engine::render::{render_mix_with, Mode, RenderOptions};
use las_engine::{PlayMode, Player};
use las_project::create::{add_channel, default_project};
use las_project::{AutomationClip, AutomationPoint, Clip, ClipKind, EffectSlot, Project, PPQ};
use std::collections::BTreeMap;
use std::sync::Arc;

fn base() -> (Project, BTreeMap<String, las_project::Sample>) {
    let mut p = default_project();
    let a = las_engine::factory::Audio {
        channels: vec![(0..88_200).map(|i| (i as f32 * 0.03).sin() * 0.1).collect()],
        sample_rate: 44_100.0,
    };
    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_t".to_string(),
        las_project::Sample {
            name: "t".into(),
            mime: "audio/wav".into(),
            data: encode_wav(&a),
        },
    );
    p.samples.push(las_project::SampleRef {
        id: "smp_t".into(),
        name: "t".into(),
        duration: Some(2.0),
        ..Default::default()
    });
    let ch = add_channel(&mut p, "audioclip");
    p.channels[ch].sample_id = Some("smp_t".into());
    p.channels[ch].mixer_track = 1;
    p.mixer[1].volume = 0.25;
    p.playlist.clips.push(Clip {
        id: "clip_t".into(),
        kind: ClipKind::Audio,
        ref_id: p.channels[ch].id.clone(),
        track: 0,
        start: 0.0,
        length: 2.0 * p.bpm / 60.0 * f64::from(PPQ),
        ..Default::default()
    });
    (p, samples)
}

fn with_fx(mix: f64) -> Project {
    let (mut p, _) = base();
    let d = las_dsp::fx::registry::descriptor("las-dist").expect("distortion");
    p.mixer[1].slots.resize(las_project::MIXER_SLOTS, None);
    p.mixer[1].slots[0] = Some(EffectSlot {
        id: "fx_d".into(),
        plugin_id: "las-dist".into(),
        enabled: true,
        mix,
        state: {
            let mut st = d.default_state.clone();
            st["drive"] = serde_json::json!(30);
            st["output"] = serde_json::json!(-12);
            st
        },
        ..Default::default()
    });
    p
}

fn render(p: &Project) -> Vec<f32> {
    let (_, samples) = base();
    let opts = RenderOptions {
        sample_rate: 24_000.0,
        tail: 0.0,
        mode: Mode::Song,
    };
    render_mix_with(p, &samples, &opts).expect("renders").left
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0f32, |m, (x, y)| m.max((x - y).abs()))
}

#[test]
fn mix_0_is_the_dry_signal_and_100_the_effect_alone() {
    let (dry_p, _) = base();
    let dry = render(&dry_p);
    let wet = render(&with_fx(1.0));
    let none = render(&with_fx(0.0));
    let half = render(&with_fx(0.5));
    assert!(dry.iter().any(|s| s.abs() > 0.02), "the clip plays");
    assert!(max_diff(&none, &dry) < 1e-5, "0% is the dry signal");
    assert!(max_diff(&wet, &dry) > 0.05, "the effect changes the sound");
    // 50% is half of each: so 100% holds none of the dry signal
    let blend: Vec<f32> = dry
        .iter()
        .zip(&wet)
        .map(|(d, w)| 0.5 * d + 0.5 * w)
        .collect();
    let e = max_diff(&half, &blend);
    assert!(e < 1e-3, "50% blends the two: {e}");
}

#[test]
fn an_automation_clip_moves_the_mix_over_time() {
    let mut p = with_fx(0.0);
    let bar = 4.0 * f64::from(PPQ);
    p.automation.push(AutomationClip {
        id: "auto_m".into(),
        name: "mix".into(),
        target: serde_json::json!({ "kind": "effect", "track": 1, "slotId": "fx_d", "param": "mix" }),
        points: vec![
            AutomationPoint {
                t: 0.0,
                v: 0.0,
                ..Default::default()
            },
            AutomationPoint {
                t: bar / 2.0,
                v: 0.0,
                ..Default::default()
            },
            AutomationPoint {
                t: bar / 2.0 + 1.0,
                v: 1.0,
                ..Default::default()
            },
        ],
        length: 2.0 * bar,
        ..Default::default()
    });
    p.playlist.clips.push(Clip {
        id: "clip_a".into(),
        kind: ClipKind::Automation,
        ref_id: "auto_m".into(),
        track: 1,
        start: 0.0,
        length: 2.0 * bar,
        ..Default::default()
    });
    let got = render(&p);
    let dry = render(&base().0);
    let wet = render(&with_fx(1.0));
    // the clip jumps from 0% to 100% half a bar in
    let sr = 24_000usize;
    let early = &got[..sr / 4];
    assert!(
        max_diff(early, &dry[..sr / 4]) < 1e-4,
        "dry while the clip says 0%"
    );
    let n = got.len().min(wet.len());
    let switch = (bar / 2.0 / f64::from(PPQ) * 60.0 / p.bpm * sr as f64) as usize;
    if switch + sr / 8 < n {
        let late = switch + sr / 16..n.min(switch + sr / 8);
        assert!(
            max_diff(&got[late.clone()], &wet[late]) < 1e-3,
            "wet once the clip says 100%"
        );
    } else {
        panic!("the render is too short for the test: {n} frames, switch at {switch}");
    }
}

#[test]
fn a_live_change_of_the_mix_glides_without_a_jump() {
    let p = Arc::new(with_fx(0.0));
    let (_, samples) = base();
    let mut player = Player::new(p, &samples, 24_000.0, PlayMode::Song).unwrap();
    let (mut l, mut r) = (vec![0f32; 240], vec![0f32; 240]);
    assert!(player.play(0.0));
    for _ in 0..20 {
        player.process(&mut l, &mut r);
    }
    player.set_live(1, &LiveControl::SlotMix(slot_key("fx_d")), 1.0);
    let mut out = Vec::new();
    for _ in 0..40 {
        player.process(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    // the largest step between frames is no larger than in the dry or the wet
    // signal: the change makes no click
    let wet = render(&with_fx(1.0));
    let dry = render(&with_fx(0.0));
    let step = |b: &[f32]| b.windows(2).fold(0f32, |m, w| m.max((w[1] - w[0]).abs()));
    let most = step(&dry[20 * 240..60 * 240]).max(step(&wet[20 * 240..60 * 240]));
    let got = step(&out);
    assert!(got <= most * 1.02 + 1e-3, "no jump: {got} against {most}");
    // and a while later it is the effect alone
    // (the player may run a little ahead of or behind the offline render's frames)
    let tail = &out[out.len() - 240..];
    let best = |want: &[f32]| {
        (0..want.len() - 240)
            .map(|at| max_diff(tail, &want[at..at + 240]))
            .fold(f32::MAX, f32::min)
    };
    let (to_wet, to_dry) = (best(&wet), best(&dry));
    assert!(
        to_wet < 1e-3 && to_dry > 0.01,
        "the effect alone once it has glided: {to_wet} from the wet, {to_dry} from the dry"
    );
}
