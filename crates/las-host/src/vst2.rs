// SPDX-License-Identifier: GPL-3.0-or-later
//! Hosting of VST2 plugins (the VST 2.4 interface): finding out what a library is,
//! loading it, and running it as an instrument or an effect, with its parameters, its
//! state (a chunk, or the parameter values) and its own window. The binary interface
//! is in [`abi`], written for this program.
//!
//! A VST2 plugin is a library that exports `VSTPluginMain`: it is given the host's
//! callback and returns an `AEffect`, and everything after goes through the effect's
//! dispatcher and its process and parameter functions. Unlike CLAP and VST3, a VST2
//! library cannot say what it is without being started, so a scan opens each one (in
//! its own process when the plugin host program is there; see [`crate::scan`]).
//! Like [`crate::Clap`], a [`Vst2`] is built off the audio thread and then used by one
//! thread at a time.

#![allow(non_upper_case_globals, non_snake_case)]

pub mod abi;
mod editor;

use crate::{NoteMsg, Param, PluginInfo, MAX_FRAMES};
use abi::*;
use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use editor::Vst2Editor;

/// The most events given to a plugin in one block.
const MAX_EVENTS: usize = 512;

/// The binary of a VST2 plugin: the file itself, or inside a macOS `.vst` bundle.
pub fn binary(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let bin = path.join("Contents/MacOS").join(&stem);
    if bin.is_file() {
        return Some(bin);
    }
    // a bundle whose binary is named otherwise: the one file in Contents/MacOS
    let mut files = std::fs::read_dir(path.join("Contents/MacOS"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file());
    let first = files.next()?;
    files.next().is_none().then_some(first)
}

// ---------------------------------------------------------------------------
// The host side of the callback
// ---------------------------------------------------------------------------

/// What the host keeps for one instance, reached from the callback through the
/// effect's `resvd2`.
pub(crate) struct HostData {
    sample_rate: f32,
    block_size: i32,
    time: Mutex<VstTimeInfo>,
    /// Values the plugin reported (`audioMasterAutomate`): its window was turned.
    pub(crate) edits: Mutex<Vec<(u32, f64)>>,
    /// The size the plugin's window asked for.
    pub(crate) resize: Mutex<Option<(u32, u32)>>,
}

thread_local! {
    /// Whether this thread is in a process call (for audioMasterGetCurrentProcessLevel).
    static IN_PROCESS: Cell<bool> = const { Cell::new(false) };
    /// The sample rate of the instance being made, before its host data is set.
    static LOADING_RATE: Cell<f32> = const { Cell::new(44_100.0) };
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host's callback, which every plugin is given.
///
/// # Safety
/// Called by plugins with the arguments the interface defines for each opcode.
unsafe extern "C" fn host_callback(
    effect: *mut AEffect,
    opcode: i32,
    index: i32,
    value: isize,
    ptr: *mut c_void,
    opt: f32,
) -> isize {
    let data = if effect.is_null() || (*effect).resvd2 == 0 {
        None
    } else {
        Some(&*((*effect).resvd2 as *const HostData))
    };
    match opcode {
        audioMasterVersion => HOST_VST_VERSION,
        audioMasterCurrentId => 0,
        audioMasterAutomate => {
            if let Some(d) = data {
                // the audio thread may report too: never wait for the lock there
                if let Ok(mut e) = d.edits.try_lock() {
                    let id = index.max(0) as u32;
                    e.retain(|&(i, _)| i != id);
                    e.push((id, f64::from(opt)));
                }
            }
            0
        }
        audioMasterIdle
        | audioMasterUpdateDisplay
        | audioMasterBeginEdit
        | audioMasterEndEdit
        | audioMasterWantMidi => 1,
        audioMasterGetTime => match data {
            Some(d) => {
                // the info stays where it is for the life of the instance
                let t: *const VstTimeInfo = &*lock(&d.time);
                t as isize
            }
            None => 0,
        },
        audioMasterSizeWindow => {
            if let Some(d) = data {
                *lock(&d.resize) = Some((index.max(1) as u32, value.max(1) as u32));
            }
            1
        }
        audioMasterGetSampleRate => match data {
            Some(d) => d.sample_rate as isize,
            None => LOADING_RATE.with(Cell::get) as isize,
        },
        audioMasterGetBlockSize => data.map_or(MAX_FRAMES as isize, |d| d.block_size as isize),
        audioMasterGetInputLatency | audioMasterGetOutputLatency => 0,
        // 1: the app's thread, 2: the audio thread
        audioMasterGetCurrentProcessLevel => {
            if IN_PROCESS.with(Cell::get) {
                2
            } else {
                1
            }
        }
        // automation is read only by the host: "off"
        audioMasterGetAutomationState => 1,
        audioMasterGetVendorString => {
            write_c_text("Lumen", ptr.cast(), kVstMaxVendorStrLen);
            1
        }
        audioMasterGetProductString => {
            write_c_text("Lumen Audio Studio", ptr.cast(), kVstMaxProductStrLen);
            1
        }
        audioMasterGetVendorVersion => 1,
        // English
        audioMasterGetLanguage => 1,
        audioMasterCanDo => {
            if ptr.is_null() {
                return 0;
            }
            let what = std::ffi::CStr::from_ptr(ptr.cast()).to_string_lossy();
            match what.as_ref() {
                "sendVstEvents" | "sendVstMidiEvent" | "sendVstTimeInfo" | "sizeWindow"
                | "startStopProcess" | "supplyIdle" => 1,
                _ => 0,
            }
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// An instance
// ---------------------------------------------------------------------------

/// A started plugin: its library, its effect and the host's data for it.
pub(crate) struct Instance {
    effect: *mut AEffect,
    pub(crate) data: Box<HostData>,
    running: bool,
    // last: the library goes after the effect is closed
    _lib: libloading::Library,
}

fn load_library(bin: &Path) -> Result<libloading::Library, String> {
    // SAFETY: loading a plugin library runs its initialisation code; that is what a
    // host does
    #[cfg(windows)]
    let lib = unsafe {
        // the plugin's own folder is searched for the libraries it needs
        libloading::os::windows::Library::load_with_flags(
            bin,
            libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH,
        )
        .map(libloading::Library::from)
    };
    #[cfg(not(windows))]
    let lib = unsafe { libloading::Library::new(bin) };
    lib.map_err(|e| format!("the library did not load: {e}"))
}

impl Instance {
    /// Load the library at `path` and make an effect, opened (not yet running).
    pub(crate) fn open(path: &Path, sample_rate: f64) -> Result<Self, String> {
        let bin = binary(path).ok_or("no plugin binary in the bundle")?;
        let lib = load_library(&bin)?;
        // SAFETY: the entry point has the interface's signature
        let main = unsafe {
            lib.get::<PluginMain>(b"VSTPluginMain\0")
                .or_else(|_| lib.get::<PluginMain>(b"main_macho\0"))
                .or_else(|_| lib.get::<PluginMain>(b"main\0"))
                .map(|s| *s)
                .map_err(|_| "not a VST2 plugin (it has no VSTPluginMain)".to_string())?
        };
        LOADING_RATE.with(|r| r.set(sample_rate as f32));
        // SAFETY: the plugin's entry point, given the host's callback
        let effect = unsafe { main(host_callback) };
        if effect.is_null() {
            return Err("the plugin refused to start".into());
        }
        // SAFETY: the plugin returned an effect; its first field says whether it is one
        if unsafe { (*effect).magic } != kEffectMagic {
            return Err("not a VST2 plugin (a bad effect)".into());
        }
        let data = Box::new(HostData {
            sample_rate: sample_rate as f32,
            block_size: MAX_FRAMES as i32,
            time: Mutex::new(VstTimeInfo {
                sampleRate: sample_rate,
                tempo: 120.0,
                timeSigNumerator: 4,
                timeSigDenominator: 4,
                flags: kVstTempoValid | kVstTimeSigValid | kVstPpqPosValid,
                ..Default::default()
            }),
            edits: Mutex::new(Vec::new()),
            resize: Mutex::new(None),
        });
        // SAFETY: resvd2 is the host's to use
        unsafe { (*effect).resvd2 = &*data as *const HostData as isize };
        let me = Self {
            effect,
            data,
            running: false,
            _lib: lib,
        };
        if me.effect().dispatcher.is_none() {
            return Err("not a VST2 plugin (it has no dispatcher)".into());
        }
        me.dispatch(effOpen, 0, 0, std::ptr::null_mut(), 0.0);
        me.dispatch(
            effSetSampleRate,
            0,
            0,
            std::ptr::null_mut(),
            sample_rate as f32,
        );
        me.dispatch(
            effSetBlockSize,
            0,
            MAX_FRAMES as isize,
            std::ptr::null_mut(),
            0.0,
        );
        Ok(me)
    }

    pub(crate) fn effect(&self) -> &AEffect {
        // SAFETY: the effect lives until it is closed in `drop`
        unsafe { &*self.effect }
    }

    pub(crate) fn dispatch(
        &self,
        opcode: i32,
        index: i32,
        value: isize,
        ptr: *mut c_void,
        opt: f32,
    ) -> isize {
        match self.effect().dispatcher {
            // SAFETY: a call of the interface, on a live effect
            Some(d) => unsafe { d(self.effect, opcode, index, value, ptr, opt) },
            None => 0,
        }
    }

    /// Start processing (resume).
    pub(crate) fn start(&mut self) {
        if !self.running {
            self.dispatch(effMainsChanged, 0, 1, std::ptr::null_mut(), 0.0);
            self.dispatch(effStartProcess, 0, 0, std::ptr::null_mut(), 0.0);
            self.running = true;
        }
    }

    fn stop(&mut self) {
        if self.running {
            self.dispatch(effStopProcess, 0, 0, std::ptr::null_mut(), 0.0);
            self.dispatch(effMainsChanged, 0, 0, std::ptr::null_mut(), 0.0);
            self.running = false;
        }
    }

    /// A string the plugin writes into a buffer the host gives.
    pub(crate) fn text(&self, opcode: i32, index: i32) -> String {
        // much more room than the interface says: plugins write past its limits
        let mut buf = [0u8; 256];
        self.dispatch(opcode, index, 0, buf.as_mut_ptr().cast(), 0.0);
        c_text(&buf)
    }

    pub(crate) fn get_param(&self, index: i32) -> f32 {
        match self.effect().getParameter {
            // SAFETY: as `dispatch`
            Some(f) => unsafe { f(self.effect, index) },
            None => 0.0,
        }
    }

    pub(crate) fn set_param(&self, index: i32, value: f32) {
        if let Some(f) = self.effect().setParameter {
            // SAFETY: as `dispatch`
            unsafe { f(self.effect, index, value) }
        }
    }

    pub(crate) fn params(&self) -> Vec<Param> {
        (0..self.effect().numParams.clamp(0, 100_000))
            .map(|i| {
                let name = self.text(effGetParamName, i);
                Param {
                    id: i as u32,
                    name: if name.is_empty() {
                        format!("Parameter {}", i + 1)
                    } else {
                        name
                    },
                    min: 0.0,
                    max: 1.0,
                    default: f64::from(self.get_param(i)),
                }
            })
            .collect()
    }

    /// The plugin's state: its chunk when it has one, its parameter values otherwise.
    pub(crate) fn save_state(&self) -> Vec<u8> {
        let program = self.dispatch(effGetProgram, 0, 0, std::ptr::null_mut(), 0.0) as u32;
        let mut out = STATE_MAGIC.to_vec();
        if self.effect().flags & effFlagsProgramChunks != 0 {
            let mut data: *mut c_void = std::ptr::null_mut();
            // index 0: the whole bank
            let n = self.dispatch(
                effGetChunk,
                0,
                0,
                (&mut data as *mut *mut c_void).cast(),
                0.0,
            );
            if n > 0 && !data.is_null() {
                out.push(STATE_CHUNK);
                out.extend_from_slice(&program.to_le_bytes());
                // SAFETY: the plugin gave `n` bytes at `data`, kept until its next call
                let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), n as usize) };
                out.extend_from_slice(bytes);
                return out;
            }
        }
        out.push(STATE_PARAMS);
        out.extend_from_slice(&program.to_le_bytes());
        let n = self.effect().numParams.max(0);
        out.extend_from_slice(&(n as u32).to_le_bytes());
        for i in 0..n {
            out.extend_from_slice(&self.get_param(i).to_le_bytes());
        }
        out
    }

    /// Restore a state [`Instance::save_state`] made.
    pub(crate) fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let bad = || "not a state of this plugin".to_string();
        let rest = bytes.strip_prefix(STATE_MAGIC).ok_or_else(bad)?;
        let (&kind, rest) = rest.split_first().ok_or_else(bad)?;
        if rest.len() < 4 {
            return Err(bad());
        }
        let (program, rest) = rest.split_at(4);
        let program = u32::from_le_bytes(program.try_into().map_err(|_| bad())?);
        match kind {
            STATE_CHUNK => {
                self.dispatch(effBeginSetProgram, 0, 0, std::ptr::null_mut(), 0.0);
                self.dispatch(
                    effSetProgram,
                    0,
                    program as isize,
                    std::ptr::null_mut(),
                    0.0,
                );
                self.dispatch(effEndSetProgram, 0, 0, std::ptr::null_mut(), 0.0);
                let mut chunk = rest.to_vec();
                self.dispatch(
                    effSetChunk,
                    0,
                    chunk.len() as isize,
                    chunk.as_mut_ptr().cast(),
                    0.0,
                );
                Ok(())
            }
            STATE_PARAMS => {
                if rest.len() < 4 {
                    return Err(bad());
                }
                let (n, vals) = rest.split_at(4);
                let n = u32::from_le_bytes(n.try_into().map_err(|_| bad())?) as usize;
                if vals.len() != n * 4 {
                    return Err(bad());
                }
                self.dispatch(
                    effSetProgram,
                    0,
                    program as isize,
                    std::ptr::null_mut(),
                    0.0,
                );
                for (i, v) in vals.chunks(4).enumerate() {
                    let v = f32::from_le_bytes(v.try_into().map_err(|_| bad())?);
                    self.set_param(i as i32, v);
                }
                Ok(())
            }
            _ => Err(bad()),
        }
    }
}

const STATE_MAGIC: &[u8] = b"LVS2";
const STATE_PARAMS: u8 = 0;
const STATE_CHUNK: u8 = 1;

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop();
        self.dispatch(effClose, 0, 0, std::ptr::null_mut(), 0.0);
    }
}

/// The id a VST2 plugin goes by: its unique id and its file's name (several plugins
/// share an id, often 0).
fn plugin_id(unique: i32, path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    format!("vst2:{:08x}:{stem}", unique as u32)
}

/// What the VST2 library at `path` is. It is started and closed again to ask.
pub fn describe(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let inst = Instance::open(path, 44_100.0)?;
    let category = inst.dispatch(effGetPlugCategory, 0, 0, std::ptr::null_mut(), 0.0);
    if category == kPlugCategShell {
        return Err("a shell of several plugins, which is not supported".into());
    }
    let e = inst.effect();
    let synth = e.flags & effFlagsIsSynth != 0 || category == kPlugCategSynth;
    let mut name = inst.text(effGetEffectName, 0);
    if name.is_empty() {
        name = inst.text(effGetProductString, 0);
    }
    if name.is_empty() {
        name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    let mut features = Vec::new();
    if synth {
        features.push("instrument".to_string());
        features.push("synthesizer".to_string());
    }
    // an effect; or a synth with inputs (vocoders, the test plugin), which can be one
    if !synth || e.numInputs > 0 {
        features.push("audio-effect".to_string());
    }
    Ok(vec![PluginInfo {
        path: path.to_path_buf(),
        id: plugin_id(e.uniqueID, path),
        name,
        vendor: inst.text(effGetVendorString, 0),
        features,
        format: crate::Format::Vst2,
    }])
}

// ---------------------------------------------------------------------------
// A running plugin
// ---------------------------------------------------------------------------

/// A running VST2 plugin.
pub struct Vst2 {
    info: PluginInfo,
    params: Vec<Param>,
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    midi: Box<[VstMidiEvent; MAX_EVENTS]>,
    events: Box<VstEventsN<MAX_EVENTS>>,
    pending: Vec<(u32, f64)>,
    position: f64,
    // last: the plugin is closed after its buffers are no longer given to it
    inst: Instance,
}

// SAFETY: as `crate::Clap`: built on one thread, then used by the one thread holding it
unsafe impl Send for Vst2 {}

impl Vst2 {
    /// Load and start `info`'s plugin.
    pub fn load(info: &PluginInfo, sample_rate: f64) -> Result<Self, String> {
        let mut inst = Instance::open(&info.path, sample_rate)?;
        let e = inst.effect();
        if e.processReplacing.is_none() && e.process.is_none() {
            return Err("the plugin has no process function".into());
        }
        let (ni, no) = (
            e.numInputs.clamp(0, 64) as usize,
            e.numOutputs.clamp(0, 64) as usize,
        );
        let params = inst.params();
        inst.start();
        let mut inputs = vec![vec![0.0f32; MAX_FRAMES]; ni];
        let mut outputs = vec![vec![0.0f32; MAX_FRAMES]; no];
        let in_ptrs = inputs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let out_ptrs = outputs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        Ok(Self {
            info: info.clone(),
            params,
            inputs,
            outputs,
            in_ptrs,
            out_ptrs,
            midi: Box::new([VstMidiEvent::default(); MAX_EVENTS]),
            events: Box::new(VstEventsN {
                numEvents: 0,
                reserved: 0,
                events: [std::ptr::null_mut(); MAX_EVENTS],
            }),
            pending: Vec::with_capacity(64),
            position: 0.0,
            inst,
        })
    }

    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    pub fn params(&self) -> &[Param] {
        &self.params
    }

    pub fn param_value(&mut self, id: u32) -> Option<f64> {
        self.flush_params();
        (id < self.params.len() as u32).then(|| f64::from(self.inst.get_param(id as i32)))
    }

    /// Change a parameter; it takes effect at the start of the next block.
    pub fn set_param(&mut self, id: u32, value: f64) {
        self.pending.push((id, value));
    }

    pub fn takes_audio(&self) -> bool {
        !self.inputs.is_empty()
    }

    /// The plugin's own name for a parameter's value ("440 Hz").
    pub fn param_display(&self, id: u32) -> String {
        let v = self.inst.text(effGetParamDisplay, id as i32);
        let unit = self.inst.text(effGetParamLabel, id as i32);
        if unit.is_empty() {
            v
        } else {
            format!("{v} {unit}")
        }
    }

    pub fn save_state(&mut self) -> Option<Vec<u8>> {
        self.flush_params();
        Some(self.inst.save_state())
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.inst.load_state(bytes)
    }

    fn flush_params(&mut self) {
        for (id, v) in self.pending.drain(..) {
            if id < self.params.len() as u32 {
                self.inst.set_param(id as i32, v.clamp(0.0, 1.0) as f32);
            }
        }
    }

    /// The same contract as [`crate::Clap::process`].
    pub fn process(
        &mut self,
        input: Option<(&[f32], &[f32])>,
        notes: &[NoteMsg],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let total = out_l.len().min(out_r.len());
        let mut done = 0;
        while done < total {
            let n = (total - done).min(MAX_FRAMES);
            let range = done..done + n;
            let part = input.map(|(l, r)| (&l[range.clone()], &r[range.clone()]));
            self.block(part, notes, done as u32, n);
            match self.outputs.len() {
                0 => {
                    out_l[range.clone()].fill(0.0);
                    out_r[range].fill(0.0);
                }
                1 => {
                    out_l[range.clone()].copy_from_slice(&self.outputs[0][..n]);
                    out_r[range].copy_from_slice(&self.outputs[0][..n]);
                }
                _ => {
                    out_l[range.clone()].copy_from_slice(&self.outputs[0][..n]);
                    out_r[range].copy_from_slice(&self.outputs[1][..n]);
                }
            }
            done += n;
        }
    }

    fn block(&mut self, input: Option<(&[f32], &[f32])>, notes: &[NoteMsg], from: u32, n: usize) {
        IN_PROCESS.with(|p| p.set(true));
        self.flush_params();
        // the notes of the block, as MIDI
        let mut count = 0;
        for m in notes {
            let o = m.offset();
            if o < from || o >= from + n as u32 || count == MAX_EVENTS {
                continue;
            }
            let (status, key, vel) = match *m {
                NoteMsg::On { key, vel, .. } => (
                    0x90u8,
                    key,
                    (vel.clamp(0.0, 1.0) * 127.0).round().max(1.0) as u8,
                ),
                NoteMsg::Off { key, .. } => (0x80u8, key, 0),
            };
            self.midi[count] = VstMidiEvent {
                type_: kVstMidiType,
                byteSize: std::mem::size_of::<VstMidiEvent>() as i32,
                deltaFrames: (o - from) as i32,
                flags: kVstMidiEventIsRealtime,
                midiData: [status, key.min(127), vel, 0],
                ..Default::default()
            };
            count += 1;
        }
        if count > 0 {
            for i in 0..count {
                self.events.events[i] = (&mut self.midi[i] as *mut VstMidiEvent).cast();
            }
            self.events.numEvents = count as i32;
            let ptr = (&mut *self.events as *mut VstEventsN<MAX_EVENTS>).cast();
            self.inst.dispatch(effProcessEvents, 0, 0, ptr, 0.0);
        }
        // the input on the first two inputs
        for (i, b) in self.inputs.iter_mut().enumerate() {
            match (input, i) {
                (Some((l, _)), 0) => b[..n].copy_from_slice(&l[..n]),
                (Some((_, r)), 1) => b[..n].copy_from_slice(&r[..n]),
                _ => b[..n].fill(0.0),
            }
        }
        for b in &mut self.outputs {
            b[..n].fill(0.0);
        }
        if let Ok(mut t) = self.inst.data.time.try_lock() {
            t.samplePos = self.position;
            t.ppqPos = self.position / t.sampleRate.max(1.0) * t.tempo / 60.0;
        }
        let e = self.inst.effect;
        let f = self
            .inst
            .effect()
            .processReplacing
            .or(self.inst.effect().process);
        if let Some(f) = f {
            // SAFETY: as many channel pointers as the plugin has inputs and outputs, each
            // to at least `n` frames, which stay put during the call
            unsafe {
                f(
                    e,
                    self.in_ptrs.as_mut_ptr(),
                    self.out_ptrs.as_mut_ptr(),
                    n as i32,
                )
            };
        }
        self.position += n as f64;
        IN_PROCESS.with(|p| p.set(false));
    }
}

#[cfg(test)]
mod tests {
    use super::abi::*;
    use std::mem::{offset_of, size_of};

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_binary_interface_has_the_layout_of_vst_2_4() {
        assert_eq!(size_of::<AEffect>(), 192);
        assert_eq!(offset_of!(AEffect, magic), 0);
        assert_eq!(offset_of!(AEffect, dispatcher), 8);
        assert_eq!(offset_of!(AEffect, process), 16);
        assert_eq!(offset_of!(AEffect, setParameter), 24);
        assert_eq!(offset_of!(AEffect, getParameter), 32);
        assert_eq!(offset_of!(AEffect, numPrograms), 40);
        assert_eq!(offset_of!(AEffect, numParams), 44);
        assert_eq!(offset_of!(AEffect, numInputs), 48);
        assert_eq!(offset_of!(AEffect, numOutputs), 52);
        assert_eq!(offset_of!(AEffect, flags), 56);
        assert_eq!(offset_of!(AEffect, resvd1), 64);
        assert_eq!(offset_of!(AEffect, resvd2), 72);
        assert_eq!(offset_of!(AEffect, initialDelay), 80);
        assert_eq!(offset_of!(AEffect, ioRatio), 92);
        assert_eq!(offset_of!(AEffect, object), 96);
        assert_eq!(offset_of!(AEffect, user), 104);
        assert_eq!(offset_of!(AEffect, uniqueID), 112);
        assert_eq!(offset_of!(AEffect, version), 116);
        assert_eq!(offset_of!(AEffect, processReplacing), 120);
        assert_eq!(offset_of!(AEffect, processDoubleReplacing), 128);
        assert_eq!(offset_of!(AEffect, future), 136);

        assert_eq!(size_of::<VstEvent>(), 32);
        assert_eq!(size_of::<VstMidiEvent>(), 32);
        assert_eq!(offset_of!(VstMidiEvent, deltaFrames), 8);
        assert_eq!(offset_of!(VstMidiEvent, midiData), 24);
        assert_eq!(offset_of!(VstMidiEvent, detune), 28);
        assert_eq!(offset_of!(VstEvents, numEvents), 0);
        assert_eq!(offset_of!(VstEvents, events), 16);
        assert_eq!(size_of::<VstEvents>(), 32);
        assert_eq!(offset_of!(VstEventsN<8>, events), 16);

        assert_eq!(size_of::<ERect>(), 8);
        assert_eq!(size_of::<VstTimeInfo>(), 88);
        assert_eq!(offset_of!(VstTimeInfo, tempo), 32);
        assert_eq!(offset_of!(VstTimeInfo, timeSigNumerator), 64);
        assert_eq!(offset_of!(VstTimeInfo, flags), 84);
        assert_eq!(kEffectMagic, 0x5673_7450);
    }

    #[test]
    fn plugin_text_is_read_up_to_its_end() {
        assert_eq!(c_text(b"Gain\0junk"), "Gain");
        let mut buf = [1u8; 4];
        // SAFETY: four bytes
        unsafe { write_c_text("Sylenth1", buf.as_mut_ptr(), 4) };
        assert_eq!(&buf, b"Syl\0");
    }
}
