// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! The wavetables match the ones built by the TypeScript code of the web
//! version. The reference numbers come from `tools/parity/wavetable.gen.test.ts`.

use las_dsp::wavetable::{table_ids, TableLibrary};

#[test]
fn wavetables_match_the_typescript_ones() {
    let path = format!(
        "{}/tests/fixtures/wavetables.f32",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let want: Vec<f32> = bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let mut lib = TableLibrary::new();
    let mut ids: Vec<&str> = table_ids().collect();
    ids.push("no-such-table");
    let mut at = 0;
    let mut worst = 0f64;
    for id in &ids {
        let t = lib.get(id);
        let frames = [0, (t.frames - 1) / 2, t.frames - 1];
        for level in [0usize, 4] {
            for f in frames {
                for i in 0..128 {
                    let got = t.mips[level][f][i];
                    let d = f64::from(got - want[at]).abs();
                    worst = worst.max(d);
                    assert!(
                        d < 1e-5,
                        "{id} level {level} frame {f} sample {i}: {got} vs {}",
                        want[at]
                    );
                    at += 1;
                }
            }
        }
    }
    assert_eq!(at, want.len(), "the reference has other tables");
    println!("{} tables, worst difference {worst:.2e}", ids.len());
}
