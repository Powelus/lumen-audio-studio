// SPDX-License-Identifier: GPL-3.0-or-later
//! A small CLAP plugin to test the host with. As an effect it multiplies its input by a
//! `gain` parameter; as an instrument it plays a sine at the pitch of the note that is
//! held, scaled by the same gain. It has audio and note ports, a parameter and state.
#![allow(unsafe_code)] // the entry point of a CLAP library is exported by a macro

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};

/// The parameter's id.
pub const GAIN: u32 = 1;

pub struct TestPlugin;

pub struct TestShared {
    /// The gain, as the bits of an `f32`.
    gain: AtomicU32,
}

impl TestShared {
    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
    fn set_gain(&self, v: f32) {
        self.gain
            .store(v.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }
}

impl PluginShared<'_> for TestShared {}

pub struct TestMain<'a> {
    shared: &'a TestShared,
}

impl<'a> PluginMainThread<'a, TestShared> for TestMain<'a> {}

pub struct TestProc<'a> {
    shared: &'a TestShared,
    sr: f32,
    phase: f32,
    /// The note held: its key.
    held: Option<f32>,
}

impl Plugin for TestPlugin {
    type AudioProcessor<'a> = TestProc<'a>;
    type Shared<'a> = TestShared;
    type MainThread<'a> = TestMain<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&TestShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for TestPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::{AUDIO_EFFECT, INSTRUMENT};
        PluginDescriptor::new("las.test.gain", "Lumen Test Gain")
            .with_vendor("Lumen Audio Studio")
            .with_description("A gain and a sine, to test plugin hosting")
            .with_features([INSTRUMENT, AUDIO_EFFECT])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<TestShared, PluginError> {
        Ok(TestShared {
            gain: AtomicU32::new(1.0f32.to_bits()),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a TestShared,
    ) -> Result<TestMain<'a>, PluginError> {
        Ok(TestMain { shared })
    }
}

impl PluginAudioPortsImpl for TestMain<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: if is_input { b"In" } else { b"Out" },
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for TestMain<'_> {
    fn count(&self, is_input: bool) -> u32 {
        u32::from(is_input)
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && is_input {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"Notes",
                supported_dialects: NoteDialects::CLAP,
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

impl PluginMainThreadParams for TestMain<'_> {
    fn count(&self) -> u32 {
        1
    }

    fn get_info(&self, index: u32, info: &mut ParamInfoWriter) {
        if index == 0 {
            info.set(&ParamInfo {
                id: ClapId::new(GAIN),
                flags: ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Default::default(),
                name: b"Gain",
                module: b"",
                min_value: 0.0,
                max_value: 4.0,
                default_value: 1.0,
            });
        }
    }

    fn get_value(&self, id: ClapId) -> Option<f64> {
        (id.get() == GAIN).then(|| f64::from(self.shared.gain()))
    }

    fn value_to_text(
        &self,
        id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        if id.get() == GAIN {
            write!(writer, "{value:.2}x")
        } else {
            Err(std::fmt::Error)
        }
    }

    fn text_to_value(&self, id: ClapId, text: &CStr) -> Option<f64> {
        (id.get() == GAIN)
            .then(|| text.to_str().ok()?.trim_end_matches('x').parse().ok())
            .flatten()
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        for e in input {
            if let Some(CoreEventSpace::ParamValue(p)) = e.as_core_event() {
                if p.param_id().map(|i| i.get()) == Some(GAIN) {
                    self.shared.set_gain(p.value() as f32);
                }
            }
        }
    }
}

impl PluginAudioProcessorParams for TestProc<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for e in input {
            if let Some(CoreEventSpace::ParamValue(p)) = e.as_core_event() {
                if p.param_id().map(|i| i.get()) == Some(GAIN) {
                    self.shared.set_gain(p.value() as f32);
                }
            }
        }
    }
}

impl PluginStateImpl for TestMain<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output
            .write_all(&self.shared.gain().to_le_bytes())
            .map_err(Into::into)
    }

    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut b = [0u8; 4];
        input.read_exact(&mut b)?;
        self.shared.set_gain(f32::from_le_bytes(b));
        Ok(())
    }
}

impl<'a> PluginAudioProcessor<'a, TestShared, TestMain<'a>> for TestProc<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main: &TestMain<'a>,
        shared: &'a TestShared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            shared,
            sr: config.sample_rate as f32,
            phase: 0.0,
            held: None,
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        // the events cut the block into stretches with one state each
        let frames = audio.frames_count() as usize;
        let mut cuts: Vec<(usize, Option<CoreEventSpace>)> = events
            .input
            .iter()
            .map(|e| (e.header().time() as usize, e.as_core_event()))
            .collect();
        cuts.push((frames, None));
        let mut at = 0;
        for (time, ev) in cuts {
            let time = time.min(frames);
            if time > at {
                self.render(&mut audio, at..time);
                at = time;
            }
            match ev {
                Some(CoreEventSpace::ParamValue(p)) => {
                    if p.param_id().map(|i| i.get()) == Some(GAIN) {
                        self.shared.set_gain(p.value() as f32);
                    }
                }
                Some(CoreEventSpace::NoteOn(n)) => {
                    self.held = n.key().into_specific().map(f32::from);
                }
                Some(CoreEventSpace::NoteOff(n))
                    if self.held == n.key().into_specific().map(f32::from) =>
                {
                    self.held = None;
                }
                _ => {}
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

impl TestProc<'_> {
    /// Play the frames `range` of the block with the state as it is.
    fn render(&mut self, audio: &mut Audio, range: std::ops::Range<usize>) {
        let gain = self.shared.gain();
        let step = self
            .held
            .map(|k| 440.0 * 2f32.powf((k - 69.0) / 12.0) / self.sr);
        let start = self.phase;
        let mut end = start;
        for mut pair in &mut *audio {
            let Ok(channels) = pair.channels() else {
                continue;
            };
            let Some(channels) = channels.into_f32() else {
                continue;
            };
            for channel in channels {
                let mut ph = start;
                let voice = |ph: &mut f32| match step {
                    Some(s) => {
                        let v = (*ph * std::f32::consts::TAU).sin() * 0.5;
                        *ph = (*ph + s).fract();
                        v
                    }
                    None => 0.0,
                };
                match channel {
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(out) => {
                        for o in &mut out[range.clone()] {
                            *o = voice(&mut ph) * gain;
                        }
                    }
                    ChannelPair::InputOutput(i, o) => {
                        for (i, o) in i[range.clone()].iter().zip(&mut o[range.clone()]) {
                            *o = (*i + voice(&mut ph)) * gain;
                        }
                    }
                    ChannelPair::InPlace(buf) => {
                        for s in &mut buf[range.clone()] {
                            *s = (*s + voice(&mut ph)) * gain;
                        }
                    }
                }
                end = ph;
            }
        }
        self.phase = end;
    }
}

clack_export_entry!(SinglePluginEntry<TestPlugin>);
