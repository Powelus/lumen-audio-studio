// SPDX-License-Identifier: GPL-3.0-or-later
//! Zero-latency partitioned convolution, for the convolution reverb.
//!
//! The first block of the impulse response is convolved directly, sample by
//! sample; the rest is cut into blocks and convolved in the frequency domain
//! (uniform partitioned overlap-save), using only blocks that are already
//! complete, so the output has no delay.

use crate::fft::fft;
use std::sync::Arc;

/// Samples per partition.
pub const BLOCK: usize = 256;
const BINS: usize = BLOCK + 1;

/// An impulse response prepared for convolution (shared between channels).
pub struct Impulse {
    /// The first block of the response, for the direct part.
    direct: Vec<f64>,
    /// Spectra (bins 0 to BLOCK) of the following blocks.
    parts: Vec<Vec<[f32; 2]>>,
}

impl Impulse {
    pub fn new(ir: &[f32]) -> Self {
        let mut direct = vec![0.0; BLOCK];
        for (d, x) in direct.iter_mut().zip(ir) {
            *d = f64::from(*x);
        }
        let mut parts = Vec::new();
        let mut at = BLOCK;
        while at < ir.len() {
            let mut re = vec![0.0; 2 * BLOCK];
            let mut im = vec![0.0; 2 * BLOCK];
            for (i, r) in re.iter_mut().enumerate().take(BLOCK) {
                *r = ir.get(at + i).map_or(0.0, |x| f64::from(*x));
            }
            fft(&mut re, &mut im, false);
            parts.push((0..BINS).map(|b| [re[b] as f32, im[b] as f32]).collect());
            at += BLOCK;
        }
        Self { direct, parts }
    }

    /// Number of frequency-domain partitions.
    pub fn partitions(&self) -> usize {
        self.parts.len()
    }
}

/// One channel of convolution with an [`Impulse`].
pub struct Convolver {
    ir: Arc<Impulse>,
    /// Recent input, for the direct part (a ring of BLOCK samples).
    hist: Vec<f64>,
    hist_pos: usize,
    /// The input of the previous and the current block, for the FFT.
    prev_block: Vec<f64>,
    cur_block: Vec<f64>,
    /// Spectra of the last blocks (newest first by `fdl_pos`).
    fdl: Vec<Vec<[f32; 2]>>,
    fdl_pos: usize,
    /// The frequency-domain part of the output for the current block.
    tail: Vec<f64>,
    pos: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    acc_re: Vec<f64>,
    acc_im: Vec<f64>,
}

impl Convolver {
    pub fn new(ir: Arc<Impulse>) -> Self {
        let p = ir.partitions();
        Self {
            hist: vec![0.0; BLOCK],
            hist_pos: 0,
            prev_block: vec![0.0; BLOCK],
            cur_block: vec![0.0; BLOCK],
            fdl: vec![vec![[0.0; 2]; BINS]; p.max(1)],
            fdl_pos: 0,
            tail: vec![0.0; BLOCK],
            pos: 0,
            re: vec![0.0; 2 * BLOCK],
            im: vec![0.0; 2 * BLOCK],
            acc_re: vec![0.0; 2 * BLOCK],
            acc_im: vec![0.0; 2 * BLOCK],
            ir,
        }
    }

    /// The frequency-domain part for the block that starts now.
    fn start_block(&mut self) {
        let p = self.ir.partitions();
        if p == 0 {
            return;
        }
        // spectrum of [previous block; the block that just ended]
        self.re[..BLOCK].copy_from_slice(&self.prev_block);
        self.re[BLOCK..].copy_from_slice(&self.cur_block);
        self.im.fill(0.0);
        fft(&mut self.re, &mut self.im, false);
        self.fdl_pos = (self.fdl_pos + p - 1) % p;
        for b in 0..BINS {
            self.fdl[self.fdl_pos][b] = [self.re[b] as f32, self.im[b] as f32];
        }
        // sum over the partitions: partition k pairs with the block k blocks back
        self.acc_re[..BINS].fill(0.0);
        self.acc_im[..BINS].fill(0.0);
        for k in 0..p {
            let x = &self.fdl[(self.fdl_pos + k) % p];
            let h = &self.ir.parts[k];
            for b in 0..BINS {
                let (xr, xi) = (f64::from(x[b][0]), f64::from(x[b][1]));
                let (hr, hi) = (f64::from(h[b][0]), f64::from(h[b][1]));
                self.acc_re[b] += xr * hr - xi * hi;
                self.acc_im[b] += xr * hi + xi * hr;
            }
        }
        // conjugate symmetry for a real result
        for b in 1..BLOCK {
            self.acc_re[2 * BLOCK - b] = self.acc_re[b];
            self.acc_im[2 * BLOCK - b] = -self.acc_im[b];
        }
        fft(&mut self.acc_re, &mut self.acc_im, true);
        self.tail.copy_from_slice(&self.acc_re[BLOCK..]);
    }

    pub fn process(&mut self, x: f64) -> f64 {
        // the direct part: the first block of the response against the recent input
        self.hist[self.hist_pos] = x;
        let mut y = 0.0;
        let n = BLOCK;
        for (k, h) in self.ir.direct.iter().enumerate() {
            y += h * self.hist[(self.hist_pos + n - k) % n];
        }
        self.hist_pos = (self.hist_pos + 1) % n;
        y += self.tail[self.pos];
        self.cur_block[self.pos] = x;
        self.pos += 1;
        if self.pos == BLOCK {
            self.pos = 0;
            self.start_block();
            std::mem::swap(&mut self.prev_block, &mut self.cur_block);
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct(ir: &[f32], x: &[f64]) -> Vec<f64> {
        (0..x.len())
            .map(|n| {
                ir.iter()
                    .enumerate()
                    .filter(|(k, _)| *k <= n)
                    .map(|(k, h)| f64::from(*h) * x[n - k])
                    .sum()
            })
            .collect()
    }

    #[test]
    fn matches_direct_convolution_with_no_delay() {
        // a response spanning several partitions
        let ir: Vec<f32> = (0..1500)
            .map(|i| ((i * 37 % 101) as f32 / 50.0 - 1.0) * (-(i as f32) / 600.0).exp())
            .collect();
        let x: Vec<f64> = (0..4000)
            .map(|i| ((i * 13 % 29) as f64 / 14.0 - 1.0) * 0.5)
            .collect();
        let want = direct(&ir, &x);
        let mut c = Convolver::new(Arc::new(Impulse::new(&ir)));
        let got: Vec<f64> = x.iter().map(|v| c.process(*v)).collect();
        let worst = got
            .iter()
            .zip(&want)
            .fold(0f64, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-3, "worst difference {worst}");
    }

    #[test]
    fn a_unit_impulse_response_passes_the_signal_through() {
        let mut c = Convolver::new(Arc::new(Impulse::new(&[1.0])));
        for i in 0..1000 {
            let x = (i as f64 * 0.01).sin();
            assert!((c.process(x) - x).abs() < 1e-9);
        }
    }
}
