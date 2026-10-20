// SPDX-License-Identifier: GPL-3.0-or-later
//! Live audio output. The audio thread owns the engine. The interface and the
//! MIDI thread talk to it through lock-free queues, and read the meters from
//! atomics, so neither side ever waits for the other.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use las_dsp::engine::{prepare_patch, Cc, Event, PreparedPatch};
use las_dsp::fm::{fm_engine, Fm, FmEngine};
use las_dsp::fx::registry::{FxSlot, Params};
use las_dsp::fx::stepfx::Position;
use las_dsp::mist::{mist_engine, Mist, MistEngine};
use las_dsp::synth::{prepare_synth_patch, synth_engine, PreparedSynthPatch, SynthEngine};
use las_dsp::wavetable::TableLibrary;
use las_engine::Player;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc as Shared;
use std::sync::Arc;
use std::time::Instant;

/// A prepared patch, boxed so it moves through the queue cheaply.
pub enum Patch {
    Facet(Box<PreparedPatch<Fm>>),
    Mist(Box<PreparedPatch<Mist>>),
    Polaris(Box<PreparedSynthPatch>),
}

/// What the audio thread hands back to be freed elsewhere (it is only ever dropped).
#[allow(dead_code)]
pub enum Garbage {
    Patch(Patch),
    #[allow(clippy::box_collection)] // keeps the ring slot small
    Chain(Box<Vec<FxSlot>>),
    Params(Box<Params>),
    Player(Box<Player>),
    Preview(Box<Preview>),
}

/// The instruments the app can play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instrument {
    Facet,
    Mist,
    Polaris,
}

/// A source sample for Mist. The buffers are shared with the interface, so
/// swapping one in or out never frees a large buffer on the audio thread.
pub struct Source {
    pub left: Shared<[f32]>,
    pub right: Shared<[f32]>,
    pub rate: f64,
}

/// A sample being auditioned from the browser. The buffers are shared with the
/// interface so that nothing large is freed on the audio thread.
pub struct Preview {
    pub left: Shared<[f32]>,
    pub right: Shared<[f32]>,
    pub rate: f64,
}

/// What the interface and MIDI input can ask of the audio thread.
pub enum Command {
    NoteOn {
        key: u8,
        vel: f64,
    },
    NoteOff {
        key: u8,
    },
    AllOff,
    Controller {
        cc: Cc,
        value: f64,
    },
    Patch(Patch),
    Instrument(Instrument),
    Source(Source),
    Volume(f32),
    /// Replace an instrument's whole effects chain (the old chain comes back).
    #[allow(clippy::box_collection)] // keeps the command small
    Chain(Instrument, Box<Vec<FxSlot>>),
    /// New parameters for one effect of an instrument's chain.
    FxParams(Instrument, usize, Box<Params>),
    /// Switch an effect on or off and set its dry/wet mix.
    FxMix(Instrument, usize, bool, f64),
    Tempo(f64),
    /// Swap in the player of a project (the old one comes back).
    SetPlayer(Box<Player>),
    /// Start the player at a tick.
    PlayerPlay(f64),
    PlayerStop,
    /// The metronome clicks on the beats while the project plays.
    Metronome(bool),
    /// Audition a sample (it replaces the one playing).
    Preview(Box<Preview>),
    PreviewStop,
    /// A key played live on one of the project's channels.
    PlayerNote {
        channel: usize,
        key: u8,
        vel: f64,
        on: bool,
    },
}

/// Meters and counters the audio thread publishes.
#[derive(Default)]
pub struct Meters {
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    load: AtomicU32,
    voices: AtomicU32,
    dropouts: AtomicU32,
    /// The player's position (ticks, as bits of an `f64`) and whether it is playing.
    position: AtomicU64,
    playing: AtomicU32,
    /// The peak of each mixer insert, master first (bits of an `f32`).
    inserts: [AtomicU32; 32],
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
    /// Where the project's player is, in ticks.
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }
    /// The peak level of insert `i` of the project's mixer (0 beyond the last).
    pub fn insert_peak(&self, i: usize) -> f32 {
        self.inserts.get(i).map_or(0.0, Self::load_f)
    }
    /// Whether the project's player is playing.
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed) != 0
    }
}

pub struct Audio {
    _stream: cpal::Stream,
    commands: Producer<Command>,
    garbage: Consumer<Garbage>,
    pub meters: Arc<Meters>,
    pub sample_rate: u32,
    pub device: String,
    /// Wavetables are built here, off the audio thread, and shared.
    tables: TableLibrary,
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
        let (garbage_tx, garbage_rx) = RingBuffer::<Garbage>::new(64);
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
                tables: TableLibrary::new(),
            },
            midi_tx,
        ))
    }

    pub fn send(&mut self, c: Command) {
        // a full queue means the audio thread is stalled; dropping is safest
        let _ = self.commands.push(c);
    }

    /// Parse a patch here and hand it over ready to use.
    pub fn set_patch(&mut self, inst: Instrument, json: &serde_json::Value) -> Result<(), String> {
        let p = match inst {
            Instrument::Facet => Patch::Facet(Box::new(
                prepare_patch::<Fm>(json).map_err(|e| e.to_string())?,
            )),
            Instrument::Mist => Patch::Mist(Box::new(
                prepare_patch::<Mist>(json).map_err(|e| e.to_string())?,
            )),
            Instrument::Polaris => Patch::Polaris(Box::new(
                prepare_synth_patch(&mut self.tables, json).map_err(|e| e.to_string())?,
            )),
        };
        self.send(Command::Patch(p));
        Ok(())
    }

    /// Put a ready-made effects chain on an instrument.
    pub fn set_chain(&mut self, inst: Instrument, slots: Vec<FxSlot>) {
        self.send(Command::Chain(inst, Box::new(slots)));
    }

    /// New parameters for one effect (made with `parse_params`).
    pub fn set_fx_params(&mut self, inst: Instrument, slot: usize, p: Params) {
        self.send(Command::FxParams(inst, slot, Box::new(p)));
    }

    /// Hand over the player of a project; it takes over the output of the last one.
    pub fn set_player(&mut self, p: Player) {
        self.send(Command::SetPlayer(Box::new(p)));
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

fn chain_index(i: Instrument) -> usize {
    match i {
        Instrument::Facet => 0,
        Instrument::Mist => 1,
        Instrument::Polaris => 2,
    }
}

/// Both instruments exist from the start, so switching never allocates.
struct Engines {
    facet: FmEngine,
    mist: MistEngine,
    polaris: SynthEngine,
    active: Instrument,
}

impl Engines {
    fn queue(&mut self, e: Event) {
        match self.active {
            Instrument::Facet => self.facet.queue(&[e]),
            Instrument::Mist => self.mist.queue(&[e]),
            Instrument::Polaris => self.polaris.queue(&[e]),
        }
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], t0: f64) {
        match self.active {
            Instrument::Facet => self.facet.process(l, r, t0),
            Instrument::Mist => self.mist.process(l, r, t0),
            Instrument::Polaris => self.polaris.process(l, r, t0),
        }
    }

    fn active_voices(&self) -> usize {
        match self.active {
            Instrument::Facet => self.facet.active_voices(),
            Instrument::Mist => self.mist.active_voices(),
            Instrument::Polaris => self.polaris.active_voices(),
        }
    }
}

struct Renderer {
    engine: Engines,
    sr: f64,
    channels: usize,
    clock: u64,
    volume: f32,
    ui: Consumer<Command>,
    midi: Consumer<Command>,
    garbage: Producer<Garbage>,
    /// The effects chain of each instrument: Facet, Mist, Polaris.
    chains: [Vec<FxSlot>; 3],
    bpm: f64,
    l: Vec<f32>,
    r: Vec<f32>,
    /// The project's player, and room for what it renders.
    player: Option<Box<Player>>,
    pl: Vec<f32>,
    pr: Vec<f32>,
    /// The sample being auditioned and how far it has played (its own frames).
    preview: Option<(Box<Preview>, f64)>,
    metronome: bool,
    meters: Arc<Meters>,
    load: f32,
}

impl Renderer {
    fn new(
        sr: f64,
        channels: usize,
        ui: Consumer<Command>,
        midi: Consumer<Command>,
        garbage: Producer<Garbage>,
        meters: Arc<Meters>,
    ) -> Self {
        Self {
            engine: Engines {
                facet: fm_engine(sr, 1),
                mist: mist_engine(sr, 1),
                polaris: synth_engine(sr, 1),
                active: Instrument::Polaris,
            },
            sr,
            channels,
            clock: 0,
            volume: 0.8,
            ui,
            midi,
            garbage,
            chains: [Vec::new(), Vec::new(), Vec::new()],
            bpm: 120.0,
            l: vec![0.0; 8192],
            r: vec![0.0; 8192],
            player: None,
            pl: vec![0.0; 8192],
            pr: vec![0.0; 8192],
            preview: None,
            metronome: false,
            meters,
            load: 0.0,
        }
    }

    fn handle(&mut self, c: Command, now: f64) {
        match c {
            Command::NoteOn { key, vel } => self.engine.queue(Event::On {
                time: now,
                key: key as f64,
                vel,
                fine: 0.0,
                pan: 0.0,
            }),
            Command::NoteOff { key } => self.engine.queue(Event::Off {
                time: now,
                key: key as f64,
            }),
            Command::AllOff => self.engine.queue(Event::AllOff { time: now }),
            Command::Controller { cc, value } => self.engine.queue(Event::Cc {
                time: now,
                cc,
                value,
            }),
            Command::Patch(mut p) => {
                // the patch trades places with the current one, which goes back in the
                // same box to be freed off this thread
                match &mut p {
                    Patch::Facet(b) => self.engine.facet.apply_patch(b),
                    Patch::Mist(b) => self.engine.mist.apply_patch(b),
                    Patch::Polaris(b) => self.engine.polaris.apply_patch(b),
                }
                let _ = self.garbage.push(Garbage::Patch(p));
            }
            Command::Chain(inst, mut new) => {
                std::mem::swap(&mut self.chains[chain_index(inst)], &mut *new);
                let _ = self.garbage.push(Garbage::Chain(new));
            }
            Command::FxParams(inst, slot, mut params) => {
                if let Some(s) = self.chains[chain_index(inst)].get_mut(slot) {
                    let p = std::mem::replace(&mut *params, Params::None);
                    *params = s.unit.apply(p);
                }
                let _ = self.garbage.push(Garbage::Params(params));
            }
            Command::FxMix(inst, slot, on, mix) => {
                if let Some(s) = self.chains[chain_index(inst)].get_mut(slot) {
                    s.enabled = on;
                    s.mix = mix;
                }
            }
            Command::Tempo(bpm) => self.bpm = bpm,
            Command::Metronome(on) => {
                self.metronome = on;
                if let Some(p) = &mut self.player {
                    p.set_metronome(on);
                }
            }
            Command::SetPlayer(mut p) => {
                p.set_metronome(self.metronome);
                // the old player goes back to be freed off this thread
                if let Some(old) = self.player.replace(p) {
                    let _ = self.garbage.push(Garbage::Player(old));
                }
            }
            Command::PlayerPlay(from) => {
                if let Some(p) = &mut self.player {
                    p.play(from);
                }
            }
            Command::Preview(p) => {
                if let Some((old, _)) = self.preview.replace((p, 0.0)) {
                    let _ = self.garbage.push(Garbage::Preview(old));
                }
            }
            Command::PreviewStop => {
                if let Some((old, _)) = self.preview.take() {
                    let _ = self.garbage.push(Garbage::Preview(old));
                }
            }
            Command::PlayerStop => {
                if let Some(p) = &mut self.player {
                    p.stop();
                }
            }
            Command::PlayerNote {
                channel,
                key,
                vel,
                on,
            } => {
                if let Some(p) = &mut self.player {
                    if on {
                        p.note_on(channel, key as f64, vel);
                    } else {
                        p.note_off(channel, key as f64);
                    }
                }
            }
            Command::Instrument(i) => {
                if i != self.engine.active {
                    // let go of what the old instrument is playing
                    match self.engine.active {
                        Instrument::Facet => self.engine.facet.all_off(),
                        Instrument::Mist => self.engine.mist.all_off(),
                        Instrument::Polaris => self.engine.polaris.all_off(),
                    }
                    self.engine.active = i;
                }
            }
            Command::Source(src) => {
                self.engine
                    .mist
                    .sound
                    .set_source(src.left, src.right, src.rate);
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
            let t = now + done as f64 / self.sr;
            self.engine.process(l, r, t);
            if let Some(p) = &mut self.player {
                let (pl, pr) = (&mut self.pl[..n], &mut self.pr[..n]);
                p.process(pl, pr);
                for i in 0..n {
                    l[i] += pl[i];
                    r[i] += pr[i];
                }
            }
            // the active instrument's effects; the song position follows the clock
            let chain = &mut self.chains[chain_index(self.engine.active)];
            if !chain.is_empty() {
                let pos = Position {
                    beat: t * self.bpm / 60.0,
                    bpm: self.bpm,
                };
                for slot in chain.iter_mut() {
                    slot.unit.set_position(pos);
                    slot.process(l, r, None);
                }
            }
            if let Some((p, pos)) = &mut self.preview {
                let step = p.rate / self.sr;
                let len = p.left.len().min(p.right.len());
                for i in 0..n {
                    let at = *pos as usize;
                    if at + 1 >= len {
                        break;
                    }
                    let f = (*pos - at as f64) as f32;
                    l[i] += p.left[at] + (p.left[at + 1] - p.left[at]) * f;
                    r[i] += p.right[at] + (p.right[at + 1] - p.right[at]) * f;
                    *pos += step;
                }
                if *pos as usize + 1 >= len {
                    if let Some((old, _)) = self.preview.take() {
                        let _ = self.garbage.push(Garbage::Preview(old));
                    }
                }
            }
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
        if let Some(p) = &self.player {
            for (slot, v) in self.meters.inserts.iter().zip(p.insert_peaks()) {
                Meters::store(slot, *v);
            }
            self.meters
                .position
                .store(p.position().to_bits(), Ordering::Relaxed);
            self.meters.playing.store(
                u32::from(p.is_playing() && !p.finished()),
                Ordering::Relaxed,
            );
        }
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

    fn renderer() -> (Renderer, Producer<Command>, Consumer<Garbage>) {
        let (ui_tx, ui_rx) = RingBuffer::<Command>::new(64);
        let (_midi_tx, midi_rx) = RingBuffer::<Command>::new(64);
        let (garbage_tx, garbage_rx) = RingBuffer::<Garbage>::new(8);
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

    fn gain_slot(db: f64) -> FxSlot {
        use las_dsp::fx::registry::{parse_params, EffectUnit};
        let mut unit = EffectUnit::new("las-stereo", 48000.0).unwrap();
        let state = serde_json::json!({ "gain": db });
        unit.apply(parse_params("las-stereo", &state, 48000.0).unwrap());
        FxSlot::new(unit, 48000.0)
    }

    #[test]
    fn an_effects_chain_changes_the_sound_and_can_be_changed_live() {
        let (mut r, mut tx, mut garbage) = renderer();
        let mut buf = vec![0f32; 1024 * 2];
        tx.push(Command::NoteOn { key: 60, vel: 1.0 }).ok().unwrap();
        for _ in 0..4 {
            r.fill(&mut buf);
        }
        let dry = peak(&buf);
        // minus 12 dB on the active instrument
        tx.push(Command::Chain(
            Instrument::Polaris,
            Box::new(vec![gain_slot(-12.0)]),
        ))
        .ok()
        .unwrap();
        r.fill(&mut buf);
        assert!(garbage.pop().is_ok(), "the empty old chain comes back");
        let wet = peak(&buf);
        assert!(wet < dry * 0.4, "{wet} should be well below {dry}");
        // turning the slot off lets the sound through again (after the old chain's tail)
        tx.push(Command::FxMix(Instrument::Polaris, 0, false, 1.0))
            .ok()
            .unwrap();
        for _ in 0..2 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) > dry * 0.6, "bypassed slot passes the sound");
    }

    #[test]
    fn a_new_patch_is_applied_and_the_old_one_comes_back() {
        let (mut r, mut tx, mut garbage) = renderer();
        let p = prepare_patch::<Fm>(&serde_json::json!({ "algo": 7 })).unwrap();
        tx.push(Command::Patch(Patch::Facet(Box::new(p))))
            .ok()
            .unwrap();
        let mut buf = vec![0f32; 256 * 2];
        r.fill(&mut buf);
        assert!(
            garbage.pop().is_ok(),
            "the old patch is returned for freeing"
        );
    }

    #[test]
    fn switching_to_mist_plays_mist_and_a_source_can_be_swapped() {
        let (mut r, mut tx, mut garbage) = renderer();
        let mut buf = vec![0f32; 1024 * 2];
        tx.push(Command::Instrument(Instrument::Mist)).ok().unwrap();
        tx.push(Command::NoteOn { key: 60, vel: 1.0 }).ok().unwrap();
        for _ in 0..20 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) > 0.005, "Mist plays its built-in texture");
        // an empty source makes Mist silent
        let empty: Shared<[f32]> = Shared::from(Vec::new());
        tx.push(Command::Source(Source {
            left: empty.clone(),
            right: empty,
            rate: 48000.0,
        }))
        .ok()
        .unwrap();
        let p = prepare_patch::<Mist>(&serde_json::json!({ "grain": { "density": 50 } })).unwrap();
        tx.push(Command::Patch(Patch::Mist(Box::new(p))))
            .ok()
            .unwrap();
        for _ in 0..40 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) < 1e-6, "silent with no source");
        assert!(garbage.pop().is_ok());
        // back to Facet: the notes of the other instrument were released
        tx.push(Command::Instrument(Instrument::Facet))
            .ok()
            .unwrap();
        tx.push(Command::NoteOn { key: 69, vel: 1.0 }).ok().unwrap();
        for _ in 0..8 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) > 0.01);
    }

    #[test]
    fn polaris_takes_a_prepared_patch_with_its_own_wavetables() {
        let (mut r, mut tx, mut garbage) = renderer();
        let mut lib = TableLibrary::new();
        let p = prepare_synth_patch(
            &mut lib,
            &serde_json::json!({ "osc": [{ "table": "formant", "randPhase": 0 }] }),
        )
        .unwrap();
        tx.push(Command::Instrument(Instrument::Polaris))
            .ok()
            .unwrap();
        tx.push(Command::Patch(Patch::Polaris(Box::new(p))))
            .ok()
            .unwrap();
        tx.push(Command::NoteOn { key: 60, vel: 1.0 }).ok().unwrap();
        let mut buf = vec![0f32; 1024 * 2];
        for _ in 0..8 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) > 0.005);
        assert!(garbage.pop().is_ok(), "the old synth patch is handed back");
    }

    fn project_player() -> Player {
        let bytes = include_bytes!("../../las-engine/tests/fixtures/engines.las");
        let b = las_project::decode(bytes).unwrap();
        Player::new(
            Arc::new(b.project),
            &b.samples,
            48000.0,
            las_engine::PlayMode::Song,
        )
        .unwrap()
    }

    #[test]
    fn a_project_player_plays_stops_and_is_swapped_out() {
        let (mut r, mut tx, mut garbage) = renderer();
        let mut buf = vec![0f32; 512 * 2];
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        r.fill(&mut buf);
        assert_eq!(peak(&buf), 0.0, "ready, but not playing");
        assert!(!r.meters.playing());
        tx.push(Command::PlayerPlay(0.0)).ok().unwrap();
        let mut loudest = 0f32;
        for _ in 0..200 {
            r.fill(&mut buf);
            loudest = loudest.max(peak(&buf));
        }
        assert!(loudest > 0.01, "the song is heard");
        assert!(r.meters.playing() && r.meters.position() > 0.0);
        // the master's meter moved, and one beyond the mixer stays at zero
        let master = r.meters.insert_peak(0);
        assert!(master > 0.0, "the master's level is published");
        assert_eq!(r.meters.insert_peak(31 + 5), 0.0);
        tx.push(Command::PlayerStop).ok().unwrap();
        for _ in 0..600 {
            r.fill(&mut buf);
        }
        assert!(peak(&buf) < 1e-3, "silent after the release");
        // a new player replaces it and the old one comes back to be freed
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        r.fill(&mut buf);
        assert!(matches!(garbage.pop(), Ok(Garbage::Player(_))));
    }

    #[test]
    fn a_player_built_for_an_edit_carries_on_from_the_position() {
        let (mut r, mut tx, _g) = renderer();
        let mut buf = vec![0f32; 512 * 2];
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        tx.push(Command::PlayerPlay(0.0)).ok().unwrap();
        for _ in 0..60 {
            r.fill(&mut buf);
        }
        let at = r.meters.position();
        assert!(r.meters.playing() && at > 0.0);
        // the interface swaps in a new player and starts it where the old one had got to
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        tx.push(Command::PlayerPlay(at)).ok().unwrap();
        r.fill(&mut buf);
        let after = r.meters.position();
        assert!(r.meters.playing(), "still playing after the swap");
        assert!(
            (after - at).abs() < 96.0 * 2.0,
            "the song carried on near {at}, not {after}"
        );
    }

    #[test]
    fn a_previewed_sample_plays_once_and_is_handed_back() {
        let (mut r, mut tx, mut garbage) = renderer();
        let wave: Shared<[f32]> = Shared::from(
            (0..4800)
                .map(|i| (i as f32 * 0.05).sin())
                .collect::<Vec<_>>(),
        );
        tx.push(Command::Preview(Box::new(Preview {
            left: wave.clone(),
            right: wave,
            rate: 48000.0,
        })))
        .ok()
        .unwrap();
        let mut buf = vec![0f32; 512 * 2];
        r.fill(&mut buf);
        assert!(peak(&buf) > 0.1, "the sample is heard");
        for _ in 0..20 {
            r.fill(&mut buf);
        }
        assert_eq!(peak(&buf), 0.0, "it ends by itself");
        assert!(matches!(garbage.pop(), Ok(Garbage::Preview(_))));
        // stopping early hands it back too
        let wave: Shared<[f32]> = Shared::from(vec![0.5f32; 48000]);
        tx.push(Command::Preview(Box::new(Preview {
            left: wave.clone(),
            right: wave,
            rate: 48000.0,
        })))
        .ok()
        .unwrap();
        r.fill(&mut buf);
        tx.push(Command::PreviewStop).ok().unwrap();
        r.fill(&mut buf);
        assert_eq!(peak(&buf), 0.0);
        assert!(matches!(garbage.pop(), Ok(Garbage::Preview(_))));
    }

    #[test]
    fn keys_play_a_project_channel_live() {
        let (mut r, mut tx, _g) = renderer();
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        tx.push(Command::PlayerNote {
            channel: 0,
            key: 60,
            vel: 1.0,
            on: true,
        })
        .ok()
        .unwrap();
        let mut buf = vec![0f32; 512 * 2];
        let mut loudest = 0f32;
        for _ in 0..40 {
            r.fill(&mut buf);
            loudest = loudest.max(peak(&buf));
        }
        assert!(loudest > 0.005, "a live key sounds through the mixer");
    }

    #[test]
    fn mono_and_odd_buffer_sizes_work() {
        let (ui_tx, ui_rx) = RingBuffer::<Command>::new(8);
        let (_m, midi_rx) = RingBuffer::<Command>::new(8);
        let (garbage_tx, _g) = RingBuffer::<Garbage>::new(8);
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
