// SPDX-License-Identifier: GPL-3.0-or-later
//! The VST 2.4 binary interface, written from its published layout (the way the `vst`
//! crate did it): the plugin's `AEffect`, the opcodes the host and the plugin send each
//! other, MIDI events and the time info. No Steinberg header is used. The layout is the
//! natural C layout of the target (the SDK packs to 8 bytes on 64-bit systems, which is
//! the same thing); the tests check the sizes and offsets.
//!
//! This file has no dependencies, so the test plugin (`las-testvst2`) uses it as well.
#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]

use std::ffi::c_void;

/// `'VstP'`, the first field of every `AEffect`.
pub const kEffectMagic: i32 = i32::from_be_bytes(*b"VstP");

/// The host's callback, and the plugin's dispatcher: an opcode, two integers, a pointer
/// and a float in, an integer out.
pub type Callback = unsafe extern "C" fn(
    effect: *mut AEffect,
    opcode: i32,
    index: i32,
    value: isize,
    ptr: *mut c_void,
    opt: f32,
) -> isize;
pub type Dispatcher = Callback;
pub type ProcessFn = unsafe extern "C" fn(
    effect: *mut AEffect,
    inputs: *mut *mut f32,
    outputs: *mut *mut f32,
    frames: i32,
);
pub type ProcessDoubleFn = unsafe extern "C" fn(
    effect: *mut AEffect,
    inputs: *mut *mut f64,
    outputs: *mut *mut f64,
    frames: i32,
);
pub type SetParameterFn = unsafe extern "C" fn(effect: *mut AEffect, index: i32, value: f32);
pub type GetParameterFn = unsafe extern "C" fn(effect: *mut AEffect, index: i32) -> f32;
/// What a plugin library exports as `VSTPluginMain` (or `main` in old ones).
pub type PluginMain = unsafe extern "C" fn(host: Callback) -> *mut AEffect;

/// A plugin instance as the host sees it.
#[repr(C)]
pub struct AEffect {
    pub magic: i32,
    pub dispatcher: Option<Dispatcher>,
    /// The old accumulating process call.
    pub process: Option<ProcessFn>,
    pub setParameter: Option<SetParameterFn>,
    pub getParameter: Option<GetParameterFn>,
    pub numPrograms: i32,
    pub numParams: i32,
    pub numInputs: i32,
    pub numOutputs: i32,
    pub flags: i32,
    /// For the host.
    pub resvd1: isize,
    /// For the host (this host keeps its data here).
    pub resvd2: isize,
    pub initialDelay: i32,
    pub realQualities: i32,
    pub offQualities: i32,
    pub ioRatio: f32,
    /// The plugin's own object.
    pub object: *mut c_void,
    pub user: *mut c_void,
    pub uniqueID: i32,
    pub version: i32,
    pub processReplacing: Option<ProcessFn>,
    pub processDoubleReplacing: Option<ProcessDoubleFn>,
    pub future: [u8; 56],
}

// AEffect flags
pub const effFlagsHasEditor: i32 = 1 << 0;
pub const effFlagsCanReplacing: i32 = 1 << 4;
pub const effFlagsProgramChunks: i32 = 1 << 5;
pub const effFlagsIsSynth: i32 = 1 << 8;
pub const effFlagsNoSoundInStop: i32 = 1 << 9;
pub const effFlagsCanDoubleReplacing: i32 = 1 << 12;

// What the host asks the plugin (the dispatcher's opcodes)
pub const effOpen: i32 = 0;
pub const effClose: i32 = 1;
pub const effSetProgram: i32 = 2;
pub const effGetProgram: i32 = 3;
pub const effSetProgramName: i32 = 4;
pub const effGetProgramName: i32 = 5;
pub const effGetParamLabel: i32 = 6;
pub const effGetParamDisplay: i32 = 7;
pub const effGetParamName: i32 = 8;
pub const effSetSampleRate: i32 = 10;
pub const effSetBlockSize: i32 = 11;
pub const effMainsChanged: i32 = 12;
pub const effEditGetRect: i32 = 13;
pub const effEditOpen: i32 = 14;
pub const effEditClose: i32 = 15;
pub const effEditIdle: i32 = 19;
pub const effGetChunk: i32 = 23;
pub const effSetChunk: i32 = 24;
pub const effProcessEvents: i32 = 25;
pub const effCanBeAutomated: i32 = 26;
pub const effGetProgramNameIndexed: i32 = 29;
pub const effGetPlugCategory: i32 = 35;
pub const effGetEffectName: i32 = 45;
pub const effGetVendorString: i32 = 47;
pub const effGetProductString: i32 = 48;
pub const effGetVendorVersion: i32 = 49;
pub const effCanDo: i32 = 51;
pub const effGetVstVersion: i32 = 58;
pub const effBeginSetProgram: i32 = 67;
pub const effEndSetProgram: i32 = 68;
pub const effShellGetNextPlugin: i32 = 70;
pub const effStartProcess: i32 = 71;
pub const effStopProcess: i32 = 72;

// What the plugin asks the host (the callback's opcodes)
pub const audioMasterAutomate: i32 = 0;
pub const audioMasterVersion: i32 = 1;
pub const audioMasterCurrentId: i32 = 2;
pub const audioMasterIdle: i32 = 3;
pub const audioMasterWantMidi: i32 = 6;
pub const audioMasterGetTime: i32 = 7;
pub const audioMasterProcessEvents: i32 = 8;
pub const audioMasterIOChanged: i32 = 13;
pub const audioMasterSizeWindow: i32 = 15;
pub const audioMasterGetSampleRate: i32 = 16;
pub const audioMasterGetBlockSize: i32 = 17;
pub const audioMasterGetInputLatency: i32 = 18;
pub const audioMasterGetOutputLatency: i32 = 19;
pub const audioMasterGetCurrentProcessLevel: i32 = 23;
pub const audioMasterGetAutomationState: i32 = 24;
pub const audioMasterGetVendorString: i32 = 32;
pub const audioMasterGetProductString: i32 = 33;
pub const audioMasterGetVendorVersion: i32 = 34;
pub const audioMasterVendorSpecific: i32 = 35;
pub const audioMasterCanDo: i32 = 37;
pub const audioMasterGetLanguage: i32 = 38;
pub const audioMasterGetDirectory: i32 = 41;
pub const audioMasterUpdateDisplay: i32 = 42;
pub const audioMasterBeginEdit: i32 = 43;
pub const audioMasterEndEdit: i32 = 44;

// Plugin categories (effGetPlugCategory)
pub const kPlugCategUnknown: isize = 0;
pub const kPlugCategEffect: isize = 1;
pub const kPlugCategSynth: isize = 2;
pub const kPlugCategAnalysis: isize = 3;
pub const kPlugCategMastering: isize = 4;
pub const kPlugCategShell: isize = 10;
pub const kPlugCategGenerator: isize = 11;

/// String lengths the interface names (plugins often write more: hosts give more room).
pub const kVstMaxProgNameLen: usize = 24;
pub const kVstMaxParamStrLen: usize = 8;
pub const kVstMaxEffectNameLen: usize = 32;
pub const kVstMaxVendorStrLen: usize = 64;
pub const kVstMaxProductStrLen: usize = 64;

/// The window rectangle `effEditGetRect` gives.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ERect {
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
    pub right: i16,
}

pub const kVstMidiType: i32 = 1;
pub const kVstSysExType: i32 = 6;
pub const kVstMidiEventIsRealtime: i32 = 1;

/// Any event (its size is that of every event kind).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VstEvent {
    pub type_: i32,
    pub byteSize: i32,
    pub deltaFrames: i32,
    pub flags: i32,
    pub data: [u8; 16],
}

/// A MIDI message.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VstMidiEvent {
    pub type_: i32,
    pub byteSize: i32,
    pub deltaFrames: i32,
    pub flags: i32,
    pub noteLength: i32,
    pub noteOffset: i32,
    pub midiData: [u8; 4],
    pub detune: i8,
    pub noteOffVelocity: u8,
    pub reserved1: u8,
    pub reserved2: u8,
}

/// The head of a list of events: the interface declares the array as two long and the
/// host gives as many as there are, so a list is this followed by more pointers. See
/// [`VstEventsN`].
#[repr(C)]
pub struct VstEvents {
    pub numEvents: i32,
    pub reserved: isize,
    pub events: [*mut VstEvent; 2],
}

/// A list of up to `N` events, laid out as [`VstEvents`] with a longer array.
#[repr(C)]
pub struct VstEventsN<const N: usize> {
    pub numEvents: i32,
    pub reserved: isize,
    pub events: [*mut VstEvent; N],
}

// VstTimeInfo flags
pub const kVstTransportChanged: i32 = 1;
pub const kVstTransportPlaying: i32 = 1 << 1;
pub const kVstNanosValid: i32 = 1 << 8;
pub const kVstPpqPosValid: i32 = 1 << 9;
pub const kVstTempoValid: i32 = 1 << 10;
pub const kVstBarsValid: i32 = 1 << 11;
pub const kVstTimeSigValid: i32 = 1 << 13;

/// Where the host's transport is (`audioMasterGetTime`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VstTimeInfo {
    pub samplePos: f64,
    pub sampleRate: f64,
    pub nanoSeconds: f64,
    pub ppqPos: f64,
    pub tempo: f64,
    pub barStartPos: f64,
    pub cycleStartPos: f64,
    pub cycleEndPos: f64,
    pub timeSigNumerator: i32,
    pub timeSigDenominator: i32,
    pub smpteOffset: i32,
    pub smpteFrameRate: i32,
    pub samplesToNextClock: i32,
    pub flags: i32,
}

/// The VST version this host speaks.
pub const HOST_VST_VERSION: isize = 2400;

/// Copy a C string a plugin wrote (up to its first zero) into a `String`.
pub fn c_text(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}

/// Write `s` into a C string buffer of `len` bytes (ending it with a zero).
///
/// # Safety
/// `dst` must point to at least `len` writable bytes.
pub unsafe fn write_c_text(s: &str, dst: *mut u8, len: usize) {
    if dst.is_null() || len == 0 {
        return;
    }
    let n = s.len().min(len - 1);
    std::ptr::copy_nonoverlapping(s.as_ptr(), dst, n);
    *dst.add(n) = 0;
}
