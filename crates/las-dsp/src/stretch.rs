// SPDX-License-Identifier: GPL-3.0-or-later
//! Time stretch without a change of pitch: WSOLA (waveform similarity overlap
//! add). Port of the web version's `audio/dsp/stretch.ts`.
//!
//! The output is built from Hann windowed frames at a fixed hop. Each frame is
//! read from the input near its nominal position (output position over the
//! factor), moved by up to a quarter frame so it lines up with the natural
//! continuation of the previous frame, which keeps periodic sounds in phase.
//! All channels use the same positions, so the stereo image stays intact.
//! Transients are locked: a frame that holds an onset is read so the onset
//! lands exactly at its stretched time, which keeps drums from flamming.
//!
//! Arithmetic follows the original's: samples are `f32`, sums are `f64`.

#![allow(clippy::needless_range_loop)]

use std::f64::consts::PI;

/// Output length over input length: 2 is twice as long (half speed).
pub fn stretch_length(input_length: usize, factor: f64) -> usize {
    ((input_length as f64 * factor).round() as usize).max(1)
}

/// `Math.round`.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// An onset: its sample position and strength (energy jump).
#[derive(Clone, Copy, Debug)]
pub struct Onset {
    pub at: usize,
    pub strength: f64,
}

/// Onsets across all channels.
pub fn find_onsets(chans: &[Vec<f32>]) -> Vec<Onset> {
    const B: usize = 256;
    let len = chans.first().map_or(0, Vec::len);
    let n = len / B;
    let mut e = vec![0f32; n];
    let mut max = 0f32;
    for b in 0..n {
        let mut s = 0f64;
        for x in chans {
            for &v in &x[b * B..(b + 1) * B] {
                s += f64::from(v) * f64::from(v);
            }
        }
        e[b] = (s / B as f64) as f32;
        if e[b] > max {
            max = e[b];
        }
    }
    let amp = |i: usize| -> f32 {
        let mut m = 0f32;
        for x in chans {
            m = m.max(x[i].abs());
        }
        m
    };
    let floor = f64::from(max) * 1e-4;
    let mut out = Vec::new();
    let mut b = 0usize;
    while b < n {
        let mut prev = 0f32;
        for k in b.saturating_sub(4)..b {
            prev = prev.max(e[k]);
        }
        if f64::from(e[b]) <= floor || f64::from(e[b]) <= f64::from(prev) * 4.0 {
            b += 1;
            continue;
        }
        // the onset is the first sample that reaches half the block's peak
        let mut pk = 0f32;
        for i in b * B..(b + 1) * B {
            pk = pk.max(amp(i));
        }
        let mut at = b * B;
        for i in (b.saturating_sub(1) * B)..(b + 1) * B {
            if f64::from(amp(i)) >= f64::from(pk) * 0.5 {
                at = i;
                break;
            }
        }
        out.push(Onset {
            at,
            strength: f64::from(e[b]) / (f64::from(prev) + floor),
        });
        b += 4;
    }
    out
}

/// Stretch `input` by `factor` (the output is `factor` times as long).
pub fn time_stretch(input: &[Vec<f32>], sample_rate: f64, factor: f64) -> Vec<Vec<f32>> {
    if (factor - 1.0).abs() < 1e-6 {
        return input.to_vec();
    }
    let in_len = input.first().map_or(0, Vec::len);
    let out_len = stretch_length(in_len, factor);
    let mut out: Vec<Vec<f32>> = input.iter().map(|_| vec![0f32; out_len]).collect();
    let frame = 0.04;
    let n = 2f64.powf(js_round((256f64).max(sample_rate * frame).log2())) as usize;
    let ha = n as f64 / 2.0;
    let hs = ha / factor;
    let tol = (n / 4) as i64;
    let ov = n / 2;
    let win: Vec<f32> = (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32)
        .collect();
    let mut wsum = vec![0f32; out_len + n];
    // mono guide for the similarity search
    let mut mono = vec![0f32; in_len];
    for ch in input {
        for i in 0..in_len {
            mono[i] = (f64::from(mono[i]) + f64::from(ch[i])) as f32;
        }
    }
    let at = |i: i64| -> f64 {
        if i >= 0 && (i as usize) < in_len {
            f64::from(mono[i as usize])
        } else {
            0.0
        }
    };
    const DEC: usize = 4;
    let score = |p: i64, q: i64| -> f64 {
        let mut xy = 0f64;
        let mut yy = 1e-9;
        let mut i = 0usize;
        while i < ov {
            let y = at(p + i as i64);
            xy += y * at(q + i as i64);
            yy += y * y;
            i += DEC;
        }
        xy / yy.sqrt()
    };
    let onsets = find_onsets(input);
    let mut next = 0usize;
    let mut prev = 0i64;
    let frames = (out_len as f64 / ha).ceil() as usize + 1;
    for k in 0..frames {
        let nominal = js_round(k as f64 * hs) as i64;
        let mut pos = nominal;
        // an onset whose stretched time falls in this frame fixes the alignment
        let o0 = (k as f64 * ha) as i64;
        while next < onsets.len() && (onsets[next].at as f64 * factor) < o0 as f64 {
            next += 1;
        }
        let mut lock: i64 = -1;
        let mut lock_w = 0f64;
        let mut after = next;
        let mut j = next;
        while j < onsets.len() && (onsets[j].at as f64 * factor) < (o0 + n as i64) as f64 {
            let o = js_round(onsets[j].at as f64 * factor) as i64;
            let wi = (o - o0).clamp(0, n as i64 - 1) as usize;
            let w = onsets[j].strength * f64::from(win[wi]);
            if w > lock_w {
                lock_w = w;
                lock = onsets[j].at as i64 - (o - o0);
            }
            j += 1;
            after += 1;
        }
        if lock != -1 && lock_w > 0.0 {
            pos = lock;
        } else if k > 0 {
            let natural = prev + ha as i64;
            let lo = (-nominal).max(-tol);
            // coarse search, then refine around the best lag
            let mut best = 0i64;
            let mut best_s = f64::NEG_INFINITY;
            let mut d = lo;
            while d <= tol {
                let s = score(nominal + d, natural);
                if s > best_s {
                    best_s = s;
                    best = d;
                }
                d += 4;
            }
            let c = best;
            let mut d = lo.max(c - 3);
            while d <= tol.min(c + 3) {
                let s = score(nominal + d, natural);
                if s > best_s {
                    best_s = s;
                    best = d;
                }
                d += 1;
            }
            pos = nominal + best;
        }
        if lock == -1 {
            // a frame that cannot place an onset at its stretched time must not read it:
            // start after the last onset before this frame, end before the next one after it
            if next > 0 && pos <= onsets[next - 1].at as i64 {
                pos = onsets[next - 1].at as i64 + 1;
            }
            if after < onsets.len() && pos + n as i64 > onsets[after].at as i64 {
                pos = onsets[after].at as i64 - n as i64;
            }
        }
        prev = pos;
        let o = (k as f64 * ha) as usize;
        for (ch, dst) in out.iter_mut().enumerate() {
            let src = &input[ch];
            for i in 0..n {
                let oi = o + i;
                if oi >= out_len {
                    break;
                }
                let si = pos + i as i64;
                if si >= 0 && (si as usize) < in_len {
                    dst[oi] = (f64::from(dst[oi]) + f64::from(src[si as usize]) * f64::from(win[i]))
                        as f32;
                }
            }
        }
        for i in 0..n {
            if o + i >= out_len {
                break;
            }
            wsum[o + i] = (f64::from(wsum[o + i]) + f64::from(win[i])) as f32;
        }
    }
    // normalise the window sum (only matters at the very edges)
    for i in 0..out_len {
        let w = f64::from(wsum[i]);
        let g = if w > 1e-3 { 1.0 / w } else { 0.0 };
        for ch in &mut out {
            ch[i] = (f64::from(ch[i]) * g) as f32;
        }
    }
    out
}
