// SPDX-License-Identifier: GPL-3.0-or-later
//! The Mist engine renders the same audio as the TypeScript engine of the web
//! version. The reference files come from `tools/parity/mist.gen.test.ts`.

use las_dsp::engine::{Cc, Event};
use las_dsp::mist::{default_texture, mist_engine};
use serde_json::Value;

const SR: f64 = 48000.0;
const BLOCK: usize = 128;
const FRAMES: usize = 24000;

fn num(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn event(v: &Value) -> Event {
    let time = num(v, "time", 0.0);
    match v["t"].as_str().expect("event type") {
        "on" => Event::On {
            time,
            key: num(v, "key", 60.0),
            vel: num(v, "vel", 1.0),
            fine: num(v, "fine", 0.0),
            pan: num(v, "pan", 0.0),
        },
        "off" => Event::Off {
            time,
            key: num(v, "key", 60.0),
        },
        "allOff" => Event::AllOff { time },
        "cc" => Event::Cc {
            time,
            cc: match v["cc"].as_str().expect("cc name") {
                "mw" => Cc::Mw,
                "at" => Cc::At,
                _ => Cc::Pb,
            },
            value: num(v, "value", 0.0),
        },
        other => panic!("unknown event {other}"),
    }
}

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

#[test]
fn the_built_in_texture_matches_the_typescript_one() {
    let (l, r) = default_texture(SR, 4.0);
    let want = read("mist_texture");
    // first 4800 and last 4800 samples of the left channel, then of the right
    let n = l.len();
    let got: Vec<f32> = [&l[..4800], &l[n - 4800..], &r[..4800], &r[n - 4800..]].concat();
    let d = max_diff(&got, &want);
    println!("texture: max difference {d:.2e}");
    assert!(d < 1e-6, "the texture differs by {d:e}");
}

#[test]
fn mist_matches_the_typescript_engine() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../tools/parity/mist.cases.json")).expect("cases");
    assert!(cases.len() >= 5);
    for c in &cases {
        let name = c["name"].as_str().expect("name");
        let mut e = mist_engine(SR, c["seed"].as_u64().expect("seed") as u32);
        e.set_patch(&c["patch"]).expect("a valid patch");
        let events: Vec<Event> = c["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(event)
            .collect();
        e.queue(&events);
        let mut l = vec![0f32; FRAMES];
        let mut r = vec![0f32; FRAMES];
        let mut i = 0;
        while i < FRAMES {
            let end = (i + BLOCK).min(FRAMES);
            e.process(&mut l[i..end], &mut r[i..end], i as f64 / SR);
            i = end;
        }
        let all = read(&format!("mist_{name}"));
        assert_eq!(all.len(), FRAMES * 2);
        let (want_l, want_r) = (&all[..FRAMES], &all[FRAMES..]);
        let (dl, dr) = (max_diff(&l, want_l), max_diff(&r, want_r));
        let peak = want_l.iter().fold(0f32, |m, x| m.max(x.abs()));
        println!(
            "{name}: peak {peak:.4}, grains {}, max difference left {dl:.2e}, right {dr:.2e}",
            e.sound.grains_started
        );
        assert!(peak > 0.01, "{name}: the reference is silent");
        assert!(
            dl < 1e-5 && dr < 1e-5,
            "{name}: differs from the TypeScript render (left {dl:e}, right {dr:e})"
        );
    }
}
