// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin hosting: finds CLAP, VST3 and VST2 plugins on disk (see [`scan`]), loads
//! them and runs them as an instrument (notes in, audio out) or as an effect (audio
//! in, audio out).
//!
//! A [`Clap`] owns one running plugin. Build it off the audio thread with [`Clap::load`]
//! (loading, activating and reading the parameters are slow and not realtime safe), then
//! call [`Clap::process`] from the audio thread. The plugin's own code is outside this
//! program's control: a plugin can crash the process, and its callbacks can arrive from
//! other threads. A scan therefore asks each library what it is in a process of its
//! own when it can (see [`scan`]).

use clack_extensions::audio_ports::PluginAudioPorts;
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::prelude::*;
use clack_host::process::ProcessStatus;
pub mod bridge;
pub mod clap_editor;
pub mod scan;
pub mod vst2;
pub mod vst3;

use std::ffi::CString;
use std::path::{Path, PathBuf};

/// The kind of a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Clap,
    Vst3,
    Vst2,
}

impl Format {
    /// The short name shown next to a plugin: "CLAP", "VST3", "VST2".
    pub fn tag(self) -> &'static str {
        scan::format_tag(self)
    }
}

/// What a scan learns about a plugin without starting it.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginInfo {
    /// The library the plugin is in.
    pub path: PathBuf,
    /// The plugin's id, unique among plugins.
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// What the plugin declares it is ("instrument", "audio-effect", ...).
    pub features: Vec<String>,
    pub format: Format,
}

impl PluginInfo {
    /// Whether the plugin makes sound from notes.
    pub fn is_instrument(&self) -> bool {
        self.has("instrument") || self.has("synthesizer") || self.has("sampler")
    }

    /// Whether the plugin processes audio.
    pub fn is_effect(&self) -> bool {
        self.has("audio-effect")
    }

    fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// Every standard place plugins of any kind are looked for on this system (and
/// `CLAP_PATH`, `VST3_PATH`, `VST_PATH`).
pub fn all_search_paths() -> Vec<PathBuf> {
    scan::system_paths()
}

/// Every plugin in the plugin files under `dirs` (see [`scan::collect`]), each file
/// asked in its own process when it can be. Files that are not usable are skipped;
/// [`rescan_cached`] also says why.
pub fn scan(dirs: &[PathBuf]) -> Vec<PluginInfo> {
    scan::scan_with(
        dirs,
        scan::Os::current(),
        &mut scan::Cache::default(),
        &scan::describe_isolated,
        &mut |_| {},
    )
    .plugins
}

/// The plugins in one library of any kind, asked in this process.
pub fn describe_any(path: &Path) -> Vec<PluginInfo> {
    scan::describe_local(path).unwrap_or_default()
}

/// The plugins in one CLAP library.
pub fn describe(path: &Path) -> Vec<PluginInfo> {
    // SAFETY: loading a plugin library runs its initialisation code; that is what a host does
    let Ok(entry) = (unsafe { PluginEntry::load(path) }) else {
        return Vec::new();
    };
    let Some(factory) = entry.get_plugin_factory() else {
        return Vec::new();
    };
    let text = |c: Option<&std::ffi::CStr>| {
        c.map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    factory
        .plugin_descriptors()
        .filter_map(|d| {
            let id = text(d.id());
            (!id.is_empty()).then(|| PluginInfo {
                path: path.to_path_buf(),
                name: match text(d.name()) {
                    n if n.is_empty() => id.clone(),
                    n => n,
                },
                vendor: text(d.vendor()),
                features: d
                    .features()
                    .map(|f| f.to_string_lossy().into_owned())
                    .collect(),
                format: Format::Clap,
                id,
            })
        })
        .collect()
}

/// What a plugin lets the host change.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// A note for a plugin, `offset` frames into the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteMsg {
    On { offset: u32, key: u8, vel: f32 },
    Off { offset: u32, key: u8 },
}

impl NoteMsg {
    fn offset(&self) -> u32 {
        match *self {
            Self::On { offset, .. } | Self::Off { offset, .. } => offset,
        }
    }
}

struct Host;

struct HostShared;

impl SharedHandler<'_> for HostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

impl HostHandlers for Host {
    type Shared<'a> = HostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

/// The most frames given to a plugin in one call.
pub const MAX_FRAMES: usize = 512;

/// A running plugin.
pub struct Clap {
    info: PluginInfo,
    instance: PluginInstance<Host>,
    processor: Option<StartedPluginAudioProcessor<Host>>,
    params: Vec<Param>,
    audio_in: usize,
    audio_out: usize,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    inp: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    events: EventBuffer,
    out_events: EventBuffer,
    /// Parameter changes to deliver with the next block.
    pending: Vec<(u32, f64)>,
    /// The plugin said it has nothing to do until an event or some input comes
    /// (CLAP's sleep status): it is not called until then.
    asleep: bool,
}

// SAFETY: a `Clap` is built on one thread and then used by the one thread that holds it
// (the audio thread), and dropped by a thread that holds it alone. The plugin's
// main-thread calls (parameters, state) happen only while the host thread holds it.
unsafe impl Send for Clap {}

impl Clap {
    /// Load, activate and start `info`'s plugin.
    pub fn load(info: &PluginInfo, sample_rate: f64) -> Result<Self, String> {
        // SAFETY: see `describe`
        let entry = unsafe { PluginEntry::load(&info.path) }.map_err(|e| e.to_string())?;
        let host = HostInfo::new("Lumen Audio Studio", "Lumen", "", env!("CARGO_PKG_VERSION"))
            .map_err(|e| e.to_string())?;
        let id = CString::new(info.id.clone()).map_err(|e| e.to_string())?;
        let mut instance = PluginInstance::<Host>::new(|_| HostShared, |_| (), &entry, &id, &host)
            .map_err(|e| e.to_string())?;

        let (audio_in, audio_out, params) = {
            let handle = instance.plugin_handle();
            let ports = handle.get_extension::<PluginAudioPorts>();
            let count = |input| ports.map_or(0, |p| p.count(&handle, input) as usize);
            let mut params = Vec::new();
            if let Some(ext) = handle.get_extension::<PluginParams>() {
                let mut buf = ParamInfoBuffer::new();
                for i in 0..ext.count(&handle) {
                    if let Some(p) = ext.get_info(&handle, i, &mut buf) {
                        params.push(Param {
                            id: p.id.get(),
                            name: String::from_utf8_lossy(p.name).into_owned(),
                            min: p.min_value,
                            max: p.max_value,
                            default: p.default_value,
                        });
                    }
                }
            }
            (count(true), count(false), params)
        };

        let config = PluginAudioConfiguration {
            sample_rate,
            min_frames_count: 1,
            max_frames_count: MAX_FRAMES as u32,
        };
        let stopped = instance
            .activate(|_, _| (), config)
            .map_err(|e| e.to_string())?;
        let processor = stopped.start_processing().map_err(|e| e.to_string())?;

        Ok(Self {
            info: info.clone(),
            instance,
            processor: Some(processor),
            params,
            audio_in,
            audio_out,
            in_ports: AudioPorts::with_capacity(2, 1),
            out_ports: AudioPorts::with_capacity(2, 1),
            inp: [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]],
            out: [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]],
            events: EventBuffer::with_capacity(64),
            out_events: EventBuffer::with_capacity(64),
            pending: Vec::new(),
            asleep: false,
        })
    }

    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// The current value of a parameter.
    pub fn param_value(&mut self, id: u32) -> Option<f64> {
        let handle = self.instance.plugin_handle();
        let ext = handle.get_extension::<PluginParams>()?;
        ext.get_value(&handle, ClapId::new(id))
    }

    /// Change a parameter; it takes effect at the start of the next block.
    pub fn set_param(&mut self, id: u32, value: f64) {
        self.pending.push((id, value));
    }

    /// The plugin's state, to save in a project.
    pub fn save_state(&mut self) -> Option<Vec<u8>> {
        let handle = self.instance.plugin_handle();
        let ext = handle.get_extension::<PluginState>()?;
        let mut bytes = Vec::new();
        ext.save(&handle, &mut bytes).ok()?;
        Some(bytes)
    }

    /// Restore a state saved by [`Clap::save_state`].
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let handle = self.instance.plugin_handle();
        let ext = handle
            .get_extension::<PluginState>()
            .ok_or("the plugin has no state")?;
        ext.load(&handle, &mut std::io::Cursor::new(bytes))
            .map_err(|e| e.to_string())
    }

    /// Whether the plugin reads audio.
    pub fn takes_audio(&self) -> bool {
        self.audio_in > 0
    }

    /// Run the plugin over `out_l.len()` frames. `input` is the audio to process (an
    /// effect's), if any; `notes` are the notes of the block, in order of their offset.
    /// What the plugin plays replaces the content of `out_l` and `out_r`.
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
            let (a, b) = self.block(part, notes, done as u32, n);
            out_l[range.clone()].copy_from_slice(&a[..n]);
            out_r[range].copy_from_slice(&b[..n]);
            done += n;
        }
    }

    /// One block of at most `MAX_FRAMES`; `from` is the offset of the block in the
    /// notes' time.
    fn block(
        &mut self,
        input: Option<(&[f32], &[f32])>,
        notes: &[NoteMsg],
        from: u32,
        n: usize,
    ) -> (&[f32], &[f32]) {
        self.events.clear();
        self.out_events.clear();
        for (id, v) in self.pending.drain(..) {
            self.events.push(&ParamValueEvent::new(
                0,
                ClapId::new(id),
                Pckn::match_all(),
                v,
            ));
        }
        for m in notes {
            let o = m.offset();
            if o < from || o >= from + n as u32 {
                continue;
            }
            let at = o - from;
            match *m {
                NoteMsg::On { key, vel, .. } => self.events.push(&NoteOnEvent::new(
                    at,
                    Pckn::new(0u16, 0u16, u16::from(key), u32::from(key)),
                    f64::from(vel),
                )),
                NoteMsg::Off { key, .. } => self.events.push(&NoteOffEvent::new(
                    at,
                    Pckn::new(0u16, 0u16, u16::from(key), u32::from(key)),
                    0.0,
                )),
            }
        }
        // the host's events must be in time order
        self.events.sort();

        let quiet_in = input.is_none_or(|(l, r)| l.iter().chain(r).all(|s| s.abs() < 1e-6));
        if self.asleep && self.events.is_empty() && quiet_in {
            self.out[0][..n].fill(0.0);
            self.out[1][..n].fill(0.0);
            return (&self.out[0], &self.out[1]);
        }

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

        let [i0, i1] = &mut self.inp;
        let [o0, o1] = &mut self.out;
        let in_events = self.events.as_input();
        let mut out_events = self.out_events.as_output();
        let mut in_audio = if self.audio_in > 0 {
            self.in_ports.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    [&mut i0[..n], &mut i1[..n]]
                        .into_iter()
                        .map(InputChannel::variable),
                ),
            }])
        } else {
            self.in_ports.with_input_buffers(Vec::<
                AudioPortBuffer<Vec<InputChannel<f32>>, Vec<InputChannel<f64>>>,
            >::new())
        };
        let mut out_audio = if self.audio_out > 0 {
            self.out_ports.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    [&mut o0[..n], &mut o1[..n]].into_iter(),
                ),
            }])
        } else {
            self.out_ports.with_output_buffers(Vec::<
                AudioPortBuffer<Vec<&mut [f32]>, Vec<&mut [f64]>>,
            >::new())
        };
        if let Some(p) = self.processor.as_mut() {
            // a plugin that fails a block is silent for that block
            let status = p.process(
                &in_audio,
                &mut out_audio,
                &in_events,
                &mut out_events,
                None,
                None,
            );
            let quiet_out = || {
                self.out[0][..n]
                    .iter()
                    .chain(&self.out[1][..n])
                    .all(|s| s.abs() < 1e-6)
            };
            self.asleep = match status {
                Ok(ProcessStatus::Sleep) => quiet_out(),
                Ok(ProcessStatus::ContinueIfNotQuiet) => quiet_in && quiet_out(),
                _ => false,
            };
        }
        let _ = &mut in_audio;
        (&self.out[0], &self.out[1])
    }
}

impl Drop for Clap {
    fn drop(&mut self) {
        if let Some(p) = self.processor.take() {
            let stopped = p.stop_processing();
            self.instance.deactivate(stopped);
        }
    }
}

// ---------------------------------------------------------------------------
// A plugin of either kind
// ---------------------------------------------------------------------------

/// A running plugin, CLAP or VST3.
pub enum Plugin {
    Clap(Clap),
    Vst3(vst3::Vst3),
    Vst2(vst2::Vst2),
    /// A plugin in its own process (see [`bridge`]).
    Remote(bridge::Remote),
}

impl Plugin {
    /// Load a plugin: in its own process when the plugin host program is there (see
    /// [`bridge::sandboxed`]), in this one otherwise.
    pub fn load(info: &PluginInfo, sample_rate: f64) -> Result<Self, String> {
        if bridge::sandboxed() {
            return bridge::Remote::load(info, sample_rate).map(Self::Remote);
        }
        Self::load_local(info, sample_rate)
    }

    /// Load a plugin in this process.
    pub fn load_local(info: &PluginInfo, sample_rate: f64) -> Result<Self, String> {
        match info.format {
            Format::Clap => Clap::load(info, sample_rate).map(Self::Clap),
            Format::Vst3 => vst3::Vst3::load(info, sample_rate).map(Self::Vst3),
            Format::Vst2 => vst2::Vst2::load(info, sample_rate).map(Self::Vst2),
        }
    }

    pub fn info(&self) -> &PluginInfo {
        match self {
            Self::Clap(p) => p.info(),
            Self::Vst3(p) => p.info(),
            Self::Vst2(p) => p.info(),
            Self::Remote(p) => p.info(),
        }
    }

    /// The parameters; a VST3 plugin's go from 0 to 1.
    pub fn params(&self) -> &[Param] {
        match self {
            Self::Clap(p) => p.params(),
            Self::Vst3(p) => p.params(),
            Self::Vst2(p) => p.params(),
            Self::Remote(p) => p.params(),
        }
    }

    pub fn param_value(&mut self, id: u32) -> Option<f64> {
        match self {
            Self::Clap(p) => p.param_value(id),
            Self::Vst3(p) => p.param_value(id),
            Self::Vst2(p) => p.param_value(id),
            Self::Remote(p) => p.param_value(id),
        }
    }

    pub fn set_param(&mut self, id: u32, value: f64) {
        match self {
            Self::Clap(p) => p.set_param(id, value),
            Self::Vst3(p) => p.set_param(id, value),
            Self::Vst2(p) => p.set_param(id, value),
            Self::Remote(p) => p.set_param(id, value),
        }
    }

    pub fn save_state(&mut self) -> Option<Vec<u8>> {
        match self {
            Self::Clap(p) => p.save_state(),
            Self::Vst3(p) => p.save_state(),
            Self::Vst2(p) => p.save_state(),
            Self::Remote(p) => p.save_state(),
        }
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Clap(p) => p.load_state(bytes),
            Self::Vst3(p) => p.load_state(bytes),
            Self::Vst2(p) => p.load_state(bytes),
            Self::Remote(p) => p.load_state(bytes),
        }
    }

    pub fn takes_audio(&self) -> bool {
        match self {
            Self::Clap(p) => p.takes_audio(),
            Self::Vst3(p) => p.takes_audio(),
            Self::Vst2(p) => p.takes_audio(),
            Self::Remote(p) => p.takes_audio(),
        }
    }

    pub fn process(
        &mut self,
        input: Option<(&[f32], &[f32])>,
        notes: &[NoteMsg],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        match self {
            Self::Clap(p) => p.process(input, notes, out_l, out_r),
            Self::Vst3(p) => p.process(input, notes, out_l, out_r),
            Self::Vst2(p) => p.process(input, notes, out_l, out_r),
            Self::Remote(p) => p.process(input, notes, out_l, out_r),
        }
    }
}

// ---------------------------------------------------------------------------
// The plugins this program knows, and the engine's use of them
// ---------------------------------------------------------------------------

use las_dsp::fx::registry::External;
use las_engine::external::{set_plugin_host, Instrument, Note, ParamLines, PluginHost, PLUGIN_ID};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

static KNOWN: RwLock<Vec<PluginInfo>> = RwLock::new(Vec::new());

static PROGRESS: RwLock<Option<scan::Progress>> = RwLock::new(None);
static REPORT: RwLock<Option<scan::Report>> = RwLock::new(None);
/// One scan at a time (the one at start and one asked for in Settings).
static SCANNING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Look for plugins in `dirs` and remember what is found.
pub fn rescan(dirs: &[PathBuf]) -> Vec<PluginInfo> {
    rescan_cached(dirs, None).plugins
}

/// Look for plugins in `dirs`, with the answers kept in the cache file `cache` (when
/// given) for the files that have not changed, and remember what is found.
/// [`scan_progress`] says where the scan is meanwhile, and [`last_report`] what it
/// found, failures included.
pub fn rescan_cached(dirs: &[PathBuf], cache: Option<&Path>) -> scan::Report {
    rescan_forgetting(dirs, cache, None)
}

/// [`rescan_cached`], asking the file `forget` again whatever the cache says.
pub fn rescan_forgetting(
    dirs: &[PathBuf],
    cache: Option<&Path>,
    forget: Option<&Path>,
) -> scan::Report {
    let _one = SCANNING.lock().unwrap_or_else(|e| e.into_inner());
    let mut kept = cache.map(scan::Cache::load).unwrap_or_default();
    if let Some(f) = forget {
        kept.entries.retain(|e| e.path != f);
    }
    let set = |p: Option<scan::Progress>| *PROGRESS.write().unwrap_or_else(|e| e.into_inner()) = p;
    set(Some(scan::Progress {
        done: 0,
        total: 0,
        current: String::new(),
    }));
    let report = scan::scan_with(
        dirs,
        scan::Os::current(),
        &mut kept,
        &scan::describe_isolated,
        &mut |p| set(Some(p.clone())),
    );
    if let Some(f) = cache {
        kept.save(f);
    }
    *KNOWN.write().unwrap_or_else(|e| e.into_inner()) = report.plugins.clone();
    *REPORT.write().unwrap_or_else(|e| e.into_inner()) = Some(report.clone());
    set(None);
    report
}

/// Where the scan running now is, if one is.
pub fn scan_progress() -> Option<scan::Progress> {
    PROGRESS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What the last scan found, once one has finished.
pub fn last_report() -> Option<scan::Report> {
    REPORT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

static DISABLED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// The key a plugin is enabled or disabled by: its file and its id.
pub fn enable_key(info: &PluginInfo) -> String {
    format!("{}|{}", info.path.display(), info.id)
}

/// The plugins the user turned off (by [`enable_key`]): left out of [`available`].
pub fn set_disabled(keys: Vec<String>) {
    *DISABLED.write().unwrap_or_else(|e| e.into_inner()) = keys;
}

pub fn is_enabled(info: &PluginInfo) -> bool {
    let key = enable_key(info);
    !DISABLED
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&key)
}

/// The plugins found that the user has not turned off: what the menus offer. (A
/// project that uses a turned off plugin still plays it: see [`find`].)
pub fn available() -> Vec<PluginInfo> {
    known().into_iter().filter(is_enabled).collect()
}

/// Remember `list` as the plugins there are (for tests, and hosts that scan
/// themselves).
pub fn set_known(list: Vec<PluginInfo>) {
    *KNOWN.write().unwrap_or_else(|e| e.into_inner()) = list;
}

/// The plugins the last [`rescan`] found.
pub fn known() -> Vec<PluginInfo> {
    KNOWN.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A project never says where a library is: that would let a project file make this
/// program load any file. A plugin is found by its id among the ones found by a scan.
pub fn find(id: &str) -> Option<PluginInfo> {
    known().into_iter().find(|p| p.id == id)
}

/// The state a project keeps for a hosted plugin, as the engine reads it.
pub fn plugin_state(id: &str, params: &[(u32, f64)]) -> Value {
    let params: serde_json::Map<String, Value> = params
        .iter()
        .map(|&(id, v)| (id.to_string(), json!(v)))
        .collect();
    json!({ "id": id, "params": params })
}

/// Make the engine run hosted plugins.
pub fn install() {
    set_plugin_host(Some(Arc::new(Hosting)));
}

/// The whole state a project keeps for a plugin (a VST2 chunk), when it keeps one.
pub fn saved_state(state: &Value) -> Option<Vec<u8>> {
    decode_base64(state.get("chunk")?.as_str()?)
}

/// Keep a plugin's whole state in its project state.
pub fn set_saved_state(state: &mut Value, bytes: &[u8]) {
    if !state.is_object() {
        *state = json!({});
    }
    state["chunk"] = json!(encode_base64(bytes));
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Bytes as base64 text (for a project file).
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 text back as bytes; `None` when it is not base64.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for ch in text.bytes() {
        let v = B64.iter().position(|&b| b == ch)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The plugin a state names, loaded and set to the state and parameters it holds.
fn open(state: &Value, sr: f64) -> Option<Plugin> {
    let info = find(state.get("id")?.as_str()?)?;
    let mut clap = Plugin::load(&info, sr).ok()?;
    if let Some(bytes) = saved_state(state) {
        let _ = clap.load_state(&bytes);
    }
    if let Some(ps) = state.get("params").and_then(Value::as_object) {
        for (k, v) in ps {
            if let (Ok(id), Some(v)) = (k.parse::<u32>(), v.as_f64()) {
                clap.set_param(id, v);
            }
        }
    }
    Some(clap)
}

struct Hosting;

impl PluginHost for Hosting {
    fn instrument(&self, state: &Value, sr: f64) -> Option<Box<dyn Instrument>> {
        open(state, sr).map(|p| Box::new(Automated::new(p, sr)) as Box<dyn Instrument>)
    }

    fn effect(&self, state: &Value, sr: f64) -> Option<Box<dyn External>> {
        open(state, sr).map(|p| Box::new(Automated::new(p, sr)) as Box<dyn External>)
    }
}

/// A hosted plugin with the automation lines of its parameters.
struct Automated {
    plugin: Plugin,
    lines: ParamLines,
    sr: f64,
    /// Reused each block, so the audio thread does not allocate.
    msgs: Vec<NoteMsg>,
    due: Vec<(u32, f64)>,
}

impl Automated {
    fn new(plugin: Plugin, sr: f64) -> Self {
        Self {
            plugin,
            lines: ParamLines::default(),
            sr,
            msgs: Vec::with_capacity(128),
            due: Vec::with_capacity(16),
        }
    }

    fn has(&self, id: u32) -> bool {
        self.plugin.params().iter().any(|p| p.id == id)
    }

    /// Send the parameters whose lines moved, at `t` seconds.
    fn follow(&mut self, t: f64) {
        let mut due = std::mem::take(&mut self.due);
        self.lines.due_into(t, &mut due);
        for &(id, v) in &due {
            if let Some(p) = self.plugin.params().iter().find(|p| p.id == id) {
                let plain = p.min + v.clamp(0.0, 1.0) * (p.max - p.min);
                self.plugin.set_param(id, plain);
            }
        }
        self.due = due;
    }
}

impl Instrument for Automated {
    fn process(&mut self, t0: f64, notes: &[Note], l: &mut [f32], r: &mut [f32]) {
        self.follow(t0);
        self.msgs.clear();
        self.msgs.extend(notes.iter().map(|n| match *n {
            Note::On { offset, key, vel } => NoteMsg::On { offset, key, vel },
            Note::Off { offset, key } => NoteMsg::Off { offset, key },
        }));
        self.plugin.process(None, &self.msgs, l, r);
    }

    fn automate(&mut self, id: u32, points: &[(f64, f64)], first: bool) -> bool {
        if !self.has(id) {
            return false;
        }
        self.lines.automate(id, points, first);
        true
    }
}

impl External for Automated {
    fn process(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        _sidechain: Option<(&[f32], &[f32])>,
    ) {
        self.plugin.process(Some((in_l, in_r)), &[], out_l, out_r);
    }

    fn set_time(&mut self, frame: usize) {
        let t = frame as f64 / self.sr;
        self.follow(t);
    }

    fn automate(&mut self, path: &str, points: &[(f64, f64)], first: bool) -> bool {
        let Some(id) = path.strip_prefix("p.").and_then(|i| i.parse::<u32>().ok()) else {
            return false;
        };
        Instrument::automate(self, id, points, first)
    }
}

/// Make the processor treat denormal numbers as zero on the calling thread (the audio
/// thread): the tails of filters and reverbs decay into them, and they are many times
/// slower to compute. Hosts do this for the plugins they run, too.
pub fn flush_denormals() {
    #[cfg(target_arch = "x86_64")]
    {
        let mut csr: u32 = 0;
        // SAFETY: reads and writes this thread's floating point control register only;
        // FTZ (bit 15) and DAZ (bit 6) change nothing but how tiny numbers round
        unsafe {
            std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack));
            csr |= 0x8040;
            std::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack));
        }
    }
}

/// The `pluginId` a channel or an effect slot has when it is a hosted plugin.
pub const HOSTED: &str = PLUGIN_ID;

static PARAMS: RwLock<Vec<(String, Arc<Vec<Param>>)>> = RwLock::new(Vec::new());

/// The parameters of a known plugin. The first call for a plugin loads it to ask; the
/// answer is kept.
pub fn params_of(id: &str) -> Arc<Vec<Param>> {
    let cached = PARAMS.read().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = cached.iter().find(|(i, _)| i == id) {
        return p.clone();
    }
    drop(cached);
    let list = find(id)
        .and_then(|info| Plugin::load(&info, 44_100.0).ok())
        .map(|c| c.params().to_vec())
        .unwrap_or_default();
    let list = Arc::new(list);
    PARAMS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .push((id.to_string(), list.clone()));
    list
}

/// A plugin's whole state and the value of each of its parameters.
pub type WholeState = (Vec<u8>, Vec<(u32, f64)>);

/// A hosted plugin's own window, open on the app's thread.
pub enum Editor {
    Clap(clap_editor::ClapEditor),
    Vst3(vst3::Vst3Editor),
    Vst2(vst2::Vst2Editor),
}

impl Editor {
    /// Open the window of the plugin `info`, showing `params` (the project's values).
    pub fn open(info: &PluginInfo, params: &[(u32, f64)], title: &str) -> Result<Self, String> {
        Self::open_with_state(info, None, params, title)
    }

    /// [`Editor::open`], first set to a whole state the project keeps (see
    /// [`Editor::state`]).
    pub fn open_with_state(
        info: &PluginInfo,
        state: Option<&[u8]>,
        params: &[(u32, f64)],
        title: &str,
    ) -> Result<Self, String> {
        match info.format {
            Format::Vst3 => vst3::Vst3Editor::open(info, params, title).map(Self::Vst3),
            Format::Clap => clap_editor::ClapEditor::open(info, params, title).map(Self::Clap),
            Format::Vst2 => vst2::Vst2Editor::open(info, state, params, title).map(Self::Vst2),
        }
    }

    /// The window's whole state and every parameter's value, for plugins whose state
    /// is more than their parameters (VST2: a preset chosen in the window).
    pub fn state(&mut self) -> Option<WholeState> {
        match self {
            Self::Vst2(e) => Some(e.state()),
            _ => None,
        }
    }

    /// Serve the window; what the user changed in it since the last call.
    pub fn idle(&mut self) -> Vec<(u32, f64)> {
        match self {
            Self::Clap(e) => e.idle(),
            Self::Vst3(e) => e.idle(),
            Self::Vst2(e) => e.idle(),
        }
    }

    /// Show a value that changed elsewhere.
    pub fn set_param(&mut self, id: u32, value: f64) {
        match self {
            Self::Clap(e) => e.set_param(id, value),
            Self::Vst3(e) => e.set_param(id, value),
            Self::Vst2(e) => e.set_param(id, value),
        }
    }

    /// Whether the user closed the window.
    pub fn closed(&self) -> bool {
        match self {
            Self::Clap(e) => e.closed(),
            Self::Vst3(e) => e.closed(),
            Self::Vst2(e) => e.closed(),
        }
    }
}
