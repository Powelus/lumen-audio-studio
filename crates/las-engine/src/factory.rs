// SPDX-License-Identifier: GPL-3.0-or-later
//! The factory sample kit, made from maths so the app ships without any
//! recorded audio. Every sample is deterministic (seeded noise), so a project
//! that refers to `factory:*` ids sounds the same everywhere. Port of the web
//! version's `audio/factory.ts`.

use std::f64::consts::{PI, TAU};

/// A sample's audio: one or two channels.
#[derive(Clone, Debug)]
pub struct Audio {
    pub channels: Vec<Vec<f32>>,
    pub sample_rate: f64,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
}

pub struct FactorySample {
    pub id: &'static str,
    pub name: &'static str,
    pub folder: &'static str,
    render: fn(f64) -> Vec<Vec<f32>>,
}

impl FactorySample {
    pub fn render(&self, sample_rate: f64) -> Audio {
        Audio {
            channels: (self.render)(sample_rate),
            sample_rate,
        }
    }
}

pub fn is_factory_id(id: &str) -> bool {
    id.starts_with("factory:")
}

/// The sample with this id, rendered at `sample_rate`.
pub fn render_factory(id: &str, sample_rate: f64) -> Option<Audio> {
    FACTORY_SAMPLES
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.render(sample_rate))
}

/// xorshift32, mapped to -1 to 1 (the original's `rng`).
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> f64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        (f64::from(s) / 4294967296.0) * 2.0 - 1.0
    }
}

#[derive(Clone, Copy)]
enum Band {
    Lp,
    Hp,
    Bp,
}

/// The original's small biquad (direct form I, `f64`).
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    a0: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn new(kind: Band, freq: f64, q: f64, sr: f64) -> Self {
        let w0 = (TAU * freq) / sr;
        let alpha = w0.sin() / (2.0 * q);
        let cos = w0.cos();
        let (b0, b1, b2) = match kind {
            Band::Lp => ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0),
            Band::Hp => ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0),
            Band::Bp => (alpha, 0.0, -alpha),
        };
        Self {
            b0,
            b1,
            b2,
            a0: 1.0 + alpha,
            a1: -2.0 * cos,
            a2: 1.0 - alpha,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = (self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2)
            / self.a0;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn normalize(chs: &mut [Vec<f32>], peak: f64) {
    let mut m = 0f32;
    for c in chs.iter() {
        for &v in c {
            m = m.max(v.abs());
        }
    }
    if m > 0.0 {
        // the original scales in f64 and stores f32
        for c in chs.iter_mut() {
            for v in c.iter_mut() {
                *v = (f64::from(*v) * (peak / f64::from(m))) as f32;
            }
        }
    }
}

/// One channel from a function of time, with a 4 ms fade out, normalised to 0.95.
fn mono(len: f64, sr: f64, mut f: impl FnMut(f64) -> f64) -> Vec<Vec<f32>> {
    let n = (len * sr).floor() as usize;
    let mut out: Vec<f32> = (0..n).map(|i| f(i as f64 / sr) as f32).collect();
    let fade = n.min((sr * 0.004).floor() as usize);
    for i in 0..fade {
        let at = n - 1 - i;
        out[at] = (f64::from(out[at]) * (i as f64 / fade as f64)) as f32;
    }
    let mut chs = vec![out];
    normalize(&mut chs, 0.95);
    chs
}

#[derive(Clone, Copy)]
struct KickSpec {
    len: f64,
    f0: f64,
    f1: f64,
    pitch_decay: f64,
    amp_decay: f64,
    click: f64,
    drive: f64,
    seed: u32,
}

fn kick_render(o: KickSpec, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(o.seed);
    let mut phase = 0.0;
    let mut lp = Biquad::new(Band::Lp, 3000.0, 0.7, sr);
    mono(o.len, sr, |t| {
        let f = o.f1 + (o.f0 - o.f1) * (-t / o.pitch_decay).exp();
        phase += f / sr;
        let body = (TAU * phase).sin() * (-t / o.amp_decay).exp();
        let click = lp.run(r.next()) * (-t / 0.003).exp() * o.click;
        ((body + click) * o.drive).tanh()
    })
}

#[derive(Clone, Copy)]
struct SnareSpec {
    tone: f64,
    tone_decay: f64,
    noise_decay: f64,
    noise_freq: f64,
    mix: f64,
    len: f64,
    seed: u32,
}

fn snare_render(o: SnareSpec, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(o.seed);
    let mut bp = Biquad::new(Band::Bp, o.noise_freq, 0.8, sr);
    let mut hp = Biquad::new(Band::Hp, 900.0, 0.7, sr);
    let (mut p1, mut p2) = (0.0, 0.0);
    mono(o.len, sr, |t| {
        let f = o.tone * (1.0 + 0.6 * (-t / 0.01).exp());
        p1 += f / sr;
        p2 += (f * 1.47) / sr;
        let tone = ((TAU * p1).sin() + 0.5 * (TAU * p2).sin()) * (-t / o.tone_decay).exp();
        let noise = hp.run(bp.run(r.next()) * 3.0) * (-t / o.noise_decay).exp();
        (tone * (1.0 - o.mix) + noise * o.mix * 2.0).tanh()
    })
}

#[derive(Clone, Copy)]
struct ClapSpec {
    len: f64,
    decay: f64,
    freq: f64,
    seed: u32,
    bursts: u32,
}

fn clap_render(o: ClapSpec, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(o.seed);
    let mut bp = Biquad::new(Band::Bp, o.freq, 1.4, sr);
    mono(o.len, sr, |t| {
        let mut env = 0f64;
        for b in 0..o.bursts {
            let bt = t - f64::from(b) * 0.011;
            if bt >= 0.0 {
                let tau = if b == o.bursts - 1 { o.decay } else { 0.006 };
                env = env.max((-bt / tau).exp());
            }
        }
        bp.run(r.next()) * env * 3.0
    })
}

#[derive(Clone, Copy)]
struct HatSpec {
    len: f64,
    decay: f64,
    hp: f64,
    seed: u32,
    metal: f64,
}

fn hat_channel(o: HatSpec, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(o.seed);
    let mut hp = Biquad::new(Band::Hp, o.hp, 0.9, sr);
    let ratios = [2.0, 3.0, 4.16, 5.43, 6.79, 8.21];
    let mut phases = [0f64; 6];
    mono(o.len, sr, |t| {
        let mut m = 0.0;
        for (k, ratio) in ratios.iter().enumerate() {
            phases[k] += (ratio * 330.0) / sr;
            m += if phases[k] % 1.0 < 0.5 { 1.0 } else { -1.0 };
        }
        let x = r.next() * (1.0 - o.metal) + (m / ratios.len() as f64) * o.metal;
        hp.run(x) * (-t / o.decay).exp()
    })
}

fn tom_spec(freq: f64, seed: u32) -> KickSpec {
    KickSpec {
        len: 0.6,
        f0: freq * 1.8,
        f1: freq,
        pitch_decay: 0.05,
        amp_decay: 0.18,
        click: 0.2,
        drive: 1.2,
        seed,
    }
}

fn bass808_render(note: f64, len: f64, sr: f64) -> Vec<Vec<f32>> {
    let f1 = 440.0 * 2f64.powf((note - 69.0) / 12.0);
    let mut phase = 0.0;
    mono(len, sr, |t| {
        let f = f1 * (1.0 + 1.5 * (-t / 0.03).exp());
        phase += f / sr;
        let env = (t / 0.002).min(1.0) * (-t / (len * 0.45)).exp();
        ((TAU * phase).sin() * env * 1.6).tanh()
    })
}

fn riser_render(len: f64, up: bool, seed: u32, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(seed);
    let n = (len * sr).floor() as usize;
    let mut l = vec![0f32; n];
    let mut rr = vec![0f32; n];
    let (mut y1, mut y2, mut z1, mut z2) = (0f64, 0f64, 0f64, 0f64);
    for i in 0..n {
        let t = i as f64 / n as f64;
        let p = if up { t } else { 1.0 - t };
        let fc = 200.0 * 60f64.powf(p);
        let g = (PI * fc.min(sr * 0.45) / sr).tan();
        let k = 0.6;
        let a1 = 1.0 / (1.0 + g * (g + k));
        // two state variable band passes (left and right decorrelated)
        let xl = r.next();
        let xr = r.next();
        let v3l = xl - z2;
        let v1l = a1 * z1 + g * a1 * v3l;
        let v2l = z2 + g * a1 * z1 + g * g * a1 * v3l;
        z1 = 2.0 * v1l - z1;
        z2 = 2.0 * v2l - z2;
        let v3r = xr - y2;
        let v1r = a1 * y1 + g * a1 * v3r;
        let v2r = y2 + g * a1 * y1 + g * g * a1 * v3r;
        y1 = 2.0 * v1r - y1;
        y2 = 2.0 * v2r - y2;
        let env = if up { t.powf(1.5) } else { (1.0 - t).powf(1.5) };
        l[i] = (v1l * env) as f32;
        rr[i] = (v1r * env) as f32;
    }
    let mut chs = vec![l, rr];
    normalize(&mut chs, 0.8);
    chs
}

fn impact_render(seed: u32, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(seed);
    let mut lp = Biquad::new(Band::Lp, 600.0, 0.7, sr);
    let mut ph = 0.0;
    mono(3.0, sr, |t| {
        ph += (40.0 + 80.0 * (-t / 0.08).exp()) / sr;
        ((TAU * ph).sin() * (-t / 0.8).exp() * 1.5 + lp.run(r.next()) * (-t / 0.6).exp() * 0.8)
            .tanh()
    })
}

fn cowbell_render(sr: f64) -> Vec<Vec<f32>> {
    let mut bp = Biquad::new(Band::Bp, 800.0, 3.0, sr);
    let (mut p1, mut p2) = (0.0, 0.0);
    mono(0.4, sr, |t| {
        p1 += 562.0 / sr;
        p2 += 845.0 / sr;
        let x =
            (if p1 % 1.0 < 0.5 { 1.0 } else { -1.0 }) + (if p2 % 1.0 < 0.5 { 1.0 } else { -1.0 });
        bp.run(x) * (-t / 0.09).exp()
    })
}

fn shaker_render(seed: u32, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(seed);
    let mut hp = Biquad::new(Band::Hp, 5000.0, 0.7, sr);
    mono(0.18, sr, |t| {
        hp.run(r.next()) * (t / 0.03).min(1.0) * (-t / 0.05).exp()
    })
}

fn rim_render(seed: u32, sr: f64) -> Vec<Vec<f32>> {
    let mut r = Rng::new(seed);
    let mut bp = Biquad::new(Band::Bp, 1700.0, 4.0, sr);
    mono(0.12, sr, |t| {
        (bp.run(r.next()) * 3.0 + (TAU * 1700.0 * t).sin() * 0.6) * (-t / 0.018).exp()
    })
}

fn crash_render(seed: u32, len: f64, sr: f64) -> Vec<Vec<f32>> {
    let l = hat_channel(
        HatSpec {
            len,
            decay: len * 0.35,
            hp: 4200.0,
            seed,
            metal: 0.4,
        },
        sr,
    )
    .remove(0);
    let r = hat_channel(
        HatSpec {
            len,
            decay: len * 0.35,
            hp: 4000.0,
            seed: seed + 11,
            metal: 0.4,
        },
        sr,
    )
    .remove(0);
    let mut chs = vec![l, r];
    normalize(&mut chs, 0.8);
    chs
}

fn chord_stab_render(notes: &[f64], sr: f64) -> Vec<Vec<f32>> {
    let n = (sr * 0.9).floor() as usize;
    let mut l = vec![0f64; n];
    let mut r = vec![0f64; n];
    // the original sums into Float32Arrays
    let mut lf = vec![0f32; n];
    let mut rf = vec![0f32; n];
    let mut lp = Biquad::new(Band::Lp, 2400.0, 1.5, sr);
    let mut lp2 = Biquad::new(Band::Lp, 2400.0, 1.5, sr);
    for (k, note) in notes.iter().enumerate() {
        let f = 440.0 * 2f64.powf((note - 69.0) / 12.0);
        for d in -1i32..=1 {
            let fr = f * 2f64.powf((f64::from(d) * 9.0) / 1200.0);
            let mut ph = (k as f64 * 0.31 + f64::from(d) * 0.17) % 1.0;
            for i in 0..n {
                ph += fr / sr;
                let s = 2.0 * (ph % 1.0) - 1.0;
                if d <= 0 {
                    lf[i] = (f64::from(lf[i]) + s) as f32;
                }
                if d >= 0 {
                    rf[i] = (f64::from(rf[i]) + s) as f32;
                }
            }
        }
    }
    for i in 0..n {
        let env = (i as f64 / (sr * 0.004)).min(1.0) * (-(i as f64) / (sr * 0.22)).exp();
        l[i] = lp.run(f64::from(lf[i])) * env;
        r[i] = lp2.run(f64::from(rf[i])) * env;
    }
    let mut chs = vec![
        l.iter().map(|&v| v as f32).collect::<Vec<_>>(),
        r.iter().map(|&v| v as f32).collect::<Vec<_>>(),
    ];
    normalize(&mut chs, 0.8);
    chs
}

macro_rules! kick {
    ($len:expr, $f0:expr, $f1:expr, $pd:expr, $ad:expr, $click:expr, $drive:expr) => {
        kick!($len, $f0, $f1, $pd, $ad, $click, $drive, 7)
    };
    ($len:expr, $f0:expr, $f1:expr, $pd:expr, $ad:expr, $click:expr, $drive:expr, $seed:expr) => {
        |sr| {
            kick_render(
                KickSpec {
                    len: $len,
                    f0: $f0,
                    f1: $f1,
                    pitch_decay: $pd,
                    amp_decay: $ad,
                    click: $click,
                    drive: $drive,
                    seed: $seed,
                },
                sr,
            )
        }
    };
}

macro_rules! snare {
    ($tone:expr, $td:expr, $nd:expr, $nf:expr, $mix:expr, $len:expr, $seed:expr) => {
        |sr| {
            snare_render(
                SnareSpec {
                    tone: $tone,
                    tone_decay: $td,
                    noise_decay: $nd,
                    noise_freq: $nf,
                    mix: $mix,
                    len: $len,
                    seed: $seed,
                },
                sr,
            )
        }
    };
}

macro_rules! clap {
    ($len:expr, $decay:expr, $freq:expr, $seed:expr, $bursts:expr) => {
        |sr| {
            clap_render(
                ClapSpec {
                    len: $len,
                    decay: $decay,
                    freq: $freq,
                    seed: $seed,
                    bursts: $bursts,
                },
                sr,
            )
        }
    };
}

macro_rules! hat {
    ($len:expr, $decay:expr, $hp:expr, $seed:expr, $metal:expr) => {
        |sr| {
            hat_channel(
                HatSpec {
                    len: $len,
                    decay: $decay,
                    hp: $hp,
                    seed: $seed,
                    metal: $metal,
                },
                sr,
            )
        }
    };
}

macro_rules! sample {
    ($id:expr, $name:expr, $folder:expr, $render:expr) => {
        FactorySample {
            id: $id,
            name: $name,
            folder: $folder,
            render: $render,
        }
    };
}

/// Every factory sample, in the order of the web version.
pub static FACTORY_SAMPLES: &[FactorySample] = &[
    sample!(
        "factory:kick-punch",
        "Kick Punch",
        "Kicks",
        kick!(0.5, 260.0, 52.0, 0.025, 0.16, 0.6, 1.8)
    ),
    sample!(
        "factory:kick-deep",
        "Kick Deep",
        "Kicks",
        kick!(0.9, 180.0, 42.0, 0.04, 0.3, 0.3, 1.3)
    ),
    sample!(
        "factory:kick-hard",
        "Kick Hard",
        "Kicks",
        kick!(0.45, 380.0, 58.0, 0.018, 0.12, 1.0, 3.5)
    ),
    sample!(
        "factory:kick-house",
        "Kick House",
        "Kicks",
        kick!(0.6, 220.0, 48.0, 0.03, 0.2, 0.45, 2.2, 9)
    ),
    sample!(
        "factory:snare-tight",
        "Snare Tight",
        "Snares",
        snare!(190.0, 0.05, 0.12, 4000.0, 0.6, 0.35, 3)
    ),
    sample!(
        "factory:snare-fat",
        "Snare Fat",
        "Snares",
        snare!(160.0, 0.09, 0.22, 2500.0, 0.55, 0.5, 4)
    ),
    sample!(
        "factory:snare-trap",
        "Snare Trap",
        "Snares",
        snare!(220.0, 0.04, 0.17, 6000.0, 0.75, 0.4, 5)
    ),
    sample!(
        "factory:clap-classic",
        "Clap Classic",
        "Claps",
        clap!(0.5, 0.12, 1200.0, 21, 4)
    ),
    sample!(
        "factory:clap-wide",
        "Clap Wide",
        "Claps",
        clap!(0.7, 0.2, 1600.0, 22, 5)
    ),
    sample!(
        "factory:snap",
        "Finger Snap",
        "Claps",
        clap!(0.25, 0.04, 2600.0, 23, 2)
    ),
    sample!(
        "factory:hat-closed",
        "Hat Closed",
        "Hats",
        hat!(0.12, 0.03, 7000.0, 31, 0.5)
    ),
    sample!(
        "factory:hat-closed-2",
        "Hat Tick",
        "Hats",
        hat!(0.08, 0.015, 9000.0, 32, 0.3)
    ),
    sample!(
        "factory:hat-open",
        "Hat Open",
        "Hats",
        hat!(0.6, 0.2, 6500.0, 33, 0.55)
    ),
    sample!(
        "factory:ride",
        "Ride",
        "Cymbals",
        hat!(1.5, 0.6, 3500.0, 34, 0.8)
    ),
    sample!("factory:crash", "Crash", "Cymbals", |sr| crash_render(
        35, 2.4, sr
    )),
    sample!("factory:rim", "Rimshot", "Percussion", |sr| rim_render(
        41, sr
    )),
    sample!("factory:cowbell", "Cowbell", "Percussion", cowbell_render),
    sample!(
        "factory:shaker",
        "Shaker",
        "Percussion",
        |sr| shaker_render(43, sr)
    ),
    sample!(
        "factory:tom-low",
        "Tom Low",
        "Percussion",
        |sr| kick_render(tom_spec(90.0, 44), sr)
    ),
    sample!(
        "factory:tom-mid",
        "Tom Mid",
        "Percussion",
        |sr| kick_render(tom_spec(140.0, 45), sr)
    ),
    sample!("factory:tom-high", "Tom High", "Percussion", |sr| {
        kick_render(tom_spec(210.0, 46), sr)
    }),
    sample!("factory:808-c", "808 Bass C", "808s", |sr| bass808_render(
        36.0, 1.6, sr
    )),
    sample!("factory:808-f", "808 Bass F", "808s", |sr| bass808_render(
        41.0, 1.6, sr
    )),
    sample!("factory:808-long", "808 Long", "808s", |sr| bass808_render(
        36.0, 3.0, sr
    )),
    sample!("factory:riser", "Noise Riser", "FX", |sr| riser_render(
        4.0, true, 51, sr
    )),
    sample!("factory:downlifter", "Downlifter", "FX", |sr| riser_render(
        3.0, false, 52, sr
    )),
    sample!("factory:impact", "Impact", "FX", |sr| impact_render(53, sr)),
    sample!("factory:stab-minor", "Stab Cm", "Stabs", |sr| {
        chord_stab_render(&[60.0, 63.0, 67.0, 72.0], sr)
    }),
    sample!("factory:stab-major", "Stab F", "Stabs", |sr| {
        chord_stab_render(&[65.0, 69.0, 72.0, 77.0], sr)
    }),
];
