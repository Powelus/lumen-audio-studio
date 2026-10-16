// SPDX-License-Identifier: GPL-3.0-or-later
//! The Rust mix against the web version's offline render of the same project
//! (files from `tools/project/mix.gen.test.ts` and `render-web.mjs`).
//!
//! The web version builds its mixer from Web Audio nodes, so the two are not
//! bit for bit; the check is that the error is far below the signal.

use las_engine::{render_mix, Mode, RenderOptions};
use las_project::decode;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

struct Diff {
    peak: f64,
    max_err: f64,
    /// Error RMS against signal RMS, dB.
    rel_db: f64,
    frames: (usize, usize),
}

fn compare(name: &str) -> Diff {
    let bundle = decode(&fixture(&format!("{name}.las"))).expect("a project");
    let opts = RenderOptions {
        sample_rate: 24000.0,
        tail: 0.5,
        mode: Mode::Song,
    };
    let got = render_mix(&bundle.project, &opts).expect("renders");
    let want = floats(&fixture(&format!("{name}.f32")));
    let frames = want.len() / 2;
    let (wl, wr) = want.split_at(frames);
    let mut peak = 0f64;
    let mut max_err = 0f64;
    let (mut e2, mut s2) = (0f64, 0f64);
    for i in 0..frames.min(got.left.len()) {
        for (g, w) in [(got.left[i], wl[i]), (got.right[i], wr[i])] {
            let (g, w) = (f64::from(g), f64::from(w));
            peak = peak.max(w.abs());
            max_err = max_err.max((g - w).abs());
            e2 += (g - w) * (g - w);
            s2 += w * w;
        }
    }
    Diff {
        peak,
        max_err,
        rel_db: 10.0 * (e2.max(1e-30) / s2.max(1e-30)).log10(),
        frames: (got.left.len(), frames),
    }
}

/// Projects whose parts the web version runs as its own code (the synths, the
/// effect worklets, the routing): equal but for rounding.
const EXACT: [&str; 6] = [
    "single",
    "mixerdry",
    "sidechain",
    "enginesdry",
    "nebuladry",
    "fx_stereo",
];

/// With effects the web version builds from Web Audio's own nodes (equaliser,
/// overdrive, chorus, phaser, auto filter, maximizer, convolution reverb): the
/// error is more than 85 dB below the signal.
const NATIVE: [&str; 12] = [
    "mixer",
    "engines",
    "nebula",
    "fx_eq",
    "fx_dist",
    "fx_chorus",
    "fx_chorus_fb",
    "fx_phaser",
    "fx_phaser_nofb",
    "fx_filter",
    "fx_maximizer",
    "fx_reverb",
];

#[test]
fn synths_mixer_and_scheduler_match_the_web_version() {
    for name in EXACT {
        let d = compare(name);
        assert_eq!(d.frames.0, d.frames.1, "{name}: length");
        assert!(d.peak > 0.01, "{name}: the reference is silent");
        assert!(
            d.rel_db < -120.0,
            "{name}: error {:.1} dB below the signal",
            -d.rel_db
        );
        assert!(d.max_err < 1e-6, "{name}: largest error {}", d.max_err);
    }
}

#[test]
fn web_audio_effects_match_closely() {
    for name in NATIVE {
        let d = compare(name);
        assert_eq!(d.frames.0, d.frames.1, "{name}: length");
        assert!(d.peak > 0.01, "{name}: the reference is silent");
        assert!(
            d.rel_db < -85.0,
            "{name}: error only {:.1} dB below the signal",
            -d.rel_db
        );
    }
}

/// Where the error is, window by window (`CASE=mixer cargo test --release windows -- --ignored --nocapture`).
#[test]
#[ignore]
fn windows() {
    let name = std::env::var("CASE").unwrap_or("mixer".into());
    let bundle = decode(&fixture(&format!("{name}.las"))).expect("a project");
    let opts = RenderOptions {
        sample_rate: 24000.0,
        tail: 0.5,
        mode: Mode::Song,
    };
    let got = render_mix(&bundle.project, &opts).expect("renders");
    let want = floats(&fixture(&format!("{name}.f32")));
    let frames = want.len() / 2;
    let (wl, wr) = want.split_at(frames);
    let w = 3000;
    for k in 0..frames / w {
        let (mut e, mut s, mut sg) = (0f64, 0f64, 0f64);
        for i in k * w..(k + 1) * w {
            for (g, x) in [(got.left[i], wl[i]), (got.right[i], wr[i])] {
                e += f64::from(g - x).powi(2);
                s += f64::from(x).powi(2);
                sg += f64::from(g).powi(2);
            }
        }
        println!(
            "{:5.2}s web rms {:.5} rust rms {:.5} err rms {:.6}",
            (k * w) as f64 / 24000.0,
            (s / (2 * w) as f64).sqrt(),
            (sg / (2 * w) as f64).sqrt(),
            (e / (2 * w) as f64).sqrt()
        );
    }
}
