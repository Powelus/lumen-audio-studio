// SPDX-License-Identifier: GPL-3.0-or-later
//! A small VST3 plugin to test the host with. As an effect it multiplies its input by a
//! gain parameter; as an instrument it plays a sine at the pitch of the note that is
//! held. Audio in and out, a note input, one parameter and state. Built on the
//! structure of the `gain` example of the `vst3` crate.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
#![allow(unsafe_op_in_unsafe_fn)]
// the enum types of the bindings differ by platform, so some casts only look needless
#![allow(clippy::unnecessary_cast)]

use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::{ptr, slice};
use vst3::{uid, Class, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

/// The id of the one parameter.
pub const GAIN: u32 = 1;
/// The gain is this times the normalized value of the parameter.
pub const GAIN_RANGE: f64 = 2.0;
const NAME: &str = "Lumen Test VST3";

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let c = CString::new(src).unwrap_or_default();
    let bytes = c.as_bytes_with_nul();
    for (s, d) in bytes.iter().zip(dst.iter_mut()) {
        *d = *s as c_char;
    }
    if bytes.len() > dst.len() {
        if let Some(last) = dst.last_mut() {
            *last = 0;
        }
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (s, d) in src.encode_utf16().zip(dst.iter_mut()) {
        *d = s as TChar;
        len += 1;
    }
    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

#[derive(Default)]
struct Voice {
    sr: f64,
    phase: f64,
    key: Option<i16>,
}

struct Processor {
    /// The normalized gain parameter, as the bits of an `f64`.
    gain: AtomicU64,
    voice: Mutex<Voice>,
}

impl Class for Processor {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl Processor {
    const CID: TUID = uid(0x4C554D31, 0x56535433, 0x50524F43, 0x00000001);

    fn new() -> Self {
        Self {
            gain: AtomicU64::new(0.5f64.to_bits()),
            voice: Mutex::new(Voice::default()),
        }
    }

    fn norm(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

impl IPluginBaseTrait for Processor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Processor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        *class_id = Controller::CID;
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        match (media as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, _) => 1,
            (MediaTypes_::kEvent, BusDirections_::kInput) => 1,
            _ => 0,
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let bus = &mut *bus;
        match (media as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, d)
                if d == BusDirections_::kInput || d == BusDirections_::kOutput =>
            {
                bus.mediaType = MediaTypes_::kAudio as MediaType;
                bus.direction = dir;
                bus.channelCount = 2;
                copy_wstring(
                    if d == BusDirections_::kInput {
                        "Input"
                    } else {
                        "Output"
                    },
                    &mut bus.name,
                );
                bus.busType = BusTypes_::kMain as BusType;
                bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
                kResultOk
            }
            (MediaTypes_::kEvent, BusDirections_::kInput) => {
                bus.mediaType = MediaTypes_::kEvent as MediaType;
                bus.direction = BusDirections_::kInput as BusDirection;
                bus.channelCount = 1;
                copy_wstring("Notes", &mut bus.name);
                bus.busType = BusTypes_::kMain as BusType;
                bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(stream) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut bytes = [0u8; 8];
        let mut read = 0;
        let r = stream.read(bytes.as_mut_ptr() as *mut c_void, 8, &mut read);
        if r != kResultOk || read != 8 {
            return kResultFalse;
        }
        self.gain
            .store(f64::from_le_bytes(bytes).to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(stream) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let bytes = self.norm().to_le_bytes();
        let mut written = 0;
        stream.write(bytes.as_ptr() as *mut c_void, 8, &mut written)
    }
}

impl IAudioProcessorTrait for Processor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        if num_ins != 1 || num_outs != 1 {
            return kResultFalse;
        }
        if *inputs != SpeakerArr::kStereo || *outputs != SpeakerArr::kStereo {
            return kResultFalse;
        }
        kResultTrue
    }
    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0
            || (dir as BusDirections != BusDirections_::kInput
                && dir as BusDirections != BusDirections_::kOutput)
        {
            return kInvalidArgument;
        }
        *arr = SpeakerArr::kStereo;
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size as SymbolicSampleSizes == SymbolicSampleSizes_::kSample32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        self.voice.lock().unwrap().sr = (*setup).sampleRate;
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = &*data;
        if let Some(changes) = ComRef::from_raw(d.inputParameterChanges) {
            for i in 0..changes.getParameterCount() {
                let Some(queue) = ComRef::from_raw(changes.getParameterData(i)) else {
                    continue;
                };
                let n = queue.getPointCount();
                if queue.getParameterId() == GAIN && n > 0 {
                    let (mut offset, mut value) = (0, 0.0);
                    if queue.getPoint(n - 1, &mut offset, &mut value) == kResultTrue {
                        self.gain.store(value.to_bits(), Ordering::Relaxed);
                    }
                }
            }
        }
        let frames = d.numSamples as usize;
        if d.numOutputs != 1 {
            return kResultOk;
        }
        let out = slice::from_raw_parts(d.outputs, 1)[0];
        let out_ch = slice::from_raw_parts(out.__field0.channelBuffers32, out.numChannels as usize);
        let inp = if d.numInputs == 1 {
            let b = slice::from_raw_parts(d.inputs, 1)[0];
            Some(slice::from_raw_parts(
                b.__field0.channelBuffers32,
                b.numChannels as usize,
            ))
        } else {
            None
        };
        // events cut the block into stretches
        let mut cuts: Vec<(usize, Event)> = Vec::new();
        if let Some(events) = ComRef::from_raw(d.inputEvents) {
            for i in 0..events.getEventCount() {
                let mut e: Event = std::mem::zeroed();
                if events.getEvent(i, &mut e) == kResultOk {
                    cuts.push((e.sampleOffset.max(0) as usize, e));
                }
            }
        }
        cuts.sort_by_key(|c| c.0);
        let mut voice = self.voice.lock().unwrap();
        let gain = (self.norm() * GAIN_RANGE) as f32;
        let mut at = 0;
        let render = |voice: &mut Voice, from: usize, to: usize| {
            let step = voice
                .key
                .map(|k| 440.0 * 2f64.powf((f64::from(k) - 69.0) / 12.0) / voice.sr.max(1.0));
            let start = voice.phase;
            let mut end = start;
            for (c, o) in out_ch.iter().enumerate() {
                let o = slice::from_raw_parts_mut(*o, frames);
                let mut ph = start;
                for (f, out) in o.iter_mut().enumerate().take(to).skip(from) {
                    let tone = match step {
                        Some(s) => {
                            let v = (ph * std::f64::consts::TAU).sin() as f32 * 0.5;
                            ph = (ph + s).fract();
                            v
                        }
                        None => 0.0,
                    };
                    let x = inp.and_then(|i| i.get(c)).map_or(0.0, |i| *i.add(f));
                    *out = (x + tone) * gain;
                }
                end = ph;
            }
            voice.phase = end;
        };
        for (time, e) in cuts {
            let time = time.min(frames);
            if time > at {
                render(&mut voice, at, time);
                at = time;
            }
            match e.r#type as Event_::EventTypes {
                Event_::EventTypes_::kNoteOnEvent => voice.key = Some(e.__field0.noteOn.pitch),
                Event_::EventTypes_::kNoteOffEvent
                    if voice.key == Some(e.__field0.noteOff.pitch) =>
                {
                    voice.key = None;
                }
                _ => {}
            }
        }
        render(&mut voice, at, frames);
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IProcessContextRequirementsTrait for Processor {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        0
    }
}

struct Controller {
    gain: Mutex<f64>,
    handler: Mutex<Option<vst3::ComPtr<IComponentHandler>>>,
}

impl Class for Controller {
    type Interfaces = (IEditController,);
}

impl Controller {
    const CID: TUID = uid(0x4C554D31, 0x56535433, 0x434F4E54, 0x00000002);
}

impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let Some(stream) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut bytes = [0u8; 8];
        let mut read = 0;
        if stream.read(bytes.as_mut_ptr() as *mut c_void, 8, &mut read) != kResultOk || read != 8 {
            return kResultFalse;
        }
        *self.gain.lock().unwrap() = f64::from_le_bytes(bytes);
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        1
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.id = GAIN;
        copy_wstring("Gain", &mut info.title);
        copy_wstring("Gain", &mut info.shortTitle);
        copy_wstring("x", &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = 0.5;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, value: f64, string: *mut String128) -> tresult {
        if id != GAIN {
            return kInvalidArgument;
        }
        copy_wstring(&format!("{:.2}", value * GAIN_RANGE), &mut *string);
        kResultOk
    }
    unsafe fn getParamValueByString(
        &self,
        _id: u32,
        _string: *mut TChar,
        _value: *mut f64,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, id: u32, v: f64) -> f64 {
        if id == GAIN {
            v * GAIN_RANGE
        } else {
            0.0
        }
    }
    unsafe fn plainParamToNormalized(&self, id: u32, v: f64) -> f64 {
        if id == GAIN {
            v / GAIN_RANGE
        } else {
            0.0
        }
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        if id == GAIN {
            *self.gain.lock().unwrap()
        } else {
            0.0
        }
    }
    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> tresult {
        if id != GAIN {
            return kInvalidArgument;
        }
        *self.gain.lock().unwrap() = value;
        kResultOk
    }
    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        *self.handler.lock().unwrap() = ComRef::from_raw(handler).map(|h| h.to_com_ptr());
        kResultOk
    }
    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        let view = ComWrapper::new(View {
            handler: self.handler.lock().unwrap().clone(),
            #[cfg(target_os = "linux")]
            timer: Mutex::new(None),
        });
        view.to_com_ptr::<IPlugView>()
            .map_or(ptr::null_mut(), |v| v.into_raw())
    }
}

/// The view: it draws nothing. When it is attached it sets the gain to 0.25 as if
/// the user had turned it; on Linux it also asks the host's run loop for a timer, which
/// sets the gain to 0.75 and asks for a bigger window.
pub const EDIT_ON_OPEN: f64 = 0.25;
pub const EDIT_ON_TIMER: f64 = 0.75;
pub const VIEW_SIZE: (i32, i32) = (300, 200);
pub const RESIZED: (i32, i32) = (320, 240);

struct View {
    handler: Option<vst3::ComPtr<IComponentHandler>>,
    #[cfg(target_os = "linux")]
    timer: Mutex<Option<(vst3::ComPtr<Linux::IRunLoop>, ComWrapper<Tick>)>>,
}

impl Class for View {
    type Interfaces = (IPlugView,);
}

impl View {
    unsafe fn edit(handler: &Option<vst3::ComPtr<IComponentHandler>>, v: f64) {
        if let Some(h) = handler {
            h.beginEdit(GAIN);
            h.performEdit(GAIN, v);
            h.endEdit(GAIN);
        }
    }
}

#[cfg(target_os = "linux")]
struct Tick {
    handler: Option<vst3::ComPtr<IComponentHandler>>,
    frame: vst3::ComPtr<IPlugFrame>,
    done: std::sync::atomic::AtomicBool,
}

#[cfg(target_os = "linux")]
impl Class for Tick {
    type Interfaces = (Linux::ITimerHandler,);
}

#[cfg(target_os = "linux")]
impl Linux::ITimerHandlerTrait for Tick {
    unsafe fn onTimer(&self) {
        if self.done.swap(true, Ordering::Relaxed) {
            return;
        }
        View::edit(&self.handler, EDIT_ON_TIMER);
        let mut r = ViewRect {
            left: 0,
            top: 0,
            right: RESIZED.0,
            bottom: RESIZED.1,
        };
        self.frame.resizeView(ptr::null_mut(), &mut r);
    }
}

impl IPlugViewTrait for View {
    unsafe fn isPlatformTypeSupported(&self, _kind: FIDString) -> tresult {
        kResultTrue
    }
    unsafe fn attached(&self, parent: *mut c_void, _kind: FIDString) -> tresult {
        if parent.is_null() {
            return kInvalidArgument;
        }
        View::edit(&self.handler, EDIT_ON_OPEN);
        kResultOk
    }
    unsafe fn removed(&self) -> tresult {
        #[cfg(target_os = "linux")]
        if let Some((rl, tick)) = self.timer.lock().unwrap().take() {
            use Linux::IRunLoopTrait;
            if let Some(t) = tick.to_com_ptr::<Linux::ITimerHandler>() {
                rl.unregisterTimer(t.as_ptr());
            }
        }
        kResultOk
    }
    unsafe fn onWheel(&self, _d: f32) -> tresult {
        kResultFalse
    }
    unsafe fn onKeyDown(&self, _k: char16, _c: int16, _m: int16) -> tresult {
        kResultFalse
    }
    unsafe fn onKeyUp(&self, _k: char16, _c: int16, _m: int16) -> tresult {
        kResultFalse
    }
    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        *size = ViewRect {
            left: 0,
            top: 0,
            right: VIEW_SIZE.0,
            bottom: VIEW_SIZE.1,
        };
        kResultOk
    }
    unsafe fn onSize(&self, _size: *mut ViewRect) -> tresult {
        kResultOk
    }
    unsafe fn onFocus(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setFrame(&self, frame: *mut IPlugFrame) -> tresult {
        #[cfg(target_os = "linux")]
        if let Some(f) = ComRef::from_raw(frame) {
            use Linux::IRunLoopTrait;
            if let Some(rl) = f.cast::<Linux::IRunLoop>() {
                let tick = ComWrapper::new(Tick {
                    handler: self.handler.clone(),
                    frame: f.to_com_ptr(),
                    done: std::sync::atomic::AtomicBool::new(false),
                });
                if let Some(t) = tick.to_com_ptr::<Linux::ITimerHandler>() {
                    rl.registerTimer(t.as_ptr(), 10);
                }
                *self.timer.lock().unwrap() = Some((rl, tick));
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = frame;
        kResultOk
    }
    unsafe fn canResize(&self) -> tresult {
        kResultFalse
    }
    unsafe fn checkSizeConstraint(&self, _rect: *mut ViewRect) -> tresult {
        kResultOk
    }
}

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;
        copy_cstring("Lumen Audio Studio", &mut info.vendor);
        copy_cstring(
            "https://github.com/Powelus/lumen-audio-studio",
            &mut info.url,
        );
        copy_cstring("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        2
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let info = &mut *info;
        match index {
            0 => {
                info.cid = Processor::CID;
                copy_cstring("Audio Module Class", &mut info.category);
            }
            1 => {
                info.cid = Controller::CID;
                copy_cstring("Component Controller Class", &mut info.category);
            }
            _ => return kInvalidArgument,
        }
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring(NAME, &mut info.name);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let instance = match *(cid as *const TUID) {
            Processor::CID => ComWrapper::new(Processor::new()).to_com_ptr::<FUnknown>(),
            Controller::CID => ComWrapper::new(Controller {
                gain: Mutex::new(0.5),
                handler: Mutex::new(None),
            })
            .to_com_ptr::<FUnknown>(),
            _ => None,
        };
        match instance {
            Some(i) => {
                let p = i.as_ptr();
                ((*(*p).vtbl).queryInterface)(p, iid as *mut TUID, obj)
            }
            None => kInvalidArgument,
        }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let info = &mut *info;
        match index {
            0 => {
                info.cid = Processor::CID;
                copy_cstring("Audio Module Class", &mut info.category);
                copy_cstring("Instrument|Fx", &mut info.subCategories);
            }
            1 => {
                info.cid = Controller::CID;
                copy_cstring("Component Controller Class", &mut info.category);
                copy_cstring("", &mut info.subCategories);
            }
            _ => return kInvalidArgument,
        }
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring(NAME, &mut info.name);
        info.classFlags = 0;
        copy_cstring("Lumen Audio Studio", &mut info.vendor);
        copy_cstring("1.0", &mut info.version);
        copy_cstring("VST 3.7", &mut info.sdkVersion);
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}
#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}
#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}
#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleExit() -> bool {
    true
}
#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_library_handle: *mut c_void) -> bool {
    true
}
#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .unwrap()
        .into_raw()
}
