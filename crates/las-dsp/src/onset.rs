// SPDX-License-Identifier: GPL-3.0-or-later
//! Beat (transient) detection and tempo detection for loops, as the slicer and the
//! sample editor use them.
//!
//! Onsets are found from the rise of a short-time log energy envelope: a frame whose
//! level jumps above the level just before it by more than a threshold (set by the
//! sensitivity) and is the biggest jump nearby starts a new slice. Each onset is then
//! moved back to where the sound really starts, so a slice holds the whole attack.

/// Where the transients of some mono audio are, in frames, ascending. `sensitivity`
/// is 0 to 1: higher finds quieter hits. The very start of the audio is never
/// reported (a slicer always starts a slice there).
pub fn onsets(mono: &[f32], sample_rate: f64, sensitivity: f32) -> Vec<usize> {
    let sens = sensitivity.clamp(0.0, 1.0);
    let hop = ((sample_rate * 0.005).round() as usize).max(16);
    let win = hop * 2;
    if mono.len() < win * 2 {
        return Vec::new();
    }
    let frames = mono.len().div_ceil(hop);
    // the jump in level a hit needs, dB: 14 at the least sensitive, 3 at the most
    let jump = 14.0 - 11.0 * sens;
    // the whole signal, and its top end (a hat over a ringing kick shows there)
    let high: Vec<f32> = std::iter::once(0.0)
        .chain(mono.windows(2).map(|w| w[1] - w[0]))
        .collect();
    let full = rises(mono, frames, hop, win, sens);
    let top = rises(&high, frames, hop, win, sens);
    let rise: Vec<f32> = full.iter().zip(&top).map(|(a, b)| a.max(*b)).collect();
    let min_gap = ((sample_rate * 0.06) / hop as f64).ceil() as usize;
    let mut out: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    let mut f = 1;
    while f < frames {
        if rise[f] >= jump {
            let end = (f + 4).min(frames);
            if last.is_none_or(|l| f - l >= min_gap) {
                let at = refine(mono, f * hop, hop, win);
                if at > hop && out.last().is_none_or(|&p| at > p + hop) {
                    out.push(at);
                    last = Some(f);
                }
            }
            f = end.max(f + 1);
            continue;
        }
        f += 1;
    }
    out
}

/// How far each frame's level rises above the quietest of the frames just before it
/// (a hit can take a few frames to reach its peak), dB; 0 for frames too quiet to count.
fn rises(x: &[f32], frames: usize, hop: usize, win: usize, sens: f32) -> Vec<f32> {
    let db: Vec<f32> = (0..frames)
        .map(|f| {
            let a = f * hop;
            let z = (a + win).min(x.len());
            let e: f32 = x[a..z].iter().map(|v| v * v).sum::<f32>() / win as f32;
            10.0 * (e + 1e-12).log10()
        })
        .collect();
    let loudest = db.iter().copied().fold(f32::MIN, f32::max);
    // quieter than this is noise, whatever its rises
    let floor = loudest.max(-90.0) - (24.0 + 36.0 * sens);
    (0..frames)
        .map(|f| {
            let before = db[f.saturating_sub(4)..f]
                .iter()
                .copied()
                .fold(f32::MAX, f32::min);
            if f == 0 || db[f] < floor {
                0.0
            } else {
                (db[f] - before).max(0.0)
            }
        })
        .collect()
}

/// Where a hit found in the frame starting at `at` really starts: the first sample
/// that is a fair part of the hit's level, moved back to the zero crossing before it.
fn refine(mono: &[f32], at: usize, hop: usize, win: usize) -> usize {
    let a = at.saturating_sub(hop);
    let z = (at + win + hop).min(mono.len());
    let peak = mono[a..z].iter().fold(0f32, |m, v| m.max(v.abs()));
    if peak <= 0.0 {
        return at;
    }
    let mut first = mono[a..z]
        .iter()
        .position(|v| v.abs() >= peak * 0.2)
        .map_or(at, |i| a + i);
    // back to a zero crossing, at most a millisecond or so
    let limit = first.saturating_sub(hop / 4);
    while first > limit && first > 0 && mono[first - 1].signum() == mono[first].signum() {
        if mono[first - 1].abs() > mono[first].abs() {
            break;
        }
        first -= 1;
    }
    first
}

/// A loop's tempo from its onsets (frames) and its length: the tempo, between 85 and
/// 170 BPM, whose beat grid the onsets fit best, preferring one that makes the loop a
/// whole number of beats (as loops are cut). `None` when there is too little to go on.
pub fn detect_tempo(onsets: &[usize], sample_rate: f64, frames: usize) -> Option<f64> {
    let len = frames as f64 / sample_rate;
    if len <= 0.0 {
        return None;
    }
    let (lo, hi) = (85.0, 170.0);
    // a tempo that makes the loop a whole number of beats
    let by_length: Vec<f64> = (1..=128)
        .map(|n| 60.0 * f64::from(n) / len)
        .filter(|b| (lo..hi).contains(b))
        .collect();
    let mut times: Vec<f64> = vec![0.0];
    times.extend(onsets.iter().map(|&o| o as f64 / sample_rate));
    if times.len() < 3 {
        return by_length
            .iter()
            .copied()
            .min_by(|a, b| (a - 120.0).abs().total_cmp(&(b - 120.0).abs()));
    }
    let score = |bpm: f64| -> f64 {
        let beat = 60.0 / bpm;
        let mut total = 0.0;
        for (h, w) in [(1.0, 1.0), (2.0, 0.5), (4.0, 0.25)] {
            let (mut re, mut im) = (0.0, 0.0);
            for t in &times {
                let ph = std::f64::consts::TAU * h * t / beat;
                re += ph.cos();
                im += ph.sin();
            }
            total += w * (re * re + im * im).sqrt() / times.len() as f64;
        }
        total
    };
    let mut best = (lo, f64::MIN);
    let mut b = lo;
    while b < hi {
        let s = score(b);
        if s > best.1 {
            best = (b, s);
        }
        b += 0.05;
    }
    // a whole number of beats that fits nearly as well wins: it is exact
    let fit = by_length
        .iter()
        .map(|&b| (b, score(b)))
        .filter(|&(_, s)| s >= best.1 * 0.9)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let bpm = fit.map_or(best.0, |f| f.0);
    Some((bpm * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decaying tone bursts at `hits` (seconds) with the given levels.
    fn hits(hits: &[(f64, f32)], seconds: f64, sr: f64) -> Vec<f32> {
        let n = (seconds * sr) as usize;
        let mut out = vec![0f32; n];
        for &(t, level) in hits {
            let a = (t * sr) as usize;
            for (k, o) in out[a.min(n)..].iter_mut().enumerate() {
                let x = k as f64 / sr;
                *o +=
                    level * ((x * 330.0 * std::f64::consts::TAU).sin() * (-x * 14.0).exp()) as f32;
            }
        }
        out
    }

    #[test]
    fn the_hits_of_a_loop_are_found_where_they_start() {
        let sr = 44_100.0;
        let at: Vec<(f64, f32)> = (0..8).map(|i| (f64::from(i) * 0.5, 0.8)).collect();
        let x = hits(&at, 4.0, sr);
        let found = onsets(&x, sr, 0.5);
        assert_eq!(found.len(), 7, "{found:?}");
        for (k, f) in found.iter().enumerate() {
            let want = (k + 1) as f64 * 0.5 * sr;
            assert!((*f as f64 - want).abs() < sr * 0.003, "{f} vs {want}");
        }
    }

    #[test]
    fn sensitivity_decides_whether_quiet_hits_count() {
        let sr = 44_100.0;
        let x = hits(&[(0.0, 0.9), (0.5, 0.9), (1.0, 0.04), (1.5, 0.9)], 2.0, sr);
        assert_eq!(onsets(&x, sr, 1.0).len(), 3);
        assert_eq!(onsets(&x, sr, 0.0).len(), 2);
    }

    #[test]
    fn silence_has_no_onsets() {
        assert!(onsets(&vec![0.0; 44_100], 44_100.0, 1.0).is_empty());
    }

    #[test]
    fn the_tempo_of_a_loop_is_detected() {
        let sr = 44_100.0;
        // quarter notes at 120 BPM over two bars
        let q: Vec<usize> = (1..16)
            .map(|i| (f64::from(i) * 0.5 * sr) as usize)
            .collect();
        assert_eq!(detect_tempo(&q, sr, (8.0 * sr) as usize), Some(120.0));
        // eighth notes at 100 BPM, one bar, with a few missing
        let beat = 0.6;
        let e: Vec<usize> = (1..8)
            .filter(|i| *i != 3 && *i != 6)
            .map(|i| (f64::from(i) * beat / 2.0 * sr) as usize)
            .collect();
        assert_eq!(
            detect_tempo(&e, sr, (4.0 * beat * sr) as usize),
            Some(100.0)
        );
        // found from the onsets of real audio
        let at: Vec<(f64, f32)> = (0..8).map(|i| (f64::from(i) * 60.0 / 140.0, 0.8)).collect();
        let x = hits(&at, 8.0 * 60.0 / 140.0, sr);
        let t = detect_tempo(&onsets(&x, sr, 0.5), sr, x.len()).unwrap();
        assert!((t - 140.0).abs() < 0.05, "{t}");
    }
}
