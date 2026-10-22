// SPDX-License-Identifier: GPL-3.0-or-later
//! Live audio output. The audio thread owns the engine. The interface and the
//! MIDI thread talk to it through lock-free queues, and read the meters from
//! atomics, so neither side ever waits for the other.

use crate::audioctl::StreamHandle;
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
use std::panic::{catch_unwind, AssertUnwindSafe};
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
    WeavePatch(Box<las_dsp::modular::Patch>),
    Preview(Box<Preview>),
    Reference(Box<RefTrack>),
}

/// A reference track to compare the mix with: its audio, the gain that matches its
/// loudness to the mix, and the tempo that turns song positions into its seconds.
pub struct RefTrack {
    pub audio: Preview,
    pub gain: f32,
    pub bpm: f64,
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
    /// Play a short chime, to check the output works.
    TestSound,
    /// Click `beats` beats at `bpm` (the first one high), then start the player at tick
    /// `from`: the count-in before recording.
    CountIn {
        beats: u32,
        bpm: f64,
        from: f64,
    },
    /// A key played live on one of the project's channels.
    PlayerNote {
        channel: usize,
        key: u8,
        vel: f64,
        on: bool,
    },
    /// New values for a Weave channel's patch (the same modules and cables), taken
    /// while it plays. The patch it replaces comes back as garbage.
    WeavePatch {
        channel: usize,
        patch: Box<las_dsp::modular::Patch>,
    },
    /// A controller (mod wheel, pitch bend, aftertouch) moved on one of the project's
    /// channels.
    PlayerController {
        channel: usize,
        cc: Cc,
        value: f64,
    },
    /// Load a reference track (or clear it with `None`).
    Reference(Option<Box<RefTrack>>),
    /// Hear the reference instead of the mix (the A/B switch).
    ReferenceOn(bool),
    /// New loudness-matching gain for the reference.
    ReferenceGain(f32),
    /// The insert the tuner listens to, or none.
    Tuner(Option<usize>),
    /// Panic on the audio thread (to test that it is caught).
    #[cfg(test)]
    Panic,
}

/// Meters and counters the audio thread publishes.
#[derive(Default)]
pub struct Meters {
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    load: AtomicU32,
    voices: AtomicU32,
    dropouts: AtomicU32,
    /// Times the audio thread caught a panic (it then plays silence and drops the
    /// project's player, which the app builds again).
    faults: AtomicU32,
    /// The player's position (ticks, as bits of an `f64`) and whether it is playing.
    position: AtomicU64,
    playing: AtomicU32,
    /// Beats of count-in still to come (0 when not counting in).
    counting: AtomicU32,
    /// Frames the driver asked for in its last callback (the real buffer size).
    callback_frames: AtomicU32,
    /// The peak of each mixer insert, master first (bits of an `f32`).
    inserts: [AtomicU32; 32],
    /// The last moments of what goes out, for the analyser.
    pub scope: Scope,
}

/// The newest frames of the output, written by the audio thread and read by the
/// analyser: a fixed ring of atomics and a count of frames written, so the audio thread
/// never waits or allocates. A reader that falls behind just sees the newest frames.
pub struct Scope {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    written: AtomicU64,
}

impl Scope {
    /// Frames kept.
    pub const LEN: usize = 1 << 15;

    pub fn push(&self, l: f32, r: f32) {
        let n = self.written.load(Ordering::Relaxed);
        let at = (n as usize) & (Self::LEN - 1);
        self.left[at].store(l.to_bits(), Ordering::Relaxed);
        self.right[at].store(r.to_bits(), Ordering::Relaxed);
        self.written.store(n + 1, Ordering::Release);
    }

    /// How many frames have been written so far.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Copy the frames from `from` (a count of frames written) up to now into `l` and
    /// `r`, at most the ring's length; the count reached.
    pub fn read_since(&self, from: u64, l: &mut Vec<f32>, r: &mut Vec<f32>) -> u64 {
        let end = self.written();
        let start = from.max(end.saturating_sub(Self::LEN as u64 - 1024));
        for n in start..end {
            let at = (n as usize) & (Self::LEN - 1);
            l.push(f32::from_bits(self.left[at].load(Ordering::Relaxed)));
            r.push(f32::from_bits(self.right[at].load(Ordering::Relaxed)));
        }
        end
    }
}

impl Default for Scope {
    fn default() -> Self {
        let ring = || (0..Self::LEN).map(|_| AtomicU32::new(0)).collect();
        Self {
            left: ring(),
            right: ring(),
            written: AtomicU64::new(0),
        }
    }
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
    /// Panics the audio thread caught.
    pub fn faults(&self) -> u32 {
        self.faults.load(Ordering::Relaxed)
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
    /// Frames in the driver's last callback, 0 before the first one. With the device's
    /// own buffer size this is the only way to know it.
    pub fn callback_frames(&self) -> u32 {
        self.callback_frames.load(Ordering::Relaxed)
    }
    /// Beats of count-in still to come, 0 when not counting in.
    pub fn counting(&self) -> u32 {
        self.counting.load(Ordering::Relaxed)
    }
}

/// The output the user chose in Settings. Empty or 0 is "the system's default".
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioChoice {
    /// The audio driver (cpal host) by name: "WASAPI" or "ASIO" on Windows, "ALSA" or
    /// "JACK" on Linux, "CoreAudio" on macOS. Empty is Automatic (see [`auto_asio`]).
    pub driver: String,
    /// The output device, by name.
    pub output: String,
    /// Frames per second.
    pub sample_rate: u32,
    /// Frames per buffer.
    pub buffer: u32,
    /// The input device recording uses, by name.
    pub input: String,
}

/// Sample rates offered in Settings.
pub const RATES: [u32; 4] = [44100, 48000, 88200, 96000];
/// Buffer sizes offered in Settings, frames.
pub const BUFFERS: [u32; 8] = [64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// How long a buffer of `frames` lasts, milliseconds.
pub fn latency_ms(frames: u32, rate: u32) -> f64 {
    if rate == 0 {
        0.0
    } else {
        f64::from(frames) * 1000.0 / f64::from(rate)
    }
}

/// The name a device is shown and saved by.
pub fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|x| x.name().to_string())
        .or_else(|_| d.id().map(|id| id.to_string()))
        .unwrap_or_else(|_| "Unknown device".into())
}

/// The audio drivers this build can use, by name, the system's default first. Only on
/// the audio-control thread (see [`device_lists`]).
fn drivers() -> Vec<String> {
    let default = cpal::default_host().id();
    let mut v = vec![default.name().to_string()];
    for id in cpal::available_hosts() {
        if id != default {
            v.push(id.name().to_string());
        }
    }
    v
}

/// Whether this build can talk to ASIO drivers (Windows builds made with the ASIO SDK).
pub const HAS_ASIO: bool = cfg!(all(windows, feature = "asio"));

/// What Automatic does, for Settings.
pub const AUTO_HELP: &str = "Picks the best driver for you: your audio interface's ASIO driver, then FlexASIO, then FL Studio ASIO or ASIO4ALL, then Windows audio.";

/// ASIO drivers that wrap other drivers rather than belong to an audio interface.
fn generic_asio(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "asio4all",
        "fl studio asio",
        "flstudio asio",
        "generic low latency",
        "asio2wasapi",
        "flexasio",
        "voicemeeter",
        "asio4all v2",
        // the onboard sound's ASIO driver: it holds the device and often will not
        // start, so Automatic leaves it to Windows audio
        "realtek asio",
    ]
    .iter()
    .any(|g| n.contains(g))
}

/// Which ASIO device Automatic opens, from the ASIO devices there are: the audio
/// interface's own driver, else FlexASIO, else FL Studio ASIO, else ASIO4ALL. None: use the system's
/// default driver (WASAPI on Windows).
pub fn auto_asio(devices: &[String]) -> Option<String> {
    let has = |key: &str| {
        devices
            .iter()
            .find(|d| d.to_ascii_lowercase().contains(key))
            .cloned()
    };
    devices
        .iter()
        .find(|d| !generic_asio(d))
        .cloned()
        .or_else(|| has("flexasio"))
        .or_else(|| has("fl studio asio"))
        .or_else(|| has("flstudio asio"))
        .or_else(|| has("asio4all"))
}

/// What a driver is, in a few words, for Settings.
pub fn driver_help(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "asio" => "ASIO talks to the audio interface directly and gives the lowest latency. Pick your interface's own ASIO driver, or ASIO4ALL or FL Studio ASIO if your sound card has none. Only one program can use most ASIO drivers at a time",
        "wasapi" => "WASAPI is Windows' own audio system: it works with every device and lets other programs play at the same time, with a little more latency than ASIO",
        "alsa" => "ALSA is Linux's own audio system",
        "jack" => "JACK is a low latency audio server for Linux",
        "coreaudio" => "Core Audio is macOS's own audio system, with low latency for every device",
        _ => "The system's audio driver",
    }
}

/// The host for a driver name, or the system's default when it is "" or not there.
fn host(driver: &str) -> cpal::Host {
    if !driver.is_empty() {
        if let Some(id) = cpal::available_hosts()
            .into_iter()
            .find(|id| id.name().eq_ignore_ascii_case(driver))
        {
            if let Ok(h) = cpal::host_from_id(id) {
                return h;
            }
        }
    }
    cpal::default_host()
}

/// The drivers, and the output and input devices of one driver, by name, read on the
/// audio-control thread. A driver that fails or crashes while listing gives no devices.
pub fn device_lists(driver: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let driver = driver.to_string();
    crate::audioctl::run(move || {
        let list = |f: fn(&str) -> Vec<String>| {
            catch_unwind(AssertUnwindSafe(|| f(&driver))).unwrap_or_else(|p| {
                eprintln!(
                    "audio: listing {driver} devices: {}",
                    crate::audioctl::panic_text(p)
                );
                Vec::new()
            })
        };
        (drivers(), list(output_devices), list(input_devices))
    })
    .unwrap_or_else(|e| {
        eprintln!("audio: listing devices: {e}");
        Default::default()
    })
}

/// The ASIO devices there are, by name, read on the audio-control thread (to tell
/// whether FlexASIO is installed). None when listing fails or crashes.
pub fn asio_devices() -> Vec<String> {
    crate::audioctl::run(|| {
        catch_unwind(|| output_devices("ASIO")).unwrap_or_else(|p| {
            eprintln!(
                "audio: listing ASIO devices: {}",
                crate::audioctl::panic_text(p)
            );
            Vec::new()
        })
    })
    .unwrap_or_else(|e| {
        eprintln!("audio: listing ASIO devices: {e}");
        Vec::new()
    })
}

/// The output devices the driver has, by name. Only on the audio-control thread.
fn output_devices(driver: &str) -> Vec<String> {
    let mut v: Vec<String> = host(driver)
        .output_devices()
        .map(|it| it.map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    v.dedup();
    v
}

/// The input devices the driver has, by name. Only on the audio-control thread.
fn input_devices(driver: &str) -> Vec<String> {
    let mut v: Vec<String> = host(driver)
        .input_devices()
        .map(|it| it.map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    v.dedup();
    v
}

/// The chosen output device, or the default one when it is not chosen or not there.
/// The flag says the chosen one was missing.
fn pick_output(name: &str, driver: &str) -> (Option<cpal::Device>, bool) {
    let host = host(driver);
    if !name.is_empty() {
        if let Some(d) = host
            .output_devices()
            .ok()
            .and_then(|mut it| it.find(|d| device_name(d) == name))
        {
            return (Some(d), false);
        }
        return (first_output(&host), true);
    }
    (first_output(&host), false)
}

/// The driver's default output; ASIO has no default, so its first device.
fn first_output(host: &cpal::Host) -> Option<cpal::Device> {
    host.default_output_device()
        .or_else(|| host.output_devices().ok().and_then(|mut it| it.next()))
}

/// The chosen input device, or the default one. Only on the audio-control thread.
pub fn pick_input(name: &str, driver: &str) -> Option<cpal::Device> {
    let host = host(driver);
    if !name.is_empty() {
        if let Some(d) = host
            .input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| device_name(d) == name))
        {
            return Some(d);
        }
    }
    host.default_input_device()
        .or_else(|| host.input_devices().ok().and_then(|mut it| it.next()))
}

pub struct Audio {
    /// The output stream, kept on the audio-control thread.
    _stream: StreamHandle,
    commands: Producer<Command>,
    garbage: Consumer<Garbage>,
    pub meters: Arc<Meters>,
    pub sample_rate: u32,
    pub device: String,
    /// The driver the output is open on ("WASAPI", "ASIO", ...).
    pub driver: String,
    /// Whether Automatic chose the driver.
    pub auto: bool,
    /// The frames per buffer asked for (None: the device's own choice).
    pub buffer: Option<u32>,
    /// What did not go as chosen (a missing device, a rate it does not take).
    pub note: Option<String>,
    /// What the insert the tuner listens to plays (mono, at `sample_rate`).
    pub tuner: Consumer<f32>,
    /// Wavetables are built here, off the audio thread, and shared.
    tables: TableLibrary,
}

/// Where MIDI input sends its messages.
pub type MidiSender = Producer<Command>;

impl Audio {
    /// Open the default output device. Returns the audio handle and the queue
    /// for MIDI input.
    #[allow(dead_code)]
    pub fn start() -> Result<(Self, MidiSender), String> {
        Self::start_with(&AudioChoice::default())
    }

    /// Open the output chosen in Settings. What cannot be had as chosen falls back to the
    /// device's defaults, then to the system's driver and any output it has (and says
    /// so in `note`). All of it runs on the audio-control thread.
    pub fn start_with(choice: &AudioChoice) -> Result<(Self, MidiSender), String> {
        let choice = choice.clone();
        crate::audioctl::run(move || Self::start_here(&choice))?
    }

    /// One try at opening, with a driver that crashes turned into an error.
    fn attempt(choice: &AudioChoice) -> Result<(Self, MidiSender), String> {
        catch_unwind(AssertUnwindSafe(|| Self::open_with(choice))).unwrap_or_else(|p| {
            Err(format!(
                "the driver crashed: {}",
                crate::audioctl::panic_text(p)
            ))
        })
    }

    /// [`Audio::start_with`], on the audio-control thread.
    fn start_here(choice: &AudioChoice) -> Result<(Self, MidiSender), String> {
        let auto = choice.driver.is_empty();
        let shown = |output: &str| {
            if output.is_empty() {
                "the default output".to_string()
            } else {
                output.to_string()
            }
        };
        // what did not open, to say why
        let mut tried: Vec<String> = Vec::new();
        let done = |got: (Self, MidiSender), tried: &[String]| {
            let (mut a, tx) = got;
            a.auto = auto;
            if !tried.is_empty() {
                let mut n = format!(
                    "{} did not open, using {} ({})",
                    tried.join("; "),
                    a.device,
                    a.driver
                );
                if let Some(old) = a.note.take() {
                    n = format!("{n}. {old}");
                }
                a.note = Some(n);
            }
            Ok((a, tx))
        };
        if !auto {
            match Self::attempt(choice) {
                Ok(got) => return Ok(got),
                Err(e) => tried.push(format!(
                    "{} ({}): {e}",
                    shown(&choice.output),
                    choice.driver
                )),
            }
        } else if HAS_ASIO {
            // Automatic: the best ASIO device there is, if there is any ASIO driver
            let devices = catch_unwind(|| output_devices("ASIO")).unwrap_or_else(|p| {
                tried.push(format!(
                    "listing ASIO drivers: {}",
                    crate::audioctl::panic_text(p)
                ));
                Vec::new()
            });
            if let Some(dev) = auto_asio(&devices) {
                let asio = AudioChoice {
                    driver: "ASIO".into(),
                    output: dev.clone(),
                    ..choice.clone()
                };
                match Self::attempt(&asio) {
                    Ok(got) => return done(got, &tried),
                    Err(e) => tried.push(format!("{dev} (ASIO): {e}")),
                }
            }
        }
        // the system's own driver (WASAPI on Windows): the chosen device under
        // Automatic, else its default device
        let system = cpal::default_host().id().name().to_string();
        let sys = AudioChoice {
            driver: String::new(),
            output: if auto {
                choice.output.clone()
            } else {
                String::new()
            },
            ..choice.clone()
        };
        match Self::attempt(&sys) {
            Ok(got) => return done(got, &tried),
            Err(e) => tried.push(format!("{} ({system}): {e}", shown(&sys.output))),
        }
        // then every output it has, with the device's own rate and buffer
        for output in output_devices("") {
            if output == sys.output {
                continue;
            }
            let any = AudioChoice {
                driver: String::new(),
                output: output.clone(),
                sample_rate: 0,
                buffer: 0,
                ..choice.clone()
            };
            match Self::attempt(&any) {
                Ok(got) => return done(got, &tried),
                Err(e) => tried.push(format!("{output} ({system}): {e}")),
            }
        }
        Err(format!("No output would open. {}", tried.join("; ")))
    }

    /// Open the output on the chosen driver ("" is the system's default).
    fn open_with(choice: &AudioChoice) -> Result<(Self, MidiSender), String> {
        let driver = host(&choice.driver).id().name().to_string();
        let (device, missing) = pick_output(&choice.output, &choice.driver);
        let device = device.ok_or("No audio output device found")?;
        let mut notes = Vec::new();
        if !choice.driver.is_empty() && !driver.eq_ignore_ascii_case(&choice.driver) {
            notes.push(format!(
                "The {} driver is not in this build or not installed, using {driver}",
                choice.driver
            ));
        }
        if missing {
            notes.push(format!(
                "{} is not there, using the default output",
                choice.output
            ));
        }
        let name = device_name(&device);
        let supported = device
            .default_output_config()
            .map_err(|e| format!("cannot read the output format: {e}"))?;
        let format = supported.sample_format();
        let base: StreamConfig = supported.into();
        // the chosen rate and buffer, if the device takes them
        let mut config = base;
        if choice.sample_rate > 0 {
            let takes = device.supported_output_configs().is_ok_and(|mut it| {
                it.any(|r| r.channels() == base.channels && r.contains_rate(choice.sample_rate))
            });
            if takes {
                config.sample_rate = choice.sample_rate;
            } else {
                notes.push(format!(
                    "The device does not take {} Hz, using {} Hz",
                    choice.sample_rate, base.sample_rate
                ));
            }
        }
        if choice.buffer > 0 {
            config.buffer_size = cpal::BufferSize::Fixed(choice.buffer);
        }
        let open = |config: StreamConfig| -> Result<(Self, MidiSender), String> {
            let sample_rate = config.sample_rate;
            let buffer = match config.buffer_size {
                cpal::BufferSize::Fixed(n) => Some(n),
                cpal::BufferSize::Default => None,
            };
            let (ui_tx, ui_rx) = RingBuffer::<Command>::new(512);
            let (midi_tx, midi_rx) = RingBuffer::<Command>::new(512);
            let (garbage_tx, garbage_rx) = RingBuffer::<Garbage>::new(64);
            let meters = Arc::new(Meters::default());
            let (tuner_tx, tuner_rx) = RingBuffer::<f32>::new(16384);
            let mut state = Renderer::new(
                sample_rate as f64,
                config.channels as usize,
                ui_rx,
                midi_rx,
                garbage_tx,
                meters.clone(),
            );
            state.tuner = Some(tuner_tx);
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
                    _stream: StreamHandle::keep(stream),
                    commands: ui_tx,
                    garbage: garbage_rx,
                    meters,
                    sample_rate,
                    device: name.clone(),
                    driver: driver.clone(),
                    auto: false,
                    buffer,
                    note: None,
                    tuner: tuner_rx,
                    tables: TableLibrary::new(),
                },
                midi_tx,
            ))
        };
        let mut got = open(config);
        if got.is_err() && config != base {
            // what was chosen does not work here: the device's own settings
            notes
                .push("The chosen rate or buffer size did not work, using the device's own".into());
            got = open(base);
        }
        let (mut audio, tx) = got?;
        audio.note = (!notes.is_empty()).then(|| notes.join(". "));
        Ok((audio, tx))
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
    /// Frames the live effects chain has been quiet in and out (smart disable).
    chain_quiet: usize,
    /// Chimes and clicks the renderer makes itself: frames until it starts, pitch,
    /// length in frames. Room for a few, made before the stream starts.
    beeps: Vec<Beep>,
    /// The count-in: frames until the player starts, the tick it starts at, and the
    /// beats still to come.
    count_in: Option<(usize, f64, usize)>,
    /// The reference track, where it has got to (its frames), and whether it is heard.
    reference: Option<(Box<RefTrack>, f64)>,
    reference_on: bool,
    /// Where the tuner's signal goes, and the insert it comes from.
    tuner: Option<Producer<f32>>,
    tuner_insert: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct Beep {
    wait: usize,
    freq: f32,
    len: usize,
    at: usize,
    level: f32,
}

/// The most beeps the renderer keeps at once.
const BEEPS: usize = 16;

/// Add the beeps sounding in `l` and `r`.
fn render_beeps(beeps: &mut Vec<Beep>, sr: f64, l: &mut [f32], r: &mut [f32]) {
    let sr = sr as f32;
    for b in beeps.iter_mut() {
        for i in 0..l.len() {
            if b.wait > 0 {
                b.wait -= 1;
                continue;
            }
            if b.at >= b.len {
                break;
            }
            let k = b.at as f32;
            let env = (1.0 - k / b.len as f32).powi(2) * (k / (sr * 0.002)).min(1.0);
            let v = (k * b.freq * std::f32::consts::TAU / sr).sin() * env * b.level;
            l[i] += v;
            r[i] += v;
            b.at += 1;
        }
    }
    beeps.retain(|b| b.at < b.len);
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
            chain_quiet: 0,
            beeps: Vec::with_capacity(BEEPS),
            count_in: None,
            reference: None,
            reference_on: false,
            tuner: None,
            tuner_insert: None,
        }
    }

    /// Queue a beep (dropped when there is no room, so nothing is allocated here).
    fn beep(&mut self, wait: usize, freq: f32, seconds: f64, level: f32) {
        if self.beeps.len() < BEEPS {
            self.beeps.push(Beep {
                wait,
                freq,
                len: (seconds * self.sr) as usize,
                at: 0,
                level,
            });
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
                p.set_tap(self.tuner_insert);
                // the old player goes back to be freed off this thread
                if let Some(old) = self.player.replace(p) {
                    let _ = self.garbage.push(Garbage::Player(old));
                }
            }
            Command::PlayerPlay(from) => {
                if let Some(p) = &mut self.player {
                    p.play(from);
                }
                // the reference follows the song position
                if let Some((r, pos)) = &mut self.reference {
                    let ppq = f64::from(las_project::PPQ);
                    *pos = from / ppq * 60.0 / r.bpm.max(1.0) * r.audio.rate;
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
                // stopping during a count-in cancels it and its clicks
                if self.count_in.take().is_some() {
                    self.meters.counting.store(0, Ordering::Relaxed);
                    self.beeps.clear();
                }
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
            #[cfg(test)]
            Command::Panic => panic!("a test panic on the audio thread"),
            Command::WeavePatch { channel, patch } => {
                let back = match &mut self.player {
                    Some(p) => p.weave_patch(channel, patch),
                    None => patch,
                };
                let _ = self.garbage.push(Garbage::WeavePatch(back));
            }
            Command::PlayerController { channel, cc, value } => {
                if let Some(p) = &mut self.player {
                    p.controller(channel, cc, value);
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
            Command::TestSound => {
                // a rising major chord, one note after another
                for (i, f) in [523.25f32, 659.25, 783.99, 1046.5].iter().enumerate() {
                    let wait = (i as f64 * 0.16 * self.sr) as usize;
                    self.beep(wait, *f, 0.5, 0.25);
                }
            }
            Command::CountIn { beats, bpm, from } => {
                let beat = (60.0 / bpm.max(1.0) * self.sr) as usize;
                for i in 0..beats.min(BEEPS as u32) as usize {
                    let f = if i == 0 { 1500.0 } else { 1000.0 };
                    self.beep(i * beat, f, 0.03, 0.35);
                }
                self.count_in = Some((beat * beats as usize, from, beat.max(1)));
                self.meters.counting.store(beats, Ordering::Relaxed);
            }
            Command::Reference(r) => {
                let new = r.map(|r| (r, 0.0));
                if let Some((old, _)) = std::mem::replace(&mut self.reference, new) {
                    let _ = self.garbage.push(Garbage::Reference(old));
                }
            }
            Command::ReferenceOn(on) => self.reference_on = on,
            Command::ReferenceGain(g) => {
                if let Some((r, _)) = &mut self.reference {
                    r.gain = g;
                }
            }
            Command::Tuner(i) => {
                self.tuner_insert = i;
                if let Some(p) = &mut self.player {
                    p.set_tap(i);
                }
            }
        }
    }

    /// Fill the device's buffer. A panic while rendering does not take the audio
    /// thread (or the app) down: this buffer is silent, the project's player is let
    /// go of (the app sees the fault and builds a new one) and the next buffer plays.
    fn fill<T>(&mut self, out: &mut [T])
    where
        T: Sample + FromSample<f32>,
    {
        if catch_unwind(AssertUnwindSafe(|| self.fill_inner(out))).is_err() {
            out.fill(T::EQUILIBRIUM);
            if let Some(old) = self.player.take() {
                let _ = self.garbage.push(Garbage::Player(old));
            }
            self.meters.faults.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fill_inner<T>(&mut self, out: &mut [T])
    where
        T: Sample + FromSample<f32>,
    {
        let started = Instant::now();
        las_host::flush_denormals();
        let frames = out.len() / self.channels;
        self.meters
            .callback_frames
            .store(frames as u32, Ordering::Relaxed);
        let now = self.clock as f64 / self.sr;
        while let Ok(c) = self.ui.pop() {
            self.handle(c, now);
        }
        while let Ok(c) = self.midi.pop() {
            self.handle(c, now);
        }
        let mut done = 0;
        while done < frames {
            // the count-in is over: the song starts on this very frame
            if let Some((0, from, _)) = self.count_in {
                self.count_in = None;
                self.meters.counting.store(0, Ordering::Relaxed);
                if let Some(p) = &mut self.player {
                    p.play(from);
                }
            }
            // very large device buffers are rendered in pieces
            let mut n = (frames - done).min(self.l.len());
            if let Some((left, _, beat)) = &mut self.count_in {
                n = n.min(*left);
                let to_come = left.div_ceil(*beat) as u32;
                self.meters
                    .counting
                    .store(to_come.max(1), Ordering::Relaxed);
                *left -= n;
            }
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
                if let Some(tx) = &mut self.tuner {
                    p.drain_tap(|s| {
                        let _ = tx.push(s);
                    });
                }
            }
            let song_playing = self
                .player
                .as_ref()
                .is_some_and(|p| p.is_playing() && !p.finished());
            // the active instrument's effects; the song position follows the clock
            let chain = &mut self.chains[chain_index(self.engine.active)];
            // (smart disable: once nothing has come in or out for a while it sleeps)
            let quiet = |l: &[f32], r: &[f32]| l.iter().chain(r).all(|s| s.abs() < 1e-6);
            let hold = (las_engine::instrument::SMART_HOLD * self.sr) as usize;
            if !quiet(l, r) {
                self.chain_quiet = 0;
            }
            if !chain.is_empty() && self.chain_quiet < hold {
                let pos = Position {
                    beat: t * self.bpm / 60.0,
                    bpm: self.bpm,
                };
                for slot in chain.iter_mut() {
                    slot.unit.set_position(pos);
                    slot.process(l, r, None);
                }
                if quiet(l, r) {
                    self.chain_quiet += n;
                } else {
                    self.chain_quiet = 0;
                }
            }
            if !self.beeps.is_empty() {
                render_beeps(&mut self.beeps, self.sr, l, r);
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
            // the reference track runs along with the song; heard, it replaces the
            // whole mix (master effects included)
            if let Some((rt, pos)) = &mut self.reference {
                let step = rt.audio.rate / self.sr;
                let len = rt.audio.left.len().min(rt.audio.right.len());
                if self.reference_on {
                    l.fill(0.0);
                    r.fill(0.0);
                }
                if song_playing {
                    for i in 0..n {
                        let at = *pos as usize;
                        if at + 1 < len && self.reference_on {
                            let f = (*pos - at as f64) as f32;
                            let (al, ar) = (&rt.audio.left, &rt.audio.right);
                            l[i] = (al[at] + (al[at + 1] - al[at]) * f) * rt.gain;
                            r[i] = (ar[at] + (ar[at + 1] - ar[at]) * f) * rt.gain;
                        }
                        *pos += step;
                    }
                }
            }
            let (mut pl, mut pr) = (0f32, 0f32);
            for i in 0..n {
                let (a, b) = (l[i] * self.volume, r[i] * self.volume);
                pl = pl.max(a.abs());
                pr = pr.max(b.abs());
                self.meters.scope.push(a, b);
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
    fn the_reference_replaces_the_mix_while_b_is_on_and_the_tuner_hears_an_insert() {
        let (mut r, mut tx, _garbage) = renderer();
        let (tuner_tx, mut tuner_rx) = RingBuffer::<f32>::new(1 << 16);
        r.tuner = Some(tuner_tx);
        let mut buf = vec![0f32; 512 * 2];
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        // a constant reference, easy to recognise, at a quarter gain
        let n = 48000 * 30;
        let one: Shared<[f32]> = vec![0.8f32; n].into();
        tx.push(Command::Reference(Some(Box::new(RefTrack {
            audio: Preview {
                left: one.clone(),
                right: one,
                rate: 48000.0,
            },
            gain: 0.25,
            bpm: 120.0,
        }))))
        .ok()
        .unwrap();
        tx.push(Command::Tuner(Some(0))).ok().unwrap();
        tx.push(Command::PlayerPlay(0.0)).ok().unwrap();
        for _ in 0..50 {
            r.fill(&mut buf);
        }
        assert!(
            buf.iter().any(|v| (v - 0.2 * 0.8).abs() > 1e-3),
            "A: the mix is heard"
        );
        let heard = std::iter::from_fn(|| tuner_rx.pop().ok()).count();
        assert!(
            heard >= 512 * 40,
            "the tuner got the master's signal: {heard}"
        );
        tx.push(Command::ReferenceOn(true)).ok().unwrap();
        r.fill(&mut buf);
        r.fill(&mut buf);
        // the volume (0.8) times the matched reference (0.25 * 0.8)
        assert!(
            buf.iter().all(|v| (v - 0.8 * 0.25 * 0.8).abs() < 1e-4),
            "B: only the reference"
        );
        tx.push(Command::ReferenceOn(false)).ok().unwrap();
        tx.push(Command::Reference(None)).ok().unwrap();
        r.fill(&mut buf);
        assert!(r.reference.is_none());
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
    fn a_panic_on_the_audio_thread_is_silence_not_a_crash() {
        let (mut r, mut tx, mut garbage) = renderer();
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        tx.push(Command::Panic).ok().unwrap();
        let mut buf = vec![1f32; 512 * 2];
        r.fill(&mut buf);
        assert_eq!(peak(&buf), 0.0, "the buffer is silent");
        assert_eq!(r.meters.faults(), 1);
        assert!(r.player.is_none(), "the player is let go of");
        let mut got_player = false;
        while let Ok(g) = garbage.pop() {
            got_player |= matches!(g, Garbage::Player(_));
        }
        assert!(got_player, "and freed off the audio thread");
        // the next buffer plays on
        tx.push(Command::TestSound).ok().unwrap();
        r.fill(&mut buf);
        assert!(peak(&buf) > 0.05);
    }

    #[test]
    fn a_weave_patch_is_taken_live_and_the_old_one_comes_back() {
        let (mut r, mut tx, mut garbage) = renderer();
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .ok()
            .unwrap();
        let p = las_dsp::modular::Patch::default();
        tx.push(Command::WeavePatch {
            channel: 0,
            patch: Box::new(p.clone()),
        })
        .ok()
        .unwrap();
        let mut buf = vec![0f32; 512 * 2];
        r.fill(&mut buf);
        let mut back = false;
        while let Ok(g) = garbage.pop() {
            back |= matches!(g, Garbage::WeavePatch(_));
        }
        assert!(back, "freed off the audio thread");
    }

    #[test]
    fn the_test_sound_is_heard_and_ends() {
        let (mut r, mut tx, _g) = renderer();
        tx.push(Command::TestSound).unwrap();
        let mut out = vec![0f32; 2 * 4800];
        r.fill(&mut out);
        assert!(peak(&out) > 0.05);
        // a little over a second later it is over
        for _ in 0..12 {
            r.fill(&mut out);
        }
        assert!(peak(&out) < 1e-6);
    }

    #[test]
    fn a_count_in_clicks_and_then_the_song_starts_on_time() {
        let (mut r, mut tx, _g) = renderer();
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .unwrap();
        // two beats at 120 BPM: one second
        tx.push(Command::CountIn {
            beats: 2,
            bpm: 120.0,
            from: 0.0,
        })
        .unwrap();
        let mut out = vec![0f32; 2 * 4800];
        r.fill(&mut out);
        assert!(peak(&out) > 0.05, "the first click");
        assert_eq!(r.meters.counting(), 2);
        assert!(!r.player.as_ref().unwrap().is_playing());
        // 0.9 s in: still counting, one beat to come
        for _ in 0..8 {
            r.fill(&mut out);
        }
        assert_eq!(r.meters.counting(), 1);
        assert!(!r.player.as_ref().unwrap().is_playing());
        // the count-in ends with this buffer and the song starts on the next frame
        r.fill(&mut out);
        r.fill(&mut out);
        assert!(r.player.as_ref().unwrap().is_playing());
        assert_eq!(r.meters.counting(), 0);
    }

    #[test]
    fn stopping_during_a_count_in_cancels_it() {
        let (mut r, mut tx, _g) = renderer();
        tx.push(Command::SetPlayer(Box::new(project_player())))
            .unwrap();
        tx.push(Command::CountIn {
            beats: 4,
            bpm: 120.0,
            from: 0.0,
        })
        .unwrap();
        let mut out = vec![0f32; 2 * 4800];
        r.fill(&mut out);
        tx.push(Command::PlayerStop).unwrap();
        for _ in 0..30 {
            r.fill(&mut out);
        }
        assert!(!r.player.as_ref().unwrap().is_playing());
        assert_eq!(r.meters.counting(), 0);
    }

    #[test]
    fn the_default_driver_comes_first_and_an_unknown_one_falls_back_to_it() {
        let d = drivers();
        assert_eq!(d[0], cpal::default_host().id().name());
        assert_eq!(host("no such driver").id(), cpal::default_host().id());
        assert_eq!(host("").id(), cpal::default_host().id());
        assert!(driver_help("ASIO").contains("ASIO4ALL"));
        let c: AudioChoice = serde_json::from_str(r#"{"output": "x"}"#).unwrap();
        assert!(c.driver.is_empty(), "old settings get Automatic");
    }

    #[test]
    fn automatic_ranks_the_interface_driver_then_flexasio_fl_and_asio4all() {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let pick = |x: &[&str]| auto_asio(&v(x));
        assert_eq!(
            pick(&["ASIO4ALL v2", "FL Studio ASIO", "Focusrite USB ASIO"]).as_deref(),
            Some("Focusrite USB ASIO")
        );
        assert_eq!(
            pick(&["ASIO4ALL v2", "FL Studio ASIO"]).as_deref(),
            Some("FL Studio ASIO")
        );
        assert_eq!(pick(&["ASIO4ALL v2"]).as_deref(), Some("ASIO4ALL v2"));
        assert_eq!(
            pick(&["ASIO4ALL v2", "FL Studio ASIO", "FlexASIO"]).as_deref(),
            Some("FlexASIO")
        );
        assert_eq!(
            pick(&["FlexASIO", "Focusrite USB ASIO"]).as_deref(),
            Some("Focusrite USB ASIO")
        );
        assert_eq!(pick(&["Generic Low Latency ASIO Driver"]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn latency_is_the_buffer_in_milliseconds() {
        assert!((latency_ms(512, 48000) - 10.666).abs() < 0.01);
        assert_eq!(latency_ms(441, 44100), 10.0);
        assert_eq!(latency_ms(256, 0), 0.0);
    }

    #[test]
    fn audio_choices_load_from_old_settings() {
        let c: AudioChoice = serde_json::from_str(r#"{"buffer": 256}"#).unwrap();
        assert_eq!(c.buffer, 256);
        assert!(c.output.is_empty(), "the default device");
        assert_eq!(c.sample_rate, 0);
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
