// SPDX-License-Identifier: GPL-3.0-or-later
//! The samples a project plays: the factory kit (made on demand) and the audio
//! stored in the project file, decoded and brought to the render's sample rate.

use crate::factory::{is_factory_id, render_factory, Audio};
use las_project::Sample;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub struct Library {
    raw: BTreeMap<String, Sample>,
    cache: HashMap<String, Option<Arc<Audio>>>,
    processed: HashMap<(String, bool, String), Arc<Audio>>,
    sample_rate: f64,
}

impl Library {
    pub fn new(sample_rate: f64, samples: &BTreeMap<String, Sample>) -> Self {
        Self {
            raw: samples.clone(),
            cache: HashMap::new(),
            processed: HashMap::new(),
            sample_rate,
        }
    }

    /// A library with only the factory kit.
    pub fn factory_only(sample_rate: f64) -> Self {
        Self::new(sample_rate, &BTreeMap::new())
    }

    /// The decoded audio of a sample at the render's sample rate, or `None` if
    /// there is no such sample or it cannot be decoded.
    pub fn get(&mut self, id: &str) -> Option<Arc<Audio>> {
        if let Some(hit) = self.cache.get(id) {
            return hit.clone();
        }
        let audio = if is_factory_id(id) {
            render_factory(id, self.sample_rate)
        } else {
            self.raw
                .get(id)
                .and_then(|s| decode_wav(&s.data))
                .map(|a| resample(a, self.sample_rate))
        };
        let audio = audio.map(Arc::new);
        self.cache.insert(id.to_string(), audio.clone());
        audio
    }
}

impl Library {
    /// A clip's source reversed and/or time stretched (`factor` 1 is no stretch).
    pub fn processed(&mut self, id: &str, reverse: bool, factor: f64) -> Option<Arc<Audio>> {
        let src = self.get(id)?;
        if factor == 1.0 && !reverse {
            return Some(src);
        }
        let key = (id.to_string(), reverse, format!("{factor:.6}"));
        if let Some(hit) = self.processed.get(&key) {
            return Some(hit.clone());
        }
        let base: Vec<Vec<f32>> = if reverse {
            src.channels
                .iter()
                .map(|c| c.iter().rev().copied().collect())
                .collect()
        } else {
            src.channels.clone()
        };
        let channels = if factor == 1.0 {
            base
        } else {
            las_dsp::stretch::time_stretch(&base, src.sample_rate, factor)
        };
        let out = Arc::new(Audio {
            channels,
            sample_rate: src.sample_rate,
        });
        self.processed.insert(key, out.clone());
        Some(out)
    }
}

/// Decode a WAV file (8 to 32 bit integer or 32 bit float).
pub fn decode_wav(bytes: &[u8]) -> Option<Audio> {
    let mut r = hound::WavReader::new(std::io::Cursor::new(bytes)).ok()?;
    let spec = r.spec();
    let n = usize::from(spec.channels);
    if n == 0 {
        return None;
    }
    let all: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().ok()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .ok()?
        }
    };
    let frames = all.len() / n;
    let channels = (0..n)
        .map(|c| (0..frames).map(|i| all[i * n + c]).collect())
        .collect();
    Some(Audio {
        channels,
        sample_rate: f64::from(spec.sample_rate),
    })
}

/// Bring audio to `rate` with a windowed sinc (as `decodeAudioData` does for
/// files at another rate; the browser's own filter is not specified).
pub fn resample(a: Audio, rate: f64) -> Audio {
    if (a.sample_rate - rate).abs() < 1e-9 {
        return a;
    }
    let ratio = rate / a.sample_rate;
    let frames = (a.frames() as f64 * ratio).round() as usize;
    let cutoff = ratio.min(1.0);
    let half = 16.0 / cutoff;
    let channels = a
        .channels
        .iter()
        .map(|src| {
            (0..frames)
                .map(|i| {
                    let t = i as f64 / ratio;
                    let lo = ((t - half).ceil().max(0.0)) as usize;
                    let hi = ((t + half).floor() as usize).min(src.len().saturating_sub(1));
                    let mut acc = 0.0;
                    for (k, &s) in src.iter().enumerate().take(hi + 1).skip(lo) {
                        let x = k as f64 - t;
                        let w = 0.5 + 0.5 * (std::f64::consts::PI * x / half).cos();
                        // low pass at the lower of the two Nyquist frequencies
                        let sinc = if x == 0.0 {
                            cutoff
                        } else {
                            (std::f64::consts::PI * x * cutoff).sin() / (std::f64::consts::PI * x)
                        };
                        acc += f64::from(s) * sinc * w;
                    }
                    acc as f32
                })
                .collect()
        })
        .collect();
    Audio {
        channels,
        sample_rate: rate,
    }
}
