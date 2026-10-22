// SPDX-License-Identifier: GPL-3.0-or-later
//! The slicer: a loop cut at its beats and played back by the pattern the slicer
//! writes sounds like the loop itself.

use las_engine::{render_mix_with, Mode, RenderOptions};
use las_project::slicer::{add_slicer, SliceSpec};
use std::collections::BTreeMap;

const SR: f64 = 44_100.0;

/// Two bars of a drum-like loop at 120 BPM: kicks, hats and a snare.
fn drum_loop() -> Vec<f32> {
    let n = (SR * 4.0) as usize;
    let mut out = vec![0f32; n];
    let mut seed = 1u32;
    for eighth in 0..16 {
        let a = (f64::from(eighth) * 0.25 * SR) as usize;
        let kick = eighth % 4 == 0;
        let snare = eighth % 4 == 2;
        for (k, o) in out[a..n].iter_mut().enumerate() {
            let t = k as f64 / SR;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
            let v = if kick {
                (t * 60.0 * std::f64::consts::TAU).sin() * (-t * 9.0).exp() * 0.8
            } else if snare {
                f64::from(noise) * (-t * 18.0).exp() * 0.5
            } else {
                f64::from(noise) * (-t * 60.0).exp() * 0.25
            };
            *o += v as f32;
        }
    }
    out
}

fn wav(x: &[f32]) -> Vec<u8> {
    las_engine::library::encode_wav(&las_engine::factory::Audio {
        channels: vec![x.to_vec()],
        sample_rate: SR,
    })
}

fn render(
    p: &las_project::Project,
    samples: &BTreeMap<String, las_project::Sample>,
    pat: &str,
) -> Vec<f32> {
    render_mix_with(
        p,
        samples,
        &RenderOptions {
            sample_rate: SR,
            tail: 0.0,
            mode: Mode::Pattern {
                id: Some(pat.to_string()),
                loops: 1,
            },
        },
    )
    .unwrap()
    .left
}

#[test]
fn the_slices_pattern_plays_the_loop_back() {
    let x = drum_loop();
    let onsets = las_dsp::onset::onsets(&x, SR, 0.5);
    assert!(onsets.len() >= 12, "the hits are found: {}", onsets.len());
    let bpm = las_dsp::onset::detect_tempo(&onsets, SR, x.len()).unwrap();
    assert!((bpm - 120.0).abs() < 0.01, "tempo {bpm}");

    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_loop".to_string(),
        las_project::Sample {
            name: "Loop".into(),
            mime: "audio/wav".into(),
            data: wav(&x),
        },
    );
    let mut p = las_project::create::empty_project("slices");
    p.bpm = 120.0;
    p.samples.push(las_project::SampleRef {
        id: "smp_loop".into(),
        name: "Loop".into(),
        duration: Some(4.0),
        ..Default::default()
    });
    p.channels.clear();
    let spec = SliceSpec {
        starts: onsets,
        frames: x.len(),
        sample_rate: SR,
        bpm,
    };
    let (_, pat) = add_slicer(&mut p, "smp_loop", "Loop", &spec);
    let pat_id = p.patterns[pat].id.clone();
    let sliced = render(&p, &samples, &pat_id);

    // the same loop played whole by a sampler, through the same path
    let mut q = p.clone();
    q.channels[0].plugin_id = "sampler".into();
    q.channels[0].state = serde_json::json!({ "velToVol": 0.0, "attack": 0.001 });
    let id = q.channels[0].id.clone();
    let pq = q.patterns.iter_mut().find(|x| x.id == pat_id).unwrap();
    let mut first = pq.notes[&id][0].clone();
    first.len = 384.0 * 2.0;
    pq.notes.insert(id, vec![first]);
    let whole = render(&q, &samples, &pat_id);

    let n = (SR * 3.9) as usize;
    assert!(sliced.len() >= n && whole.len() >= n);
    let (a, b) = (&sliced[..n], &whole[..n]);
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    let ea: f64 = a.iter().map(|x| f64::from(*x).powi(2)).sum();
    let eb: f64 = b.iter().map(|x| f64::from(*x).powi(2)).sum();
    let corr = dot / (ea * eb).sqrt();
    assert!(eb > 1.0, "the loop sounds");
    assert!(
        corr > 0.98,
        "the slices sound like the loop: correlation {corr}"
    );
    assert!((ea / eb - 1.0).abs() < 0.05, "and as loud: {}", ea / eb);
}

#[test]
fn a_key_past_the_last_slice_is_silent() {
    let x = drum_loop();
    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_loop".to_string(),
        las_project::Sample {
            name: "Loop".into(),
            mime: "audio/wav".into(),
            data: wav(&x),
        },
    );
    let mut p = las_project::create::empty_project("slices");
    p.channels.clear();
    let spec = SliceSpec {
        starts: vec![88_200],
        frames: x.len(),
        sample_rate: SR,
        bpm: 120.0,
    };
    let (ch, pat) = add_slicer(&mut p, "smp_loop", "Loop", &spec);
    let id = p.channels[ch].id.clone();
    for n in p.patterns[pat].notes.get_mut(&id).unwrap() {
        n.key += 2;
    }
    let pat_id = p.patterns[pat].id.clone();
    let out = render(&p, &samples, &pat_id);
    assert!(out.iter().all(|v| v.abs() < 1e-6));
}
