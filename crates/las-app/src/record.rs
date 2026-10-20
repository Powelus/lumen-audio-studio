// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording audio, as FL Studio's record button does: while it is armed and the song
//! plays, the default input is captured; when playback stops, the take becomes an audio
//! clip in the playlist where the recording started.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The input device, open and ready to capture.
pub struct Recorder {
    _stream: cpal::Stream,
    capture: Arc<AtomicBool>,
    samples: Consumer<f32>,
    pub channels: usize,
    pub sample_rate: u32,
    /// What has been captured since [`Recorder::begin`].
    pub take: Vec<f32>,
}

impl Recorder {
    /// Open the default input device.
    pub fn open() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("no audio input device found")?;
        let supported = device
            .default_input_config()
            .map_err(|e| format!("cannot read the input format: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate;
        // ten seconds of slack between the audio thread and the screen
        let (tx, rx) = RingBuffer::<f32>::new(sample_rate as usize * channels * 10);
        let capture = Arc::new(AtomicBool::new(false));
        let on = capture.clone();
        let stream = match format {
            SampleFormat::F32 => input::<f32>(&device, config, tx, on),
            SampleFormat::I16 => input::<i16>(&device, config, tx, on),
            SampleFormat::U16 => input::<u16>(&device, config, tx, on),
            SampleFormat::I32 => input::<i32>(&device, config, tx, on),
            other => Err(format!("unsupported input format {other}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("cannot start the input: {e}"))?;
        Ok(Self {
            _stream: stream,
            capture,
            samples: rx,
            channels,
            sample_rate,
            take: Vec::new(),
        })
    }

    /// Start a take.
    pub fn begin(&mut self) {
        self.drain();
        self.take.clear();
        self.capture.store(true, Ordering::Relaxed);
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.load(Ordering::Relaxed)
    }

    /// Move what the input captured into the take. Called every frame.
    pub fn drain(&mut self) {
        let capturing = self.is_capturing();
        while let Ok(v) = self.samples.pop() {
            if capturing {
                self.take.push(v);
            }
        }
    }

    /// End the take and hand it over as a WAV file, if anything was captured.
    pub fn end(&mut self) -> Option<Vec<u8>> {
        self.drain();
        self.capture.store(false, Ordering::Relaxed);
        let take = std::mem::take(&mut self.take);
        (!take.is_empty()).then(|| to_wav(&take, self.channels, self.sample_rate))
    }
}

fn input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut tx: Producer<f32>,
    on: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if on.load(Ordering::Relaxed) {
                    for &s in data {
                        // a full buffer drops audio rather than blocking the device
                        let _ = tx.push(s.to_sample::<f32>());
                    }
                }
            },
            |e| eprintln!("input: {e}"),
            None,
        )
        .map_err(|e| format!("cannot open the input: {e}"))
}

/// The first playlist track with no clip from `at` on, for a new take.
pub fn free_track(p: &las_project::Project, at: f64) -> usize {
    (0..)
        .find(|&t| {
            !p.playlist
                .clips
                .iter()
                .any(|c| c.track == t && c.start + c.length > at)
        })
        .unwrap_or(0)
}

/// Interleaved samples as a 16-bit WAV file (mono and stereo stay as they are; more
/// channels keep the first two).
pub fn to_wav(interleaved: &[f32], channels: usize, sample_rate: u32) -> Vec<u8> {
    let ch = channels.max(1);
    let keep = ch.min(2);
    let mut out = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: keep as u16,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    if let Ok(mut w) = hound::WavWriter::new(&mut out, spec) {
        for frame in interleaved.chunks_exact(ch) {
            for &s in &frame[..keep] {
                let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
        let _ = w.finalize();
    }
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_take_goes_on_the_first_track_free_from_its_start() {
        let mut p = las_project::create::empty_project("x");
        assert_eq!(free_track(&p, 0.0), 0);
        let clip = |track, start, length| las_project::Clip {
            track,
            start,
            length,
            ..Default::default()
        };
        p.playlist.clips.push(clip(0, 0.0, 384.0));
        p.playlist.clips.push(clip(1, 768.0, 384.0));
        assert_eq!(free_track(&p, 0.0), 2);
        assert_eq!(free_track(&p, 384.0), 0, "track 0 is free from bar 2");
        assert_eq!(free_track(&p, 2000.0), 0);
    }

    #[test]
    fn a_take_becomes_a_wav_with_its_channels_and_rate() {
        let stereo: Vec<f32> = (0..2000)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.25 })
            .collect();
        let wav = to_wav(&stereo, 2, 48000);
        let a = las_engine::library::decode_wav(&wav).unwrap();
        assert_eq!(
            (a.channels.len(), a.frames(), a.sample_rate),
            (2, 1000, 48000.0)
        );
        assert!((a.channels[0][10] - 0.5).abs() < 1e-3);
        assert!((a.channels[1][10] + 0.25).abs() < 1e-3);
        // four channels keep the first two
        let quad = vec![0.1f32; 400];
        let a = las_engine::library::decode_wav(&to_wav(&quad, 4, 44100)).unwrap();
        assert_eq!((a.channels.len(), a.frames()), (2, 100));
    }
}
