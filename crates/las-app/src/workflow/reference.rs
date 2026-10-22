// SPDX-License-Identifier: GPL-3.0-or-later
//! A reference track to A/B the mix against, as mastering tools and Studio One do. The
//! reference plays along with the song; the A/B switch picks what is heard. It is
//! turned to the mix's loudness so that "louder" never passes for "better", it skips
//! the master's effects, and it is never part of an export.

use std::sync::Arc;

/// A loaded reference track.
pub struct Reference {
    pub name: String,
    pub left: Arc<[f32]>,
    pub right: Arc<[f32]>,
    pub rate: f64,
    /// Its integrated loudness, LUFS.
    pub lufs: f64,
    /// The mix's loudness once measured, LUFS.
    pub mix_lufs: Option<f64>,
}

impl Reference {
    /// Read a WAV file and measure it.
    pub fn from_wav(name: &str, bytes: &[u8]) -> Result<Self, String> {
        let a = las_engine::library::decode_wav(bytes).ok_or("not a WAV file this app can read")?;
        let left: Vec<f32> = a.channels.first().cloned().unwrap_or_default();
        let right: Vec<f32> = a.channels.get(1).cloned().unwrap_or_else(|| left.clone());
        if left.is_empty() {
            return Err("the file is empty".into());
        }
        let lufs = las_dsp::loudness::analyze_loudness(&[&left, &right], a.sample_rate).integrated;
        Ok(Self {
            name: name.to_string(),
            left: left.into(),
            right: right.into(),
            rate: a.sample_rate,
            lufs,
            mix_lufs: None,
        })
    }

    /// The gain that brings the reference to the mix's loudness.
    pub fn gain(&self) -> f32 {
        match_gain(self.mix_lufs, self.lufs)
    }
}

/// Gain (linear) that turns something at `reference` LUFS into `mix` LUFS, at most
/// 24 dB either way; unity while either is unknown or silent.
pub fn match_gain(mix: Option<f64>, reference: f64) -> f32 {
    match mix {
        Some(m) if m.is_finite() && reference.is_finite() => {
            let db = (m - reference).clamp(-24.0, 24.0);
            10f64.powf(db / 20.0) as f32
        }
        _ => 1.0,
    }
}

/// The loudness of the project's mix (the song), measured from an offline render.
pub fn mix_loudness(
    project: &las_project::Project,
    samples: &std::collections::BTreeMap<String, las_project::Sample>,
) -> Result<f64, String> {
    let r = las_engine::render_mix_with(
        project,
        samples,
        &las_engine::RenderOptions {
            sample_rate: 22050.0,
            tail: 0.0,
            mode: las_engine::Mode::Song,
        },
    )?;
    Ok(las_dsp::loudness::analyze_loudness(&[&r.left, &r.right], r.sample_rate).integrated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(amp: f32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 24000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
        for i in 0..24000 * 2 {
            let v = (i as f32 * 0.07).sin() * amp;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        out.into_inner()
    }

    #[test]
    fn a_louder_reference_is_turned_down_to_the_mix() {
        let quiet = Reference::from_wav("q", &wav(0.1)).unwrap();
        let loud = Reference::from_wav("l", &wav(0.8)).unwrap();
        // eight times the level is 18 dB louder
        assert!((loud.lufs - quiet.lufs - 18.06).abs() < 0.2);
        let g = match_gain(Some(quiet.lufs), loud.lufs);
        assert!((g - 0.125).abs() < 0.01, "{g}");
        assert_eq!(match_gain(None, loud.lufs), 1.0);
        assert_eq!(match_gain(Some(f64::NEG_INFINITY), loud.lufs), 1.0);
        assert!((match_gain(Some(0.0), -60.0) - 10f32.powf(24.0 / 20.0)).abs() < 1e-3);
        assert!(Reference::from_wav("x", b"nope").is_err());
    }

    #[test]
    fn the_mix_is_measured_from_the_song() {
        let p = las_project::create::default_project();
        let l = mix_loudness(&p, &Default::default()).unwrap();
        assert!(l.is_finite() && l < 0.0 && l > -40.0, "{l}");
    }
}
