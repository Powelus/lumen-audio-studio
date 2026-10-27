// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! The effects render the same audio as the effect worklets of the web version.
//! The reference files come from `tools/parity/fx.gen.test.ts`.

use las_dsp::fx::delay::Echo;
use las_dsp::fx::dynamics::{Compressor, Crusher, Limiter, NoiseGate, TransientShaper};
use las_dsp::fx::modulation::{FrequencyShifter, PitchShifter, Tape};
use las_dsp::fx::reverb::Hall;
use las_dsp::fx::stepfx::{Position, StepFx, VolumeShaper};
use las_dsp::fx::StereoEffect;
use serde_json::Value;

const SR: f64 = 48000.0;
const BLOCK: usize = 128;
const FRAMES: usize = 188 * BLOCK;

fn read(name: &str) -> Vec<f32> {
    let path = format!("{}/tests/fixtures/{name}.f32", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn max_diff(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .fold(0f64, |m, (x, y)| m.max(f64::from(*x - *y).abs()))
}

fn make(processor: &str) -> Box<dyn StereoEffect> {
    match processor {
        "las-dynamics" => Box::new(Compressor::new(SR)),
        "las-limiter" => Box::new(Limiter::new(SR)),
        "las-crusher" => Box::new(Crusher::new(SR)),
        "las-noisegate" => Box::new(NoiseGate::new(SR)),
        "las-transient" => Box::new(TransientShaper::new(SR)),
        "las-tape" => Box::new(Tape::new(SR)),
        "las-freqshift" => Box::new(FrequencyShifter::new(SR)),
        "las-pitch" => Box::new(PitchShifter::new(SR)),
        "las-hall" => Box::new(Hall::new(SR)),
        "las-echo" => Box::new(Echo::new(SR)),
        other => panic!("no effect {other}"),
    }
}

#[test]
fn the_effects_match_the_typescript_worklets() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../tools/parity/fx.cases.json")).expect("cases");
    assert!(cases.len() >= 19);
    let input = read("fx_input");
    assert_eq!(input.len(), FRAMES * 4);
    let (xl, xr) = (&input[..FRAMES], &input[FRAMES..2 * FRAMES]);
    let (sl, sr_) = (&input[2 * FRAMES..3 * FRAMES], &input[3 * FRAMES..]);
    let mut worst = 0f64;
    for c in &cases {
        let name = c["name"].as_str().expect("name");
        let processor = c["processor"].as_str().expect("processor");
        let use_sc = c["sidechain"].as_bool().unwrap_or(false);
        let bpm = c["transport"]["bpm"].as_f64().unwrap_or(120.0);
        let anchor_beat = c["transport"]["anchorBeat"].as_f64().unwrap_or(0.0);
        // the two effects that follow the song position keep their own typed handle
        let mut stepfx = (processor == "las-stepfx").then(|| StepFx::new(SR));
        let mut shaper = (processor == "las-shaper").then(|| VolumeShaper::new(SR));
        let mut other = (stepfx.is_none() && shaper.is_none()).then(|| make(processor));
        if let Some(s) = &mut stepfx {
            s.set_params(&c["params"]).expect("params");
        }
        if let Some(s) = &mut shaper {
            s.set_params(&c["params"]).expect("params");
        }
        if let Some(o) = &mut other {
            o.set_params(&c["params"]).expect("params");
        }
        let (mut l, mut r) = (vec![0f32; FRAMES], vec![0f32; FRAMES]);
        let mut i = 0;
        while i < FRAMES {
            let end = i + BLOCK;
            let t = i as f64 / SR;
            let beat = anchor_beat + t * (bpm / 60.0);
            let sc = use_sc.then_some((&sl[i..end], &sr_[i..end]));
            let (ol, or) = (&mut l[i..end], &mut r[i..end]);
            if let Some(s) = &mut stepfx {
                s.position = Position { beat, bpm };
                s.process(&xl[i..end], &xr[i..end], ol, or, sc);
            } else if let Some(s) = &mut shaper {
                s.position = Position { beat, bpm };
                s.process(&xl[i..end], &xr[i..end], ol, or, sc);
            } else if let Some(o) = &mut other {
                o.process(&xl[i..end], &xr[i..end], ol, or, sc);
            }
            i = end;
        }
        let all = read(&format!("fx_{name}"));
        let (want_l, want_r) = (&all[..FRAMES], &all[FRAMES..]);
        let (dl, dr) = (max_diff(&l, want_l), max_diff(&r, want_r));
        let peak = want_l.iter().fold(0f32, |m, x| m.max(x.abs()));
        worst = worst.max(dl).max(dr);
        println!("{name}: peak {peak:.4}, max difference left {dl:.2e}, right {dr:.2e}");
        assert!(peak > 0.005, "{name}: the reference is silent");
        assert!(
            dl < 1e-4 && dr < 1e-4,
            "{name}: differs from the TypeScript render (left {dl:e}, right {dr:e})"
        );
    }
    println!("worst difference {worst:.2e}");
}
