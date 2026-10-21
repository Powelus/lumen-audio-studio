// SPDX-License-Identifier: GPL-3.0-or-later
//! The samples a project plays: the factory kit (made on demand) and the audio
//! stored in the project file, decoded and brought to the render's sample rate.

use crate::factory::{is_factory_id, render_factory, Audio};
use las_project::Sample;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// A key for audio that is the same whatever its id: a hash of the file's bytes and
/// the rate it is brought to.
type AudioKey = (u64, u64);

/// The audio every library has decoded, kept for the life of the program so playing,
/// stopping, editing and rendering never decode a file twice. Shared, read only.
struct Shared {
    decoded: HashMap<AudioKey, Arc<Audio>>,
    processed: HashMap<(AudioKey, bool, u64), Arc<Audio>>,
    factory: HashMap<(String, u64), Arc<Audio>>,
}

fn shared() -> std::sync::MutexGuard<'static, Shared> {
    static SHARED: std::sync::OnceLock<std::sync::Mutex<Shared>> = std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| {
            std::sync::Mutex::new(Shared {
                decoded: HashMap::new(),
                processed: HashMap::new(),
                factory: HashMap::new(),
            })
        })
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Audio past this many bytes in the shared cache is let go once nothing plays it.
const CACHE_BYTES: usize = 1 << 30;

impl Shared {
    fn trim(&mut self) {
        let size = |a: &Arc<Audio>| a.channels.iter().map(Vec::len).sum::<usize>() * 4;
        let total: usize = self
            .decoded
            .values()
            .chain(self.processed.values())
            .map(size)
            .sum();
        if total > CACHE_BYTES {
            self.decoded.retain(|_, a| Arc::strong_count(a) > 1);
            self.processed.retain(|_, a| Arc::strong_count(a) > 1);
        }
    }
}

/// A hash of a file's bytes, which names its audio in the shared cache.
pub fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    bytes.hash(&mut h);
    h.finish()
}

pub struct Library {
    /// Each sample's key in the shared cache.
    keys: HashMap<String, AudioKey>,
    /// The bytes of the samples not decoded yet (the rest are not copied).
    raw: HashMap<String, Vec<u8>>,
    cache: HashMap<String, Option<Arc<Audio>>>,
    sample_rate: f64,
}

impl Library {
    pub fn new(sample_rate: f64, samples: &BTreeMap<String, Sample>) -> Self {
        let rate = sample_rate.to_bits();
        let mut keys = HashMap::new();
        let mut raw = HashMap::new();
        let have = shared();
        for (id, s) in samples {
            let key = (content_hash(&s.data), rate);
            if !have.decoded.contains_key(&key) {
                raw.insert(id.clone(), s.data.clone());
            }
            keys.insert(id.clone(), key);
        }
        drop(have);
        Self {
            keys,
            raw,
            cache: HashMap::new(),
            sample_rate,
        }
    }

    /// A library with only the factory kit.
    pub fn factory_only(sample_rate: f64) -> Self {
        Self::new(sample_rate, &BTreeMap::new())
    }

    /// The decoded audio of a sample at the render's sample rate, or `None` if
    /// there is no such sample or it cannot be decoded. Each file is decoded once in
    /// the life of the program.
    pub fn get(&mut self, id: &str) -> Option<Arc<Audio>> {
        if let Some(hit) = self.cache.get(id) {
            return hit.clone();
        }
        let rate = self.sample_rate.to_bits();
        let audio = if is_factory_id(id) {
            let key = (id.to_string(), rate);
            let hit = shared().factory.get(&key).cloned();
            hit.or_else(|| {
                let a = Arc::new(render_factory(id, self.sample_rate)?);
                shared().factory.insert(key, a.clone());
                Some(a)
            })
        } else {
            let key = *self.keys.get(id)?;
            let hit = shared().decoded.get(&key).cloned();
            hit.or_else(|| {
                let bytes = self.raw.remove(id)?;
                DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let a = Arc::new(resample(decode_wav(&bytes)?, self.sample_rate));
                let mut s = shared();
                s.decoded.insert(key, a.clone());
                s.trim();
                Some(a)
            })
        };
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
        let base_key = self
            .keys
            .get(id)
            .copied()
            .unwrap_or_else(|| (content_hash(id.as_bytes()), self.sample_rate.to_bits()));
        let key = (base_key, reverse, ((factor * 1e6).round() as i64) as u64);
        if let Some(hit) = shared().processed.get(&key) {
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
        let mut s = shared();
        s.processed.insert(key, out.clone());
        s.trim();
        Some(out)
    }
}

static DECODES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many times a project sample has been decoded in this process (for tests and
/// the benchmark: playing again must not decode again).
pub fn decode_count() -> usize {
    DECODES.load(std::sync::atomic::Ordering::Relaxed)
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

/// The audio file extensions [`decode_audio`] reads.
pub const AUDIO_EXTENSIONS: [&str; 9] = [
    "wav", "mp3", "flac", "ogg", "oga", "aif", "aiff", "m4a", "aac",
];

/// Whether a file name looks like audio this program reads.
pub fn is_audio_file(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Decode an audio file of any format [`AUDIO_EXTENSIONS`] names, WAV quickest.
pub fn decode_audio(bytes: &[u8]) -> Option<Audio> {
    if let Some(a) = decode_wav(bytes) {
        return Some(a);
    }
    if bytes.starts_with(b"fLaC") {
        return decode_flac(bytes);
    }
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    let source = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut rate = track.codec_params.sample_rate.unwrap_or(44_100);
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            // a damaged packet is skipped, as players do
            continue;
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        let n = spec.channels.count().max(1);
        if channels.is_empty() {
            channels = vec![Vec::new(); n];
        }
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        for frame in buf.samples().chunks(n) {
            for (c, v) in frame.iter().enumerate().take(channels.len()) {
                channels[c].push(*v);
            }
        }
    }
    if channels.first().is_none_or(Vec::is_empty) {
        return None;
    }
    Some(Audio {
        channels,
        sample_rate: f64::from(rate),
    })
}

/// Decode a FLAC file.
fn decode_flac(bytes: &[u8]) -> Option<Audio> {
    let mut r = claxon::FlacReader::new(bytes).ok()?;
    let info = r.streaminfo();
    let n = info.channels as usize;
    let scale = 1.0 / (1u64 << (info.bits_per_sample - 1)) as f32;
    let all: Vec<i32> = r.samples().collect::<Result<_, _>>().ok()?;
    let frames = all.len() / n.max(1);
    let channels = (0..n)
        .map(|c| (0..frames).map(|i| all[i * n + c] as f32 * scale).collect())
        .collect();
    Some(Audio {
        channels,
        sample_rate: f64::from(info.sample_rate),
    })
}

/// A 32 bit float WAV file of some audio, which is how imported audio is kept.
pub fn encode_wav(a: &Audio) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: a.channels.len().max(1) as u16,
        sample_rate: a.sample_rate.round() as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    if let Ok(mut w) = hound::WavWriter::new(&mut out, spec) {
        for i in 0..a.frames() {
            for c in &a.channels {
                let _ = w.write_sample(c[i]);
            }
        }
        let _ = w.finalize();
    }
    out.into_inner()
}

/// Read an audio file for a project: the audio, and the bytes to keep (the file
/// itself when it is a WAV, otherwise the audio as a WAV, so it plays the same
/// everywhere the project is opened).
pub fn import_audio(bytes: Vec<u8>) -> Result<(Audio, Vec<u8>), String> {
    if let Some(a) = decode_wav(&bytes) {
        return Ok((a, bytes));
    }
    let a = decode_audio(&bytes)
        .ok_or("this file could not be read as audio (WAV, MP3, FLAC, OGG, AIFF and M4A work)")?;
    let wav = encode_wav(&a);
    Ok((a, wav))
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
    let pi = std::f64::consts::PI;
    // the window and the sinc are sines of x, which steps by one from tap to tap: they
    // are turned on by a rotation rather than computed again for every tap
    let (dc, ds) = ((pi / half).cos(), (pi / half).sin());
    let (ec, es) = ((pi * cutoff).cos(), (pi * cutoff).sin());
    let one = |src: &[f32]| -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f64 / ratio;
                let lo = ((t - half).ceil().max(0.0)) as usize;
                let hi = ((t + half).floor() as usize).min(src.len().saturating_sub(1));
                let x0 = lo as f64 - t;
                let (mut wc, mut ws) = ((pi * x0 / half).cos(), (pi * x0 / half).sin());
                let (mut sc, mut ss) = ((pi * x0 * cutoff).cos(), (pi * x0 * cutoff).sin());
                let mut acc = 0.0;
                for (k, &s) in src.iter().enumerate().take(hi + 1).skip(lo) {
                    let x = k as f64 - t;
                    let w = 0.5 + 0.5 * wc;
                    // low pass at the lower of the two Nyquist frequencies
                    let sinc = if x == 0.0 { cutoff } else { ss / (pi * x) };
                    acc += f64::from(s) * sinc * w;
                    (wc, ws) = (wc * dc - ws * ds, ws * dc + wc * ds);
                    (sc, ss) = (sc * ec - ss * es, ss * ec + sc * es);
                }
                acc as f32
            })
            .collect()
    };
    // the channels side by side
    let channels: Vec<Vec<f32>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = a
            .channels
            .iter()
            .map(|src| scope.spawn(|| one(src)))
            .collect();
        jobs.into_iter()
            .map(|j| j.join().unwrap_or_default())
            .collect()
    });
    Audio {
        channels,
        sample_rate: rate,
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    fn tone() -> Vec<f32> {
        (0..44_100)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect()
    }

    #[test]
    fn mp3_and_flac_files_are_read_and_kept_as_wav() {
        let x = tone();
        for (kind, bytes) in [
            (
                "mp3",
                crate::export::encode_mp3(&[&x, &x], 44_100, 192).unwrap(),
            ),
            (
                "flac",
                crate::export::encode_flac(&[&x, &x], 44_100, 16).unwrap(),
            ),
        ] {
            let a = decode_audio(&bytes).unwrap_or_else(|| panic!("{kind} reads"));
            assert_eq!(a.channels.len(), 2, "{kind}");
            assert!((a.sample_rate - 44_100.0).abs() < 1.0);
            // about a second (an MP3 has some padding)
            assert!(
                (a.frames() as f64 / 44_100.0 - 1.0).abs() < 0.1,
                "{kind} {}",
                a.frames()
            );
            let peak = a.channels[0].iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!((peak - 0.5).abs() < 0.05, "{kind} {peak}");
            let (b, kept) = import_audio(bytes).unwrap();
            assert_eq!(b.frames(), a.frames());
            assert!(decode_wav(&kept).is_some(), "{kind} is kept as WAV");
        }
        assert!(import_audio(b"not audio".to_vec()).is_err());
        assert!(
            is_audio_file("Song.MP3") && is_audio_file("a.b.flac") && !is_audio_file("notes.txt")
        );
    }
}
