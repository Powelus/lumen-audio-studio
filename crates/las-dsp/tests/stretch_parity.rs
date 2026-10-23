// SPDX-License-Identifier: GPL-3.0-or-later
//! The time stretch matches the web version's (`tools/project/stretch.gen.test.ts`).

use las_dsp::stretch::time_stretch;
use serde_json::Value;

#[test]
fn stretches_match_the_web_version() {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let index: Value =
        serde_json::from_slice(&std::fs::read(format!("{dir}/stretch.json")).unwrap()).unwrap();
    let all: Vec<f32> = std::fs::read(format!("{dir}/stretch.f32"))
        .unwrap()
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let mut at = 0;
    for case in index.as_array().unwrap() {
        let factor = case["factor"].as_f64().unwrap();
        let channels = case["channels"].as_u64().unwrap() as usize;
        let in_frames = case["inFrames"].as_u64().unwrap() as usize;
        let out_frames = case["outFrames"].as_u64().unwrap() as usize;
        let input: Vec<Vec<f32>> = (0..channels)
            .map(|c| all[at + c * in_frames..at + (c + 1) * in_frames].to_vec())
            .collect();
        at += channels * in_frames;
        let want: Vec<&[f32]> = (0..channels)
            .map(|c| &all[at + c * out_frames..at + (c + 1) * out_frames])
            .collect();
        at += channels * out_frames;
        let got = time_stretch(&input, 24000.0, factor);
        assert_eq!(got[0].len(), out_frames, "{case}: length");
        for (c, w) in want.iter().enumerate() {
            let worst = got[c]
                .iter()
                .zip(*w)
                .fold(0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(
                worst < 1e-4,
                "{case} channel {c}: largest difference {worst}"
            );
        }
    }
    assert_eq!(at, all.len());
}
