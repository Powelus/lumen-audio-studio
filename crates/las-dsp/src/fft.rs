// SPDX-License-Identifier: GPL-3.0-or-later
//! In-place iterative radix-2 complex FFT, ported from `src/audio/dsp/fft.ts`.

use std::f64::consts::PI;

/// Transform `re` and `im` in place. `inverse` scales by 1/N. The length must
/// be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    assert_eq!(n, im.len());
    // bit reversal
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = (if inverse { 2.0 } else { -2.0 } * PI) / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let half = len >> 1;
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..half {
                let a = i + k;
                let b = a + half;
                let xr = re[b] * cr - im[b] * ci;
                let xi = re[b] * ci + im[b] * cr;
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
    if inverse {
        let n = n as f64;
        for i in 0..re.len() {
            re[i] /= n;
            im[i] /= n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_lands_in_one_bin_and_the_inverse_restores_it() {
        let n = 256;
        let mut re: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * 5.0 * i as f64 / n as f64).sin())
            .collect();
        let orig = re.clone();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im, false);
        // sin = (e^{ix} - e^{-ix}) / 2i: bins 5 and n-5 carry n/2
        assert!((im[5] + 128.0).abs() < 1e-9 && (im[n - 5] - 128.0).abs() < 1e-9);
        fft(&mut re, &mut im, true);
        for (a, b) in re.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-12);
        }
    }
}
