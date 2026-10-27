// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! The loudness meter reads the same numbers as the TypeScript one. The golden
//! values were made by running `src/audio/dsp/loudness.ts` of the web version on
//! the effects test signal (`tests/fixtures/fx_input.f32`): a plain meter run on
//! the half-second signal, and `analyzeLoudness` on twelve repeats of it with
//! level steps (x1, x0.2, x0.02), which exercises the gating and the range.

use las_dsp::loudness::{analyze_loudness, correlation, LoudnessMeter};

fn input() -> (Vec<f32>, Vec<f32>) {
    let path = format!("{}/tests/fixtures/fx_input.f32", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).expect("fx_input.f32");
    let all: Vec<f32> = bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let n = all.len() / 4;
    (all[..n].to_vec(), all[n..2 * n].to_vec())
}

fn close(name: &str, got: f64, want: f64) {
    assert!((got - want).abs() < 1e-9, "{name}: {got} vs {want}");
}

#[test]
fn the_meter_matches_the_typescript_one() {
    let (l, r) = input();
    let mut m = LoudnessMeter::new(48000.0, 2);
    m.process(&[&l, &r], l.len());
    close("momentary", m.momentary(), -8.282070211169673);
    close("short term", m.short_term(), -8.378390979749552);
    close("integrated", m.integrated(), -8.358159616114232);
    close("sample peak", m.sample_peak_db(), 0.7253244252764529);
    close("true peak", m.true_peak_db(), 1.0003122272965088);
    close("correlation", correlation(&l, &r), 0.11369772462714005);

    let rep = 12;
    let n = l.len();
    let (mut bl, mut br) = (Vec::with_capacity(n * rep), Vec::with_capacity(n * rep));
    for k in 0..rep {
        let g: f64 = if k < 6 {
            1.0
        } else if k < 9 {
            0.2
        } else {
            0.02
        };
        // scaled in f64 and stored as f32, as the original does
        bl.extend(l.iter().map(|x| (f64::from(*x) * g) as f32));
        br.extend(r.iter().map(|x| (f64::from(*x) * g) as f32));
    }
    let s = analyze_loudness(&[&bl, &br], 48000.0);
    close("integrated (steps)", s.integrated, -8.694396803976044);
    close("range", s.range, 8.839756515501454);
    close("max momentary", s.max_momentary, -8.281751906360817);
    close("max short term", s.max_short_term, -8.379589071809953);
    close("sample peak (steps)", s.sample_peak, 0.7253244252764529);
    close("true peak (steps)", s.true_peak, 1.0003122272965088);
}
