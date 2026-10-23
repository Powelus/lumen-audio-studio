// SPDX-License-Identifier: GPL-3.0-or-later
//! A small VST2 plugin to test the host with, written on the host's own binary
//! interface (`las-host/src/vst2/abi.rs`, shared by path). Like `las-testvst3`: as an
//! effect it multiplies its input by a gain parameter, and as an instrument it plays a
//! sine at the pitch of the note held (both, added). One parameter, a state chunk, and
//! a window that reports edits: one when it opens and one (with a new size) on its
//! third idle call.
#![allow(clippy::missing_safety_doc, non_upper_case_globals, non_snake_case)]

#[path = "../../las-host/src/vst2/abi.rs"]
pub mod abi;

use abi::*;
use std::ffi::c_void;

/// The index of the one parameter.
pub const GAIN: i32 = 0;
/// The gain is this times the normalized value of the parameter.
pub const GAIN_RANGE: f32 = 2.0;
pub const NAME: &str = "Lumen Test VST2";
pub const VENDOR: &str = "Lumen Audio Studio";
pub const UNIQUE_ID: i32 = i32::from_be_bytes(*b"LTv2");
/// What the window reports when it opens, and on its third idle call.
pub const EDIT_ON_OPEN: f32 = 0.25;
pub const EDIT_ON_IDLE: f32 = 0.75;
/// The window's size, and the size it asks for on its third idle call.
pub const WINDOW: (i16, i16) = (300, 200);
pub const RESIZED: (i32, i32) = (320, 240);
/// The first bytes of the chunk.
const CHUNK_MAGIC: &[u8; 4] = b"TV2C";

#[repr(C)]
struct Plugin {
    // first: the host's pointer to the effect is a pointer to the plugin
    effect: AEffect,
    host: Callback,
    gain: f32,
    sr: f64,
    phase: f64,
    key: Option<u8>,
    /// Notes of the block yet to start: (frame, key or none for an off).
    events: Vec<(i32, u8, bool)>,
    rect: ERect,
    idles: u32,
    chunk: Vec<u8>,
    program: isize,
}

unsafe fn me<'a>(e: *mut AEffect) -> &'a mut Plugin {
    &mut *(e as *mut Plugin)
}

unsafe extern "C" fn dispatcher(
    e: *mut AEffect,
    opcode: i32,
    index: i32,
    value: isize,
    ptr: *mut c_void,
    opt: f32,
) -> isize {
    if opcode == effClose {
        drop(Box::from_raw(e as *mut Plugin));
        return 1;
    }
    let p = me(e);
    match opcode {
        effSetSampleRate => {
            p.sr = f64::from(opt);
            1
        }
        effGetParamName if index == GAIN => {
            write_c_text("Gain", ptr.cast(), kVstMaxParamStrLen);
            1
        }
        effGetParamLabel => {
            write_c_text("x", ptr.cast(), kVstMaxParamStrLen);
            1
        }
        effGetParamDisplay => {
            write_c_text(
                &format!("{:.2}", p.gain * GAIN_RANGE),
                ptr.cast(),
                kVstMaxParamStrLen,
            );
            1
        }
        effGetEffectName => {
            write_c_text(NAME, ptr.cast(), kVstMaxEffectNameLen);
            1
        }
        effGetVendorString => {
            write_c_text(VENDOR, ptr.cast(), kVstMaxVendorStrLen);
            1
        }
        effGetProductString => {
            write_c_text(NAME, ptr.cast(), kVstMaxProductStrLen);
            1
        }
        effGetPlugCategory => kPlugCategSynth,
        effGetVstVersion => 2400,
        effSetProgram => {
            p.program = value;
            1
        }
        effGetProgram => p.program,
        effGetChunk => {
            p.chunk = CHUNK_MAGIC.to_vec();
            p.chunk.extend_from_slice(&p.gain.to_le_bytes());
            *(ptr as *mut *mut c_void) = p.chunk.as_mut_ptr().cast();
            p.chunk.len() as isize
        }
        effSetChunk => {
            let bytes = std::slice::from_raw_parts(ptr as *const u8, value.max(0) as usize);
            if bytes.len() == 8 && bytes.starts_with(CHUNK_MAGIC) {
                p.gain = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
                1
            } else {
                0
            }
        }
        effProcessEvents => {
            let list = &*(ptr as *const VstEvents);
            let first = std::ptr::addr_of!(list.events) as *const *const VstEvent;
            for i in 0..list.numEvents.max(0) as usize {
                let ev = *first.add(i);
                if (*ev).type_ != kVstMidiType {
                    continue;
                }
                let m = &*(ev as *const VstMidiEvent);
                let on = m.midiData[0] & 0xf0 == 0x90 && m.midiData[2] > 0;
                let off = m.midiData[0] & 0xf0 == 0x80 || m.midiData[0] & 0xf0 == 0x90;
                if on || off {
                    p.events.push((m.deltaFrames, m.midiData[1], on));
                }
            }
            1
        }
        effCanBeAutomated => 1,
        effCanDo => {
            let what = std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char);
            match what.to_bytes() {
                b"receiveVstEvents" | b"receiveVstMidiEvent" => 1,
                _ => -1,
            }
        }
        effEditGetRect => {
            *(ptr as *mut *mut ERect) = &mut p.rect;
            1
        }
        effEditOpen => {
            if ptr.is_null() {
                return 0;
            }
            (p.host)(
                e,
                audioMasterAutomate,
                GAIN,
                0,
                std::ptr::null_mut(),
                EDIT_ON_OPEN,
            );
            p.gain = EDIT_ON_OPEN;
            1
        }
        effEditIdle => {
            p.idles += 1;
            if p.idles == 3 {
                p.gain = EDIT_ON_IDLE;
                (p.host)(
                    e,
                    audioMasterAutomate,
                    GAIN,
                    0,
                    std::ptr::null_mut(),
                    EDIT_ON_IDLE,
                );
                (p.host)(
                    e,
                    audioMasterSizeWindow,
                    RESIZED.0,
                    RESIZED.1 as isize,
                    std::ptr::null_mut(),
                    0.0,
                );
            }
            1
        }
        _ => 0,
    }
}

unsafe extern "C" fn set_parameter(e: *mut AEffect, index: i32, value: f32) {
    if index == GAIN {
        me(e).gain = value.clamp(0.0, 1.0);
    }
}

unsafe extern "C" fn get_parameter(e: *mut AEffect, index: i32) -> f32 {
    if index == GAIN {
        me(e).gain
    } else {
        0.0
    }
}

unsafe extern "C" fn process_replacing(
    e: *mut AEffect,
    inputs: *mut *mut f32,
    outputs: *mut *mut f32,
    frames: i32,
) {
    let p = me(e);
    let n = frames.max(0) as usize;
    let (il, ir) = (
        std::slice::from_raw_parts(*inputs, n),
        std::slice::from_raw_parts(*inputs.add(1), n),
    );
    let (ol, or) = (
        std::slice::from_raw_parts_mut(*outputs, n),
        std::slice::from_raw_parts_mut(*outputs.add(1), n),
    );
    let gain = p.gain * GAIN_RANGE;
    let mut events = std::mem::take(&mut p.events);
    events.sort_by_key(|e| e.0);
    let mut next = 0;
    for i in 0..n {
        while next < events.len() && events[next].0 as usize <= i {
            let (_, key, on) = events[next];
            if on {
                p.key = Some(key);
                p.phase = 0.0;
            } else if p.key == Some(key) {
                p.key = None;
            }
            next += 1;
        }
        let tone = match p.key {
            Some(k) => {
                let f = 440.0 * 2f64.powf((f64::from(k) - 69.0) / 12.0);
                let s = (p.phase * std::f64::consts::TAU).sin() as f32 * 0.5;
                p.phase = (p.phase + f / p.sr.max(1.0)).fract();
                s
            }
            None => 0.0,
        };
        ol[i] = il[i] * gain + tone;
        or[i] = ir[i] * gain + tone;
    }
}

/// The entry point a host looks for.
///
/// # Safety
/// Called by a host with its callback.
#[no_mangle]
pub unsafe extern "C" fn VSTPluginMain(host: Callback) -> *mut AEffect {
    // a host that does not speak VST 2.4 is still served
    let _version = host(
        std::ptr::null_mut(),
        audioMasterVersion,
        0,
        0,
        std::ptr::null_mut(),
        0.0,
    );
    let plugin = Box::new(Plugin {
        effect: AEffect {
            magic: kEffectMagic,
            dispatcher: Some(dispatcher),
            process: None,
            setParameter: Some(set_parameter),
            getParameter: Some(get_parameter),
            numPrograms: 1,
            numParams: 1,
            numInputs: 2,
            numOutputs: 2,
            flags: effFlagsHasEditor
                | effFlagsCanReplacing
                | effFlagsProgramChunks
                | effFlagsIsSynth,
            resvd1: 0,
            resvd2: 0,
            initialDelay: 0,
            realQualities: 0,
            offQualities: 0,
            ioRatio: 1.0,
            object: std::ptr::null_mut(),
            user: std::ptr::null_mut(),
            uniqueID: UNIQUE_ID,
            version: 1,
            processReplacing: Some(process_replacing),
            processDoubleReplacing: None,
            future: [0; 56],
        },
        host,
        gain: 0.5,
        sr: 44_100.0,
        phase: 0.0,
        key: None,
        events: Vec::with_capacity(64),
        rect: ERect {
            top: 0,
            left: 0,
            bottom: WINDOW.1,
            right: WINDOW.0,
        },
        idles: 0,
        chunk: Vec::new(),
        program: 0,
    });
    let p = Box::into_raw(plugin);
    (*p).effect.object = p.cast();
    p.cast()
}
