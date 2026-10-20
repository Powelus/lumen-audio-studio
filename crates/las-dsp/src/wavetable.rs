// SPDX-License-Identifier: GPL-3.0-or-later
//! Band-limited, mip-mapped wavetables, ported from
//! `src/audio/dsp/wavetables.ts`.
//!
//! Every table consists of one or more frames. Each frame is stored at
//! [`MIP_LEVELS`] resolutions; level L contains at most `1024 >> L` harmonics, so
//! the oscillator can pick an alias-free level for the played frequency.
//!
//! Building a table takes a while (an FFT per frame and level), so tables are
//! made ahead of time by a [`TableLibrary`] and shared with `Arc`.

use crate::engine::js_round;
use crate::fft::fft;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

pub const BASE_SIZE: usize = 2048;
pub const MAX_HARMONICS: usize = BASE_SIZE / 2;
pub const MIP_LEVELS: usize = 11;

const TAU: f64 = PI * 2.0;

/// A wavetable: `mips[level][frame]` is one single-cycle waveform.
pub struct Table {
    pub frames: usize,
    pub mips: Vec<Vec<Vec<f32>>>,
}

/// How a built-in table is described: by its waveform or by its harmonics.
#[derive(Clone, Copy)]
enum Gen {
    /// `(frame position 0..1, x in the cycle 0..1) -> sample`
    Time(fn(f64, f64) -> f64),
    /// `(frame position 0..1, harmonic number) -> amplitude`
    Spectral(fn(f64, usize) -> f64),
}

struct Def {
    id: &'static str,
    frames: usize,
    gen: Gen,
}

fn saw(x: f64) -> f64 {
    2.0 * x - 1.0
}

fn sq(x: f64, w: f64) -> f64 {
    if x < w {
        1.0
    } else {
        -1.0
    }
}

fn tri(x: f64) -> f64 {
    if x < 0.25 {
        4.0 * x
    } else if x < 0.75 {
        2.0 - 4.0 * x
    } else {
        4.0 * x - 4.0
    }
}

/// xorshift32 with the same steps (and the same int32 behaviour) as the original.
fn seeded(seed: u32) -> impl FnMut() -> f64 {
    let mut s: u32 = if seed == 0 { 1 } else { seed };
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        f64::from(s % 100000) / 100000.0
    }
}

fn noise_spectra() -> &'static Vec<Vec<f64>> {
    static CELL: OnceLock<Vec<Vec<f64>>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut rnd = seeded(1337);
        (0..16)
            .map(|_| {
                (0..=256usize)
                    .map(|h| rnd() / (if h == 0 { 1.0 } else { h as f64 }).powf(0.6))
                    .collect()
            })
            .collect()
    })
}

const VOWELS: [[f64; 3]; 5] = [
    [800.0, 1150.0, 2900.0],
    [400.0, 1600.0, 2700.0],
    [270.0, 2140.0, 2950.0],
    [450.0, 800.0, 2830.0],
    [325.0, 700.0, 2530.0],
];

#[allow(clippy::needless_range_loop)]
fn formant_amp(frame: f64, h: usize) -> f64 {
    let f0 = 110.0;
    let pos = frame * (VOWELS.len() - 1) as f64;
    let i = ((VOWELS.len() - 2) as f64).min(pos.floor()) as usize;
    let t = pos - i as f64;
    let freq = h as f64 * f0;
    let mut a = 0.0;
    for k in 0..3 {
        let fc = VOWELS[i][k] * (1.0 - t) + VOWELS[i + 1][k] * t;
        let bw = 80.0 + fc * 0.08;
        let z = (freq - fc) / bw;
        let w = if k == 0 {
            1.0
        } else if k == 1 {
            0.6
        } else {
            0.35
        };
        a += (-(z * z)).exp() * w;
    }
    a / (h as f64).powf(0.15)
}

const DEFS: &[Def] = &[
    Def {
        id: "sine",
        frames: 1,
        gen: Gen::Time(|_, x| (TAU * x).sin()),
    },
    Def {
        id: "triangle",
        frames: 1,
        gen: Gen::Time(|_, x| tri(x)),
    },
    Def {
        id: "saw",
        frames: 1,
        gen: Gen::Spectral(|_, h| 1.0 / h as f64),
    },
    Def {
        id: "square",
        frames: 1,
        gen: Gen::Spectral(|_, h| if h % 2 == 1 { 1.0 / h as f64 } else { 0.0 }),
    },
    Def {
        id: "pulse25",
        frames: 1,
        gen: Gen::Time(|_, x| sq(x, 0.25)),
    },
    Def {
        id: "pulse10",
        frames: 1,
        gen: Gen::Time(|_, x| sq(x, 0.1)),
    },
    Def {
        id: "basic",
        frames: 32,
        gen: Gen::Time(|f, x| {
            let p = f * 3.0;
            let shapes = [(TAU * x).sin(), tri(x), saw(x), sq(x, 0.5)];
            let i = 2f64.min(p.floor()) as usize;
            let t = p - i as f64;
            shapes[i] * (1.0 - t) + shapes[i + 1] * t
        }),
    },
    Def {
        id: "pwm",
        frames: 32,
        gen: Gen::Time(|f, x| sq(x, 0.5 - f * 0.47)),
    },
    Def {
        id: "sync",
        frames: 32,
        gen: Gen::Time(|f, x| saw((x * (1.0 + f * 7.0)) % 1.0) * (1.0 - x * 0.15)),
    },
    Def {
        id: "analog",
        frames: 32,
        gen: Gen::Spectral(|f, h| {
            let h = h as f64;
            (1.0 / h) * (-h * (1.0 - f) * (1.0 - f) * 0.25).exp()
        }),
    },
    Def {
        id: "harmonics",
        frames: 32,
        gen: Gen::Spectral(|f, h| {
            if h as f64 <= 1.0 + (f * 63.0).floor() {
                1.0 / (h as f64).sqrt()
            } else {
                0.0
            }
        }),
    },
    Def {
        id: "odd",
        frames: 32,
        gen: Gen::Spectral(|f, h| {
            if h % 2 == 1 {
                1.0 / h as f64
            } else {
                f / h as f64
            }
        }),
    },
    Def {
        id: "organ",
        frames: 16,
        gen: Gen::Spectral(|f, h| {
            let bars = [1usize, 2, 3, 4, 6, 8, 10, 12, 16];
            match bars.iter().position(|b| *b == h) {
                None => 0.0,
                Some(idx) => 0.5 + 0.5 * (TAU * (f * 1.5 + idx as f64 * 0.13)).cos(),
            }
        }),
    },
    Def {
        id: "formant",
        frames: 32,
        gen: Gen::Spectral(formant_amp),
    },
    Def {
        id: "choir",
        frames: 16,
        gen: Gen::Spectral(|f, h| {
            formant_amp(f * 0.5, h) * (0.6 + 0.4 * (h as f64 * 0.7 + f * 4.0).sin())
        }),
    },
    Def {
        id: "fm",
        frames: 32,
        gen: Gen::Time(|f, x| (TAU * x + f * 8.0 * (TAU * 2.0 * x).sin()).sin()),
    },
    Def {
        id: "fold",
        frames: 32,
        gen: Gen::Time(|f, x| ((1.0 + f * 7.0) * (TAU * x).sin()).sin()),
    },
    Def {
        id: "growl",
        frames: 32,
        gen: Gen::Time(|f, x| {
            let m = (TAU * 3.0 * x).sin() * (2.0 + f * 6.0);
            (2.0 * (TAU * x + m).sin() + saw(x) * f).tanh()
        }),
    },
    Def {
        id: "digital",
        frames: 16,
        gen: Gen::Time(|f, x| {
            let r = js_round(64.0 / 2f64.powf(f * 5.0));
            let steps = if r == 0.0 { 2.0 } else { r };
            saw((x * steps).floor() / steps)
        }),
    },
    Def {
        id: "phasedist",
        frames: 32,
        gen: Gen::Time(|f, x| {
            let r = 1.0 + f * 14.0;
            (TAU * ((x * r) % 1.0)).cos() * (1.0 - x)
        }),
    },
    Def {
        id: "bell",
        frames: 16,
        gen: Gen::Spectral(|f, h| {
            if [1usize, 3, 5, 9, 12, 17].contains(&h) {
                (-(h as f64) * 0.06 * (1.0 - f)).exp()
            } else {
                0.0
            }
        }),
    },
    Def {
        id: "noise",
        frames: 16,
        gen: Gen::Spectral(|f, h| {
            if h <= 256 {
                noise_spectra()[15usize.min((f * 15.999).floor() as usize)][h]
            } else {
                0.0
            }
        }),
    },
    Def {
        id: "reese",
        frames: 16,
        gen: Gen::Time(|f, x| {
            0.5 * (saw(x) + saw((x * 2.0 + f * 0.5) % 1.0) * 0.6)
                + 0.3 * sq((x + f * 0.25) % 1.0, 0.5)
        }),
    },
];

/// The ids of the built-in tables.
pub fn table_ids() -> impl Iterator<Item = &'static str> {
    DEFS.iter().map(|d| d.id)
}

fn mip_size(level: usize) -> usize {
    let h = MAX_HARMONICS >> level;
    256usize.max(BASE_SIZE.min(h * 4))
}

/// Most harmonics a mip level holds.
pub fn max_harmonics(level: usize) -> usize {
    1usize.max(MAX_HARMONICS >> level)
}

fn raw_spectrum(def: &Def, frame: usize) -> (Vec<f64>, Vec<f64>) {
    let mut re = vec![0.0; BASE_SIZE];
    let mut im = vec![0.0; BASE_SIZE];
    let f = if def.frames > 1 {
        frame as f64 / (def.frames - 1) as f64
    } else {
        0.0
    };
    match def.gen {
        Gen::Spectral(spectral) => {
            // sine phase (imaginary part) so a saw spectrum renders as a falling saw
            for h in 1..MAX_HARMONICS {
                let a = spectral(f, h);
                if a == 0.0 {
                    continue;
                }
                im[h] = -a * (BASE_SIZE / 2) as f64;
                im[BASE_SIZE - h] = a * (BASE_SIZE / 2) as f64;
            }
        }
        Gen::Time(time) => {
            for (i, r) in re.iter_mut().enumerate() {
                *r = time(f, i as f64 / BASE_SIZE as f64);
            }
            fft(&mut re, &mut im, false);
        }
    }
    (re, im)
}

fn spectrum_from_samples(samples: &[f32]) -> (Vec<f64>, Vec<f64>) {
    let mut re = vec![0.0; BASE_SIZE];
    let mut im = vec![0.0; BASE_SIZE];
    // resample to BASE_SIZE with linear interpolation
    let n = samples.len();
    for (i, r) in re.iter_mut().enumerate() {
        let p = (i as f64 / BASE_SIZE as f64) * n as f64;
        let i0 = p.floor();
        let t = p - i0;
        let i0 = i0 as usize;
        *r = f64::from(samples[i0 % n]) * (1.0 - t) + f64::from(samples[(i0 + 1) % n]) * t;
    }
    fft(&mut re, &mut im, false);
    (re, im)
}

fn build_mips(spec: &(Vec<f64>, Vec<f64>)) -> Vec<Vec<f32>> {
    let (sre, sim) = spec;
    let mut levels = Vec::with_capacity(MIP_LEVELS);
    let mut norm = 0.0;
    for level in 0..MIP_LEVELS {
        let size = mip_size(level);
        let hmax = max_harmonics(level).min(size / 2 - 1);
        let mut re = vec![0.0; size];
        let mut im = vec![0.0; size];
        let scale = size as f64 / BASE_SIZE as f64;
        for h in 1..=hmax {
            re[h] = sre[h] * scale;
            im[h] = sim[h] * scale;
            re[size - h] = sre[BASE_SIZE - h] * scale;
            im[size - h] = sim[BASE_SIZE - h] * scale;
        }
        fft(&mut re, &mut im, true);
        if level == 0 {
            let peak = re.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            norm = if peak > 1e-9 { 1.0 / peak } else { 0.0 };
        }
        levels.push(re.iter().map(|x| (x * norm) as f32).collect());
    }
    levels
}

fn assemble(per_frame: Vec<Vec<Vec<f32>>>) -> Table {
    let frames = per_frame.len();
    let mips = (0..MIP_LEVELS)
        .map(|level| per_frame.iter().map(|f| f[level].clone()).collect())
        .collect();
    Table { frames, mips }
}

/// Build a table from user-supplied single-cycle frames.
pub fn table_from_frames(frames: &[Vec<f32>]) -> Table {
    assemble(
        frames
            .iter()
            .map(|f| build_mips(&spectrum_from_samples(f)))
            .collect(),
    )
}

fn build_builtin(def: &Def) -> Table {
    assemble(
        (0..def.frames)
            .map(|fr| build_mips(&raw_spectrum(def, fr)))
            .collect(),
    )
}

/// Builds tables once and shares them. Unknown ids give the saw table, as in
/// the original.
#[derive(Default, Clone)]
pub struct TableLibrary {
    cache: HashMap<String, Arc<Table>>,
    user: HashMap<String, Arc<Table>>,
}

impl TableLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a user table; it wins over a built-in of the same id.
    pub fn register(&mut self, id: &str, table: Table) {
        self.user.insert(id.to_string(), Arc::new(table));
    }

    pub fn get(&mut self, id: &str) -> Arc<Table> {
        if let Some(t) = self.user.get(id) {
            return t.clone();
        }
        if let Some(t) = self.cache.get(id) {
            return t.clone();
        }
        let def = DEFS.iter().find(|d| d.id == id).unwrap_or(&DEFS[2]);
        let t = Arc::new(build_builtin(def));
        self.cache.insert(id.to_string(), t.clone());
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_builds_and_is_normalised() {
        let mut lib = TableLibrary::new();
        for id in table_ids() {
            let t = lib.get(id);
            assert!(t.frames >= 1);
            assert_eq!(t.mips.len(), MIP_LEVELS);
            let peak = t.mips[0][0].iter().fold(0f32, |m, x| m.max(x.abs()));
            assert!((peak - 1.0).abs() < 1e-4, "{id}: peak {peak}");
            assert_eq!(t.mips[0][0].len(), 2048);
            assert_eq!(t.mips[10][0].len(), 256);
        }
    }

    #[test]
    fn higher_mips_have_fewer_harmonics() {
        let mut lib = TableLibrary::new();
        let t = lib.get("saw");
        // the top mip level is nearly a sine: far less high-frequency content
        let rough =
            |v: &[f32]| v.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / v.len() as f32;
        assert!(rough(&t.mips[10][0]) < rough(&t.mips[0][0]) * 4.0);
        let t0: f32 = t.mips[0][0].iter().map(|x| x.abs()).sum::<f32>() / 2048.0;
        assert!(t0 > 0.3);
    }

    #[test]
    fn unknown_ids_fall_back_to_the_saw() {
        let mut lib = TableLibrary::new();
        let a = lib.get("saw");
        let b = lib.get("nope");
        assert_eq!(a.mips[0][0], b.mips[0][0]);
    }

    #[test]
    fn user_tables_win_and_are_normalised() {
        let mut lib = TableLibrary::new();
        let cycle: Vec<f32> = (0..512)
            .map(|i| ((i as f32) / 512.0 * 6.2831855).sin() * 0.25)
            .collect();
        lib.register("mine", table_from_frames(&[cycle]));
        let t = lib.get("mine");
        let peak = t.mips[0][0].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!((peak - 1.0).abs() < 1e-3);
    }
}
