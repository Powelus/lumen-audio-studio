// SPDX-License-Identifier: GPL-3.0-or-later
//! The factory kit matches the web version's (`tools/project/factory.gen.test.ts`).

use las_engine::factory::{render_factory, FACTORY_SAMPLES};
use serde_json::Value;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn every_factory_sample_matches_the_web_version() {
    let index: Value = serde_json::from_slice(&fixture("factory.json")).unwrap();
    let index = index.as_array().unwrap();
    assert_eq!(index.len(), FACTORY_SAMPLES.len());
    let all: Vec<f32> = fixture("factory.f32")
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let mut at = 0;
    for entry in index {
        let id = entry["id"].as_str().unwrap();
        let channels = entry["channels"].as_u64().unwrap() as usize;
        let frames = entry["frames"].as_u64().unwrap() as usize;
        let ours = render_factory(id, 24000.0).unwrap_or_else(|| panic!("no sample {id}"));
        assert_eq!(ours.channels.len(), channels, "{id}: channels");
        assert_eq!(ours.frames(), frames, "{id}: length");
        let mut worst = 0f64;
        for ch in &ours.channels {
            for (i, v) in ch.iter().enumerate() {
                worst = worst.max(f64::from(*v - all[at + i]).abs());
            }
            at += frames;
        }
        assert!(worst < 2e-6, "{id}: largest difference {worst}");
    }
    assert_eq!(at, all.len());
}
