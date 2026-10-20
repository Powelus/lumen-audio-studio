// SPDX-License-Identifier: GPL-3.0-or-later
//! Hosting of VST3 plugins: finding them, loading them, and running them as an
//! instrument or an effect. The VST3 interfaces are COM objects; `vst3` has the
//! bindings, this file is the host side: the host application object, the streams, the
//! event list and the parameter changes a plugin is given, and the setup a plugin
//! needs before it can process. Like [`crate::Clap`], a [`Vst3`] is built off the audio
//! thread and then used by one thread at a time.
#![allow(non_snake_case, non_upper_case_globals)]
// the enum types of the bindings differ by platform, so some casts only look needless
#![allow(clippy::unnecessary_cast)]

use crate::{NoteMsg, Param, PluginInfo, MAX_FRAMES};
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};

/// The binary inside a `.vst3` (a bundle folder, or on Windows often the file itself).
pub fn module_binary(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let inside = if cfg!(target_os = "macos") {
        format!("Contents/MacOS/{stem}")
    } else if cfg!(windows) {
        format!("Contents/{arch}-win/{stem}.vst3")
    } else {
        format!("Contents/{arch}-linux/{stem}.so")
    };
    let bin = path.join(inside);
    bin.is_file().then_some(bin)
}

/// A loaded plugin library, entered and ready to give its factory.
struct Module {
    #[cfg(unix)]
    handle: *mut c_void,
    #[cfg(not(unix))]
    lib: libloading::Library,
}

impl Module {
    fn open(path: &Path) -> Result<Self, String> {
        let bin = module_binary(path).ok_or("no plugin binary in the bundle")?;
        #[cfg(unix)]
        {
            // SAFETY: loading a plugin library runs its initialisation code; that is
            // what a host does
            let lib =
                unsafe { libloading::os::unix::Library::new(&bin) }.map_err(|e| e.to_string())?;
            let handle = lib.into_raw();
            let m = Self { handle };
            // the entry point is called with the handle on Linux; macOS plugins want a
            // CFBundle, which is not made here, and are entered without it
            #[cfg(not(target_os = "macos"))]
            m.enter("ModuleEntry", handle)?;
            Ok(m)
        }
        #[cfg(not(unix))]
        {
            // SAFETY: as above
            let lib = unsafe { libloading::Library::new(&bin) }.map_err(|e| e.to_string())?;
            let m = Self { lib };
            m.enter("InitDll", std::ptr::null_mut())?;
            Ok(m)
        }
    }

    #[cfg(unix)]
    fn lib(&self) -> std::mem::ManuallyDrop<libloading::os::unix::Library> {
        // SAFETY: the handle came from `into_raw` and stays valid until `drop`
        std::mem::ManuallyDrop::new(unsafe { libloading::os::unix::Library::from_raw(self.handle) })
    }

    #[cfg(not(target_os = "macos"))]
    fn enter(&self, name: &str, arg: *mut c_void) -> Result<(), String> {
        #[cfg(unix)]
        let lib = self.lib();
        #[cfg(not(unix))]
        let lib = &self.lib;
        // SAFETY: the VST3 entry points have these signatures
        unsafe {
            if let Ok(f) =
                lib.get::<unsafe extern "system" fn(*mut c_void) -> bool>(name.as_bytes())
            {
                if !f(arg) {
                    return Err("the plugin refused to start".into());
                }
            } else if let Ok(f) = lib.get::<unsafe extern "system" fn() -> bool>(name.as_bytes()) {
                if !f() {
                    return Err("the plugin refused to start".into());
                }
            }
        }
        Ok(())
    }

    fn factory(&self) -> Result<ComPtr<IPluginFactory>, String> {
        #[cfg(unix)]
        let lib = self.lib();
        #[cfg(not(unix))]
        let lib = &self.lib;
        // SAFETY: `GetPluginFactory` is the VST3 entry point and returns an owned pointer
        unsafe {
            let f = lib
                .get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory")
                .map_err(|e| e.to_string())?;
            ComPtr::from_raw(f()).ok_or_else(|| "the plugin has no factory".to_string())
        }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        #[cfg(unix)]
        let lib = self.lib();
        #[cfg(not(unix))]
        let lib = &self.lib;
        // SAFETY: the exit points have these signatures
        unsafe {
            for name in ["ModuleExit", "ExitDll"] {
                if let Ok(f) = lib.get::<unsafe extern "system" fn() -> bool>(name.as_bytes()) {
                    f();
                }
            }
        }
        #[cfg(unix)]
        {
            drop(std::mem::ManuallyDrop::into_inner(self.lib()));
        }
    }
}

fn cstr(s: &[c_char]) -> String {
    let bytes: Vec<u8> = s
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wstr(s: &[TChar]) -> String {
    let units: Vec<u16> = s
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u16)
        .collect();
    String::from_utf16_lossy(&units)
}

fn hex(cid: &TUID) -> String {
    cid.iter().map(|&b| format!("{:02X}", b as u8)).collect()
}

fn from_hex(s: &str) -> Option<TUID> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0 as c_char; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()? as c_char;
    }
    Some(out)
}

/// The audio plugins in one `.vst3`.
pub fn describe(path: &Path) -> Vec<PluginInfo> {
    let Ok(module) = Module::open(path) else {
        return Vec::new();
    };
    let Ok(factory) = module.factory() else {
        return Vec::new();
    };
    let mut vendor = String::new();
    // SAFETY: plain calls into the factory
    unsafe {
        let mut fi: PFactoryInfo = std::mem::zeroed();
        if factory.getFactoryInfo(&mut fi) == kResultOk {
            vendor = cstr(&fi.vendor);
        }
        let f2 = factory.cast::<IPluginFactory2>();
        let mut out = Vec::new();
        for i in 0..factory.countClasses() {
            let (cid, category, name, subs, v) = if let Some(f2) = &f2 {
                let mut ci: PClassInfo2 = std::mem::zeroed();
                if f2.getClassInfo2(i, &mut ci) != kResultOk {
                    continue;
                }
                (
                    ci.cid,
                    cstr(&ci.category),
                    cstr(&ci.name),
                    cstr(&ci.subCategories),
                    cstr(&ci.vendor),
                )
            } else {
                let mut ci: PClassInfo = std::mem::zeroed();
                if factory.getClassInfo(i, &mut ci) != kResultOk {
                    continue;
                }
                (
                    ci.cid,
                    cstr(&ci.category),
                    cstr(&ci.name),
                    String::new(),
                    String::new(),
                )
            };
            if category != "Audio Module Class" {
                continue;
            }
            let mut features = Vec::new();
            for sub in subs.split('|') {
                match sub.to_ascii_lowercase().as_str() {
                    "instrument" => features.push("instrument".to_string()),
                    "fx" => features.push("audio-effect".to_string()),
                    "" => {}
                    other => features.push(other.to_string()),
                }
            }
            out.push(PluginInfo {
                path: path.to_path_buf(),
                id: hex(&cid),
                name,
                vendor: if v.is_empty() { vendor.clone() } else { v },
                features,
                format: crate::Format::Vst3,
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// What the host gives a plugin
// ---------------------------------------------------------------------------

struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        for (d, s) in (*name)
            .iter_mut()
            .zip("Lumen Audio Studio\0".encode_utf16())
        {
            *d = s as TChar;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        _obj: *mut *mut c_void,
    ) -> tresult {
        kNotImplemented
    }
}

/// A stream in memory, for a plugin's state.
struct MemStream {
    data: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl MemStream {
    fn new(bytes: Vec<u8>) -> ComWrapper<Self> {
        ComWrapper::new(Self {
            data: Mutex::new((bytes, 0)),
        })
    }

    fn bytes(&self) -> Vec<u8> {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .clone()
    }
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num: int32, read: *mut int32) -> tresult {
        let mut g = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let left = g.0.len().saturating_sub(g.1);
        let n = (num.max(0) as usize).min(left);
        std::ptr::copy_nonoverlapping(g.0.as_ptr().add(g.1), buffer as *mut u8, n);
        g.1 += n;
        if !read.is_null() {
            *read = n as int32;
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num: int32, written: *mut int32) -> tresult {
        let mut g = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let n = num.max(0) as usize;
        let at = g.1;
        if g.0.len() < at + n {
            g.0.resize(at + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, g.0.as_mut_ptr().add(at), n);
        g.1 += n;
        if !written.is_null() {
            *written = n as int32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let base = if mode == IBStream_::IStreamSeekMode_::kIBSeekSet as int32 {
            0
        } else if mode == IBStream_::IStreamSeekMode_::kIBSeekCur as int32 {
            g.1 as i64
        } else if mode == IBStream_::IStreamSeekMode_::kIBSeekEnd as int32 {
            g.0.len() as i64
        } else {
            return kInvalidArgument;
        };
        let to = base + pos;
        if to < 0 {
            return kInvalidArgument;
        }
        g.1 = to as usize;
        if !result.is_null() {
            *result = to;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        let g = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if !pos.is_null() {
            *pos = g.1 as i64;
        }
        kResultOk
    }
}

/// The notes of a block.
struct Events {
    list: Mutex<Vec<Event>>,
}

impl Class for Events {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> int32 {
        self.list.lock().unwrap_or_else(|e| e.into_inner()).len() as int32
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        let g = self.list.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(index.max(0) as usize) {
            Some(x) if index >= 0 => {
                *e = *x;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, _e: *mut Event) -> tresult {
        kResultFalse
    }
}

/// One parameter's changes in a block.
struct Queue {
    id: ParamID,
    points: Vec<(int32, f64)>,
}

impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.points.len() as int32
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        match self.points.get(index.max(0) as usize) {
            Some(&(o, v)) if index >= 0 => {
                *offset = o;
                *value = v;
                kResultTrue
            }
            _ => kResultFalse,
        }
    }

    unsafe fn addPoint(&self, _offset: int32, _value: ParamValue, _index: *mut int32) -> tresult {
        kResultFalse
    }
}

/// The parameter changes of a block.
struct Changes {
    queues: Vec<ComWrapper<Queue>>,
}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        self.queues.len() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        match self.queues.get(index.max(0) as usize) {
            Some(q) if index >= 0 => q
                .as_com_ref::<IParamValueQueue>()
                .map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            _ => std::ptr::null_mut(),
        }
    }

    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        _index: *mut int32,
    ) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

// ---------------------------------------------------------------------------
// A running plugin
// ---------------------------------------------------------------------------

/// A running VST3 plugin.
pub struct Vst3 {
    info: PluginInfo,
    // the order matters: the objects go before the module they come from
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// Whether the controller is a separate object (it is then ended on its own).
    separate: bool,
    _host: ComWrapper<HostApp>,
    module: Module,
    params: Vec<Param>,
    audio_in: usize,
    audio_out: usize,
    in_channels: usize,
    out_channels: usize,
    has_events: bool,
    inp: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    pending: Vec<(u32, f64)>,
}

// SAFETY: see `Clap`: built on one thread, then used by one thread at a time
unsafe impl Send for Vst3 {}

impl Vst3 {
    pub fn load(info: &PluginInfo, sample_rate: f64) -> Result<Self, String> {
        let module = Module::open(&info.path)?;
        let factory = module.factory()?;
        let cid = from_hex(&info.id).ok_or("a bad plugin id")?;
        let host = ComWrapper::new(HostApp);
        let host_ctx = host
            .to_com_ptr::<FUnknown>()
            .ok_or("the host object has no interface")?;
        // SAFETY: calls into the plugin in the order the VST3 specification gives
        unsafe {
            let mut obj: *mut c_void = std::ptr::null_mut();
            let r = factory.createInstance(
                cid.as_ptr(),
                <IComponent as Interface>::IID.as_ptr() as *const c_char,
                &mut obj,
            );
            let component = (r == kResultOk)
                .then(|| ComPtr::from_raw(obj as *mut IComponent))
                .flatten()
                .ok_or("the plugin could not be created")?;
            if component.initialize(host_ctx.as_ptr()) != kResultOk {
                return Err("the plugin did not initialise".into());
            }
            let processor = component
                .cast::<IAudioProcessor>()
                .ok_or("the plugin is not an audio processor")?;

            // the controller: part of the component, or a class of its own
            let mut controller = component.cast::<IEditController>();
            let mut separate = false;
            if controller.is_none() {
                let mut ccid: TUID = [0; 16];
                if component.getControllerClassId(&mut ccid) == kResultOk {
                    let mut obj: *mut c_void = std::ptr::null_mut();
                    let r = factory.createInstance(
                        ccid.as_ptr(),
                        <IEditController as Interface>::IID.as_ptr() as *const c_char,
                        &mut obj,
                    );
                    if r == kResultOk {
                        controller = ComPtr::from_raw(obj as *mut IEditController);
                        if let Some(c) = &controller {
                            if c.initialize(host_ctx.as_ptr()) == kResultOk {
                                separate = true;
                            } else {
                                controller = None;
                            }
                        }
                    }
                }
            }
            if separate {
                if let (Some(a), Some(b)) = (
                    component.cast::<IConnectionPoint>(),
                    controller
                        .as_ref()
                        .and_then(|c| c.cast::<IConnectionPoint>()),
                ) {
                    a.connect(b.as_ptr());
                    b.connect(a.as_ptr());
                }
            }

            // buses: the first audio bus each way and the first note input
            let buses = |media: MediaTypes, dir: BusDirections| {
                component
                    .getBusCount(media as MediaType, dir as BusDirection)
                    .max(0) as usize
            };
            let audio_in = buses(MediaTypes_::kAudio, BusDirections_::kInput);
            let audio_out = buses(MediaTypes_::kAudio, BusDirections_::kOutput);
            let has_events = buses(MediaTypes_::kEvent, BusDirections_::kInput) > 0;
            let mut ins = vec![SpeakerArr::kStereo; audio_in];
            let mut outs = vec![SpeakerArr::kStereo; audio_out];
            // a plugin that wants other arrangements says which
            let asked = processor.setBusArrangements(
                ins.as_mut_ptr(),
                audio_in as i32,
                outs.as_mut_ptr(),
                audio_out as i32,
            );
            let channels = |dir: BusDirections| {
                let mut arr: SpeakerArrangement = SpeakerArr::kStereo;
                if asked != kResultTrue {
                    processor.getBusArrangement(dir as BusDirection, 0, &mut arr);
                }
                (arr.count_ones() as usize).clamp(1, 2)
            };
            let in_channels = channels(BusDirections_::kInput);
            let out_channels = channels(BusDirections_::kOutput);
            if audio_in > 0 {
                component.activateBus(
                    MediaTypes_::kAudio as MediaType,
                    BusDirections_::kInput as BusDirection,
                    0,
                    1,
                );
            }
            if audio_out > 0 {
                component.activateBus(
                    MediaTypes_::kAudio as MediaType,
                    BusDirections_::kOutput as BusDirection,
                    0,
                    1,
                );
            }
            if has_events {
                component.activateBus(
                    MediaTypes_::kEvent as MediaType,
                    BusDirections_::kInput as BusDirection,
                    0,
                    1,
                );
            }
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                maxSamplesPerBlock: MAX_FRAMES as int32,
                sampleRate: sample_rate,
            };
            if processor.setupProcessing(&mut setup) != kResultOk {
                return Err("the plugin refused the audio setup".into());
            }
            component.setActive(1);
            processor.setProcessing(1);

            let mut params = Vec::new();
            if let Some(c) = &controller {
                for i in 0..c.getParameterCount() {
                    let mut pi: ParameterInfo = std::mem::zeroed();
                    if c.getParameterInfo(i, &mut pi) != kResultOk {
                        continue;
                    }
                    let skip = ParameterInfo_::ParameterFlags_::kIsReadOnly
                        | ParameterInfo_::ParameterFlags_::kIsHidden
                        | ParameterInfo_::ParameterFlags_::kIsProgramChange;
                    if pi.flags & skip != 0 {
                        continue;
                    }
                    params.push(Param {
                        id: pi.id,
                        name: wstr(&pi.title),
                        min: 0.0,
                        max: 1.0,
                        default: pi.defaultNormalizedValue,
                    });
                }
            }

            Ok(Self {
                info: info.clone(),
                component,
                processor,
                controller,
                separate,
                _host: host,
                module,
                params,
                audio_in,
                audio_out,
                in_channels,
                out_channels,
                has_events,
                inp: [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]],
                out: [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]],
                pending: Vec::new(),
            })
        }
    }

    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// The parameters, with values from 0 to 1 as VST3 has them.
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    pub fn param_value(&mut self, id: u32) -> Option<f64> {
        let c = self.controller.as_ref()?;
        // SAFETY: a call into the controller
        Some(unsafe { c.getParamNormalized(id) })
    }

    pub fn set_param(&mut self, id: u32, value: f64) {
        if let Some(c) = &self.controller {
            // SAFETY: a call into the controller
            unsafe { c.setParamNormalized(id, value) };
        }
        self.pending.push((id, value));
    }

    pub fn takes_audio(&self) -> bool {
        self.audio_in > 0
    }

    /// The state of the plugin's component.
    pub fn save_state(&mut self) -> Option<Vec<u8>> {
        let stream = MemStream::new(Vec::new());
        let ptr = stream.to_com_ptr::<IBStream>()?;
        // SAFETY: the stream outlives the call
        let r = unsafe { self.component.getState(ptr.as_ptr()) };
        (r == kResultOk).then(|| stream.bytes())
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let stream = MemStream::new(bytes.to_vec());
        let ptr = stream.to_com_ptr::<IBStream>().ok_or("no stream")?;
        // SAFETY: the stream outlives the calls
        unsafe {
            if self.component.setState(ptr.as_ptr()) != kResultOk {
                return Err("the plugin refused the state".into());
            }
            if let Some(c) = &self.controller {
                let again = MemStream::new(bytes.to_vec());
                if let Some(p) = again.to_com_ptr::<IBStream>() {
                    c.setComponentState(p.as_ptr());
                }
            }
        }
        Ok(())
    }

    /// Run the plugin: the same contract as [`crate::Clap::process`].
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
            out_l[range.clone()].copy_from_slice(&self.out[0][..n]);
            out_r[range].copy_from_slice(&self.out[1][..n]);
            done += n;
        }
    }

    fn block(&mut self, input: Option<(&[f32], &[f32])>, notes: &[NoteMsg], from: u32, n: usize) {
        match input {
            Some((l, r)) => {
                self.inp[0][..n].copy_from_slice(&l[..n]);
                self.inp[1][..n].copy_from_slice(&r[..n]);
            }
            None => {
                self.inp[0][..n].fill(0.0);
                self.inp[1][..n].fill(0.0);
            }
        }
        self.out[0][..n].fill(0.0);
        self.out[1][..n].fill(0.0);

        // events
        let mut list: Vec<Event> = Vec::new();
        for m in notes {
            if m.offset() < from || m.offset() >= from + n as u32 {
                continue;
            }
            // SAFETY: a plain old data struct, filled in below
            let mut e: Event = unsafe { std::mem::zeroed() };
            e.sampleOffset = (m.offset() - from) as int32;
            match *m {
                NoteMsg::On { key, vel, .. } => {
                    e.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                    e.__field0.noteOn = NoteOnEvent {
                        channel: 0,
                        pitch: i16::from(key),
                        tuning: 0.0,
                        velocity: vel,
                        length: 0,
                        noteId: i32::from(key),
                    };
                }
                NoteMsg::Off { key, .. } => {
                    e.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                    e.__field0.noteOff = NoteOffEvent {
                        channel: 0,
                        pitch: i16::from(key),
                        velocity: 0.0,
                        noteId: i32::from(key),
                        tuning: 0.0,
                    };
                }
            }
            list.push(e);
        }
        list.sort_by_key(|e| e.sampleOffset);
        let events = ComWrapper::new(Events {
            list: Mutex::new(list),
        });
        let queues = self
            .pending
            .drain(..)
            .map(|(id, v)| {
                ComWrapper::new(Queue {
                    id,
                    points: vec![(0, v)],
                })
            })
            .collect();
        let changes = ComWrapper::new(Changes { queues });
        let (Some(events_ptr), Some(changes_ptr)) = (
            events.to_com_ptr::<IEventList>(),
            changes.to_com_ptr::<IParameterChanges>(),
        ) else {
            return;
        };

        let [i0, i1] = &mut self.inp;
        let [o0, o1] = &mut self.out;
        let mut in_ptrs = [i0.as_mut_ptr(), i1.as_mut_ptr()];
        let mut out_ptrs = [o0.as_mut_ptr(), o1.as_mut_ptr()];
        // every bus is passed; only the first of each way is active
        let mut ins: Vec<AudioBusBuffers> = (0..self.audio_in)
            .map(|b| AudioBusBuffers {
                numChannels: if b == 0 { self.in_channels as int32 } else { 0 },
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: if b == 0 {
                        in_ptrs.as_mut_ptr()
                    } else {
                        std::ptr::null_mut()
                    },
                },
            })
            .collect();
        let mut outs: Vec<AudioBusBuffers> = (0..self.audio_out)
            .map(|b| AudioBusBuffers {
                numChannels: if b == 0 {
                    self.out_channels as int32
                } else {
                    0
                },
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: if b == 0 {
                        out_ptrs.as_mut_ptr()
                    } else {
                        std::ptr::null_mut()
                    },
                },
            })
            .collect();
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: n as int32,
            numInputs: ins.len() as int32,
            numOutputs: outs.len() as int32,
            inputs: if ins.is_empty() {
                std::ptr::null_mut()
            } else {
                ins.as_mut_ptr()
            },
            outputs: if outs.is_empty() {
                std::ptr::null_mut()
            } else {
                outs.as_mut_ptr()
            },
            inputParameterChanges: changes_ptr.as_ptr(),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: if self.has_events {
                events_ptr.as_ptr()
            } else {
                std::ptr::null_mut()
            },
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };
        // SAFETY: the buffers and the objects outlive the call
        let _ = unsafe { self.processor.process(&mut data) };
        // a mono plugin plays on both sides
        if self.out_channels == 1 {
            let [a, b] = &mut self.out;
            b[..n].copy_from_slice(&a[..n]);
        }
    }
}

impl Drop for Vst3 {
    fn drop(&mut self) {
        // SAFETY: the end of the lifecycle, in the order the specification gives
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if self.separate {
                if let (Some(a), Some(b)) = (
                    self.component.cast::<IConnectionPoint>(),
                    self.controller
                        .as_ref()
                        .and_then(|c| c.cast::<IConnectionPoint>()),
                ) {
                    a.disconnect(b.as_ptr());
                    b.disconnect(a.as_ptr());
                }
                if let Some(c) = &self.controller {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
        let _ = &self.module;
    }
}
