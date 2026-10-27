// SPDX-License-Identifier: GPL-3.0-or-later OR LicenseRef-Powelus-Proprietary
//! A `DynamicsCompressorNode` as Chromium makes it (the WebKit compressor): a
//! 6 ms look-ahead, a detector that follows a shaped copy of the input, an
//! adaptive release and automatic make-up gain. The web version's maximizer is
//! built from three of these, so it is ported rather than substituted.
//!
//! Everything runs in `f32`, as in the browser.

// the make-up and release curve constants are copied at the precision they were
// derived with
#![allow(clippy::excessive_precision)]

const PRE_DELAY_MASK: usize = 1023;
const MAX_PRE_DELAY: usize = 1024;
const DEFAULT_PRE_DELAY: usize = 256;
const DIVISION: usize = 32;
const PI: f32 = std::f32::consts::PI;
const METERING_RELEASE_TC: f32 = 0.325;
const SAT_RELEASE_TIME: f32 = 0.0025;
const RELEASE_ZONES: [f32; 4] = [0.09, 0.16, 0.42, 0.98];
const SPACING_DB: f32 = 5.0;

fn to_db(x: f32) -> f32 {
    if x == 0.0 {
        -1000.0
    } else {
        20.0 * x.log10()
    }
}

fn from_db(db: f32) -> f32 {
    10f32.powf(0.05 * db)
}

#[derive(Clone, Copy, Debug)]
pub struct CompParams {
    /// dB.
    pub threshold: f32,
    /// dB.
    pub knee: f32,
    pub ratio: f32,
    /// Seconds.
    pub attack: f32,
    /// Seconds.
    pub release: f32,
}

impl Default for CompParams {
    fn default() -> Self {
        Self {
            threshold: -24.0,
            knee: 30.0,
            ratio: 12.0,
            attack: 0.003,
            release: 0.25,
        }
    }
}

pub struct WebCompressor {
    sr: f32,
    pre_delay: [Vec<f32>; 2],
    last_pre_delay_frames: usize,
    read: usize,
    write: usize,
    detector_average: f32,
    compressor_gain: f32,
    metering_gain: f32,
    metering_release_k: f32,
    max_attack_diff_db: f32,
    // static curve
    ratio: f32,
    slope: f32,
    linear_threshold: f32,
    db_threshold: f32,
    db_knee: f32,
    knee_threshold: f32,
    knee_threshold_db: f32,
    yknee_threshold_db: f32,
    k: f32,
    /// Frames into the current division of 32.
    division_pos: usize,
    envelope_rate: f32,
    scaled_desired_gain: f32,
    pub p: CompParams,
}

impl WebCompressor {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sample_rate as f32;
        Self {
            sr,
            pre_delay: [vec![0.0; MAX_PRE_DELAY], vec![0.0; MAX_PRE_DELAY]],
            last_pre_delay_frames: 0,
            read: 0,
            write: DEFAULT_PRE_DELAY,
            detector_average: 0.0,
            compressor_gain: 1.0,
            metering_gain: 1.0,
            metering_release_k: 1.0 - (-1.0 / (sr * METERING_RELEASE_TC)).exp(),
            max_attack_diff_db: -1.0,
            ratio: -1.0,
            slope: -1.0,
            linear_threshold: -1.0,
            db_threshold: -1.0,
            db_knee: -1.0,
            knee_threshold: -1.0,
            knee_threshold_db: -1.0,
            yknee_threshold_db: -1.0,
            k: -1.0,
            division_pos: 0,
            envelope_rate: 1.0,
            scaled_desired_gain: 1.0,
            p: CompParams::default(),
        }
    }

    /// Look-ahead delay, seconds (6 ms, in whole samples).
    pub fn latency(&self) -> f64 {
        f64::from(((0.006f32 * self.sr) as usize).min(MAX_PRE_DELAY - 1) as f32)
            / f64::from(self.sr)
    }

    fn knee_curve(&self, x: f32, k: f32) -> f32 {
        if x < self.linear_threshold {
            return x;
        }
        self.linear_threshold + (1.0 - (-k * (x - self.linear_threshold)).exp()) / k
    }

    fn saturate(&self, x: f32, k: f32) -> f32 {
        if x < self.knee_threshold {
            self.knee_curve(x, k)
        } else {
            // constant ratio after the knee
            let x_db = to_db(x);
            let y_db = self.yknee_threshold_db + self.slope * (x_db - self.knee_threshold_db);
            from_db(y_db)
        }
    }

    fn slope_at(&self, x: f32, k: f32) -> f32 {
        if x < self.linear_threshold {
            return 1.0;
        }
        let x2 = x * 1.001;
        let x_db = to_db(x);
        let x2_db = to_db(x2);
        let y_db = to_db(self.knee_curve(x, k));
        let y2_db = to_db(self.knee_curve(x2, k));
        (y2_db - y_db) / (x2_db - x_db)
    }

    fn k_at_slope(&self, desired: f32) -> f32 {
        let x_db = self.db_threshold + self.db_knee;
        let x = from_db(x_db);
        let mut min_k = 0.1f32;
        let mut max_k = 10000.0f32;
        let mut k = 5.0f32;
        for _ in 0..15 {
            let slope = self.slope_at(x, k);
            if slope < desired {
                // k is too high
                max_k = k;
            } else {
                min_k = k;
            }
            k = (min_k * max_k).sqrt();
        }
        k
    }

    fn update_static_curve(&mut self) -> f32 {
        let CompParams {
            threshold,
            knee,
            ratio,
            ..
        } = self.p;
        if threshold != self.db_threshold || knee != self.db_knee || ratio != self.ratio {
            self.db_threshold = threshold;
            self.linear_threshold = from_db(threshold);
            self.db_knee = knee;
            self.ratio = ratio;
            self.slope = 1.0 / ratio;
            let k = self.k_at_slope(1.0 / ratio);
            self.knee_threshold_db = threshold + knee;
            self.knee_threshold = from_db(self.knee_threshold_db);
            self.yknee_threshold_db = to_db(self.knee_curve(self.knee_threshold, k));
            self.k = k;
        }
        self.k
    }

    /// Process a block of any length; `out_*` must be as long as `in_*`.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let sr = self.sr;
        let k = self.update_static_curve();
        // make-up gain
        let full_range_gain = self.saturate(1.0, k);
        let full_range_makeup = (1.0 / full_range_gain).powf(0.6);
        let master_linear_gain = from_db(0.0) * full_range_makeup;
        let attack_time = self.p.attack.max(0.001);
        let attack_frames = attack_time * sr;
        let release_frames = sr * self.p.release;
        let sat_release_frames = SAT_RELEASE_TIME * sr;
        let y1 = release_frames * RELEASE_ZONES[0];
        let y2 = release_frames * RELEASE_ZONES[1];
        let y3 = release_frames * RELEASE_ZONES[2];
        let y4 = release_frames * RELEASE_ZONES[3];
        // 4th order fit through (0, y1), (1, y2), (2, y3), (3, y4)
        let ka = 0.999_999_999_999_999_8_f32 * y1 + 1.843_221_968_432_392_3e-16_f32 * y2
            - 1.937_339_435_167_642_3e-16_f32 * y3
            + 8.824_516_011_816_245e-18_f32 * y4;
        let kb = -1.578_832_035_284_588_8_f32 * y1 + 2.330_583_703_207_428_6_f32 * y2
            - 0.914_119_420_484_042_9_f32 * y3
            + 0.162_367_752_561_203_2_f32 * y4;
        let kc = 0.533_414_286_910_642_4_f32 * y1 - 1.272_736_789_213_631_f32 * y2
            + 0.925_885_604_220_751_2_f32 * y3
            - 0.186_563_101_917_762_26_f32 * y4;
        let kd = 0.087_834_631_382_072_34_f32 * y1 - 0.169_416_296_792_562_2_f32 * y2
            + 0.085_880_579_515_952_72_f32 * y3
            - 0.004_298_914_105_462_83_f32 * y4;
        let ke = -0.042_416_883_008_123_074_f32 * y1 + 0.111_569_382_798_760_2_f32 * y2
            - 0.097_646_763_252_658_72_f32 * y3
            + 0.028_494_263_462_021_576_f32 * y4;

        // pre-delay time (the web version leaves it at 6 ms)
        let pre_delay_frames = ((0.006f32 * sr) as usize).min(MAX_PRE_DELAY - 1);
        if self.last_pre_delay_frames != pre_delay_frames {
            self.last_pre_delay_frames = pre_delay_frames;
            for b in &mut self.pre_delay {
                b.fill(0.0);
            }
            self.read = 0;
            self.write = pre_delay_frames;
        }

        for i in 0..in_l.len() {
            if self.division_pos == 0 {
                // choose the envelope rate for the next 32 frames
                if self.detector_average.is_nan() || self.detector_average.is_infinite() {
                    self.detector_average = 1.0;
                }
                let desired_gain = self.detector_average;
                // pre-warp so we get desired_gain after the sin() warp below
                self.scaled_desired_gain = desired_gain.asin() / (0.5 * PI);
                let is_releasing = self.scaled_desired_gain > self.compressor_gain;
                let mut compression_diff_db =
                    to_db(self.compressor_gain / self.scaled_desired_gain);
                if is_releasing {
                    self.max_attack_diff_db = -1.0;
                    if compression_diff_db.is_nan() || compression_diff_db.is_infinite() {
                        compression_diff_db = -1.0;
                    }
                    // higher compression (lower diff) releases faster
                    let mut x = compression_diff_db;
                    x = x.max(-12.0);
                    x = x.min(0.0);
                    x = 0.25 * (x + 12.0);
                    let x2 = x * x;
                    let x3 = x2 * x;
                    let x4 = x2 * x2;
                    let rf = ka + kb * x + kc * x2 + kd * x3 + ke * x4;
                    let db_per_frame = SPACING_DB / rf;
                    self.envelope_rate = from_db(db_per_frame);
                } else {
                    if compression_diff_db.is_nan() || compression_diff_db.is_infinite() {
                        compression_diff_db = 1.0;
                    }
                    if self.max_attack_diff_db == -1.0
                        || self.max_attack_diff_db < compression_diff_db
                    {
                        self.max_attack_diff_db = compression_diff_db;
                    }
                    let eff_atten_diff_db = self.max_attack_diff_db.max(0.5);
                    let x = 0.25 / eff_atten_diff_db;
                    self.envelope_rate = 1.0 - x.powf(1.0 / attack_frames);
                }
            }
            // look-ahead: the gain is computed from the undelayed input
            let (a, b) = (in_l[i], in_r[i]);
            self.pre_delay[0][self.write] = a;
            self.pre_delay[1][self.write] = b;
            let mut compressor_input = 0.0f32;
            for v in [a, b] {
                let abs = if v > 0.0 { v } else { -v };
                if compressor_input < abs {
                    compressor_input = abs;
                }
            }
            let abs_input = compressor_input;
            let shaped_input = self.saturate(abs_input, k);
            let attenuation = if abs_input <= 0.0001 {
                1.0
            } else {
                shaped_input / abs_input
            };
            let mut attenuation_db = -to_db(attenuation);
            attenuation_db = attenuation_db.max(2.0);
            let db_per_frame = attenuation_db / sat_release_frames;
            let sat_release_rate = from_db(db_per_frame) - 1.0;
            let is_release = attenuation > self.detector_average;
            let rate = if is_release { sat_release_rate } else { 1.0 };
            self.detector_average += (attenuation - self.detector_average) * rate;
            self.detector_average = self.detector_average.min(1.0);
            if self.detector_average.is_nan() || self.detector_average.is_infinite() {
                self.detector_average = 1.0;
            }
            // exponential approach to the desired gain
            if self.envelope_rate < 1.0 {
                self.compressor_gain +=
                    (self.scaled_desired_gain - self.compressor_gain) * self.envelope_rate;
            } else {
                self.compressor_gain *= self.envelope_rate;
                self.compressor_gain = self.compressor_gain.min(1.0);
            }
            // warp to smooth out sharp exponential transition points
            let post_warp = (0.5 * PI * self.compressor_gain).sin();
            let total_gain = master_linear_gain * post_warp;
            let db_real_gain = 20.0 * post_warp.log10();
            if db_real_gain < self.metering_gain {
                self.metering_gain = db_real_gain;
            } else {
                self.metering_gain += (db_real_gain - self.metering_gain) * self.metering_release_k;
            }
            out_l[i] = self.pre_delay[0][self.read] * total_gain;
            out_r[i] = self.pre_delay[1][self.read] * total_gain;
            self.read = (self.read + 1) & PRE_DELAY_MASK;
            self.write = (self.write + 1) & PRE_DELAY_MASK;
            self.division_pos = (self.division_pos + 1) % DIVISION;
        }
    }
}
