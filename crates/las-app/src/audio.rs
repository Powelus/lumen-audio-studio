// SPDX-License-Identifier: GPL-3.0-or-later
//! Live audio output. The audio thread owns the engine. The interface and the
//! MIDI thread talk to it through lock-free queues, and read the meters from
//! atomics, so neither side ever waits for the other.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use las_dsp::engine::{prepare_patch, Cc, Event, PreparedPatch};
use las_dsp::fm::{fm_engine, Fm};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// A prepared Facet patch, boxed so it moves through the queue cheaply.
pub type Patch = Box<PreparedPatch<Fm>>;

/// What the interface and MIDI input can ask of the audio thread.
pub enum Command {
    NoteOn { key: u8, vel: f64 },
    NoteOff { key: u8 },
    AllOff,
    Controller { cc: Cc, value: f64 },
    Patch(Patch),
    Volume(f32),
}

/// Meters and counters the audio thread publishes.
#[derive(Default)]
pub struct Meters {
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    load: AtomicU32,
    voices: AtomicU32,
    dropouts: AtomicU32,
}

impl Meters {
    fn store(slot: &AtomicU32, v: f32) {
        slot.store(v.to_bits(), Ordering::Relaxed);
    }
    fn load_f(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.load(Ordering::Relaxed))
    }
    /// Peak of the last callback, left and right.
    pub fn peaks(&self) -> (f32, f32) {
        (Self::load_f(&self.peak_l), Self::load_f(&self.peak_r))
    }
    /// Share of the buffer time spent rendering (0 to 1 and above).
    pub fn load(&self) -> f32 {
        Self::load_f(&self.load)
    }
    pub fn voices(&self) -> u32 {
        self.voices.load(Ordering::Relaxed)
    }
    /// Callbacks that took longer than the buffer lasts.
    pub fn dropouts(&self) -> u32 {
        self.dropouts.load(Ordering::Relaxed)
    }
}

pub struct Audio {
    _stream: cpal::Stream,
    commands: Producer<Command>,
    garbage: Consumer<Patch>,
    pub meters: Arc<Meters>,
    pub sample_rate: u32,
    pub device: String,
}

/// Where MIDI input sends its messages.
pub type MidiSender = Producer<Command>;

impl Audio {
    /// Open the default output device. Returns the audio handle and the queue
    /// for MIDI input.
    pub fn start() -> Result<(Self, MidiSender), String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output device found")?;
        let name = device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "default output".into());
        let supported = device
            .default_output_config()
            .map_err(|e| format!("cannot read the output format: {e}"))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let sample_rate = config.sample_rate;

        let (ui_tx, ui_rx) = RingBuffer::<Command>::new(512);
        let (midi_tx, midi_rx) = RingBuffer::<Command>::new(512);
        let (garbage_tx, garbage_rx) = RingBuffer::<Patch>::new(64);
        let meters = Arc::new(Meters::default());
        let state = Renderer::new(
            sample_rate as f64,
            config.channels as usize,
            ui_rx,
            midi_rx,
            garbage_tx,
            meters.clone(),
        );
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, state),
            SampleFormat::I16 => build::<i16>(&device, config, state),
            SampleFormat::U16 => build::<u16>(&device, config, state),
            SampleFormat::I32 => build::<i32>(&device, config, state),
            other => Err(format!("unsupported sample format {other}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("cannot start the output: {e}"))?;
        Ok((
            Self {
                _stream: stream,
                commands: ui_tx,
                garbage: garbage_rx,
                meters,
                sample_rate,
                device: name,
            },
            midi_tx,
        ))
    }

    pub fn send(&mut self, c: Command) {
        // a full queue means the audio thread is stalled; dropping is safest
        let _ = self.commands.push(c);
    }

    /// Parse a patch here and hand it over ready to use.
    pub fn set_patch(&mut self, json: &serde_json::Value) -> Result<(), String> {
        let p = prepare_patch::<Fm>(json).map_err(|e| e.to_string())?;
        self.send(Command::Patch(Box::new(p)));
        Ok(())
    }

    /// Free what the audio thread has finished with.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut r: Renderer,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| r.fill(data),
            |e| eprintln!("audio: {e}"),
            None,
        )
        .map_err(|e| format!("cannot open the output: {e}"))
}

struct Renderer {
    engine: las_dsp::fm::FmEngine,
    sr: f64,
    channels: usize,
    clock: u64,
    volume: f32,
    ui: Consumer<Command>,
    midi: Consumer<Command>,
    garbage: Producer<Patch>,
    l: Vec<f32>,
    r: Vec<f32>,
    meters: Arc<Meters>,
    load: f32,
}

impl Renderer {
    fn new(
        sr: f64,
        channels: usize,
        ui: Consumer<Command>,
        midi: Consumer<Command>,
        garbage: Producer<Patch>,
        meters: Arc<Meters>,
    ) -> Self {
        Self {
            engine: fm_engine(sr, 1),
            sr,
            channels,
            clock: 0,
            volume: 0.8,
            ui,
            midi,
            garbage,
            l: vec![0.0; 8192],
            r: vec![0.0; 8192],
            meters,
            load: 0.0,
        }
    }

    fn handle(&mut self, c: Command, now: f64) {
        match c {
            Command::NoteOn { key, vel } => self.engine.queue(&[Event::On {
                time: now,
                key: key as f64,
                vel,
                fine: 0.0,
                pan: 0.0,
            }]),
            Command::NoteOff { key } => self.engine.queue(&[Event::Off {
                time: now,
                key: key as f64,
            }]),
            Command::AllOff => self.engine.queue(&[Event::AllOff { time: now }]),
            Command::Controller { cc, value } => self.engine.queue(&[Event::Cc {
                time: now,
                cc,
                value,
            }]),
            Command::Patch(p) => {
                let old = self.engine.apply_patch(*p);
                // hand the old patch back to be freed off this thread
                let _ = self.garbage.push(Box::new(old));
            }
            Command::Volume(v) => self.volume = v,
        }
    }

    fn fill<T>(&mut self, out: &mut [T])
    where
        T: Sample + FromSample<f32>,
    {
        let started = Instant::now();
        let frames = out.len() / self.channels;
        let now = self.clock as f64 / self.sr;
        while let Ok(c) = self.ui.pop() {
            self.handle(c, now);
        }
        while let Ok(c) = self.midi.pop() {
            self.handle(c, now);
        }
        let mut done = 0;
        while done < frames {
            // very large device buffers are rendered in pieces
            let n = (frames - done).min(self.l.len());
            let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
            l.fill(0.0);
            r.fill(0.0);
            self.engine.process(l, r, now + done as f64 / self.sr);
            let (mut pl, mut pr) = (0f32, 0f32);
            for i in 0..n {
                let (a, b) = (l[i] * self.volume, r[i] * self.volume);
                pl = pl.max(a.abs());
                pr = pr.max(b.abs());
                let frame = &mut out[(done + i) * self.channels..(done + i + 1) * self.channels];
                for (ch, s) in frame.iter_mut().enumerate() {
                    let v = match ch {
                        0 => a,
                        1 => b,
                        _ => 0.0,
                    };
                    *s = T::from_sample(v);
                }
                if self.channels == 1 {
                    frame[0] = T::from_sample((a + b) * 0.5);
                }
            }
            // meters fall slowly, jump up at once
            let (cl, cr) = self.meters.peaks();
            Meters::store(&self.meters.peak_l, pl.max(cl * 0.92));
            Meters::store(&self.meters.peak_r, pr.max(cr * 0.92));
            done += n;
        }
        self.clock += frames as u64;
        let budget = frames as f64 / self.sr;
        let used = started.elapsed().as_secs_f64() / budget;
        self.load = self.load * 0.9 + used as f32 * 0.1;
        Meters::store(&self.meters.load, self.load);
        self.meters
            .voices
            .store(self.engine.active_voices() as u32, Ordering::Relaxed);
        if used > 1.0 {
            self.meters.dropouts.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> (Renderer, Producer<Command>, Consumer<Patch>) {
        let (ui_tx, ui_rx) = RingBuffer::<Command>::new(64);
        let (_midi_tx, midi_rx) = RingBuffer::<Command>::new(64);
        let (garbage_tx, garbage_rx) = RingBuffer::<Patch>::new(8);
        let r = Renderer::new(
            48000.0,
            2,
            ui_rx,
            midi_rx,
            garbage_tx,
            Arc::new(Meters::default()),
        );
        (r, ui_tx, garbage_rx)
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    #[test]
    fn a_note_sounds_and_then_fades() {
        let (mut r, mut tx, _g) = renderer();
        let mut buf = vec![0f32; 512 * 2];
        r.fill(&mut buf);
        assert_eq!(peak(&buf), 0.0, "silent before any note");
        tx.push(Command::NoteOn { key: 69, vel: 1.0 }).ok().unwrap();
        for _ in 0..8 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) > 0.01, "the note is audible");
        assert!(r.meters.peaks().0 > 0.01 && r.meters.voices() == 1);
        tx.push(Command::NoteOff { key: 69 }).ok().unwrap();
        for _ in 0..800 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) < 1e-4, "silent after the release");
    }

    #[test]
    fn volume_scales_the_output() {
        let (mut r, mut tx, _g) = renderer();
        let mut buf = vec![0f32; 1024 * 2];
        tx.push(Command::NoteOn { key: 60, vel: 1.0 }).ok().unwrap();
        r.fill(&mut buf);
        r.fill(&mut buf);
        let loud = peak(&buf);
        tx.push(Command::Volume(0.4)).ok().unwrap();
        // the same stretch of the note, a moment later; compare against the new volume
        r.fill(&mut buf);
        let quiet = peak(&buf);
        assert!(loud > 0.0 && quiet < loud, "{quiet} should be below {loud}");
    }

    #[test]
    fn a_new_patch_is_applied_and_the_old_one_comes_back() {
        let (mut r, mut tx, mut garbage) = renderer();
        let p = prepare_patch::<Fm>(&serde_json::json!({ "algo": 7 })).unwrap();
        tx.push(Command::Patch(Box::new(p))).ok().unwrap();
        let mut buf = vec![0f32; 256 * 2];
        r.fill(&mut buf);
        assert!(
            garbage.pop().is_ok(),
            "the old patch is returned for freeing"
        );
    }

    #[test]
    fn mono_and_odd_buffer_sizes_work() {
        let (ui_tx, ui_rx) = RingBuffer::<Command>::new(8);
        let (_m, midi_rx) = RingBuffer::<Command>::new(8);
        let (garbage_tx, _g) = RingBuffer::<Patch>::new(8);
        let mut r = Renderer::new(
            44100.0,
            1,
            ui_rx,
            midi_rx,
            garbage_tx,
            Arc::new(Meters::default()),
        );
        let mut tx = ui_tx;
        tx.push(Command::NoteOn { key: 64, vel: 0.9 }).ok().unwrap();
        let mut buf = vec![0f32; 333];
        for _ in 0..5 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) > 0.01);
    }

    #[test]
    fn buffers_larger_than_the_scratch_space_are_rendered_in_pieces() {
        let (mut r, mut tx, _g) = renderer();
        tx.push(Command::NoteOn { key: 60, vel: 1.0 }).ok().unwrap();
        let mut buf = vec![0f32; 20000 * 2];
        r.fill(&mut buf);
        assert!(
            peak(&buf[30000..]) > 0.01,
            "audio reaches the end of a long buffer"
        );
    }
}
