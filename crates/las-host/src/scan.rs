// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding plugins: the folders they are installed in on each system, the files in
//! them that are plugins (CLAP, VST3, VST2), and what each one is.
//!
//! Asking a library what it is runs its code, and a plugin can crash or hang while it
//! starts. So each file is asked in its own process (`lumen-plugin-host --describe`)
//! when that program is there, and given up on after [`DESCRIBE_TIMEOUT`]. What is
//! learnt is cached by the file's path, time and size, so a rescan only opens what is
//! new or changed. Files that are not usable are reported with the reason (32-bit, made
//! for another system, not a plugin, failed to load), so a user can see why a plugin
//! is missing.

use crate::{Format, PluginInfo};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How deep below a plugin folder plugins are looked for (vendor folders, product
/// folders, version folders...).
pub const MAX_DEPTH: usize = 6;

/// How long one file may take to say what it is.
pub const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

/// A kind of system, for the folders and file names plugins have on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    /// The system this program runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Linux
        }
    }
}

/// The standard plugin folders of a system, given its environment (`env` reads a
/// variable). `CLAP_PATH`, `VST3_PATH` and `VST_PATH` come first.
pub fn default_paths(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for var in ["CLAP_PATH", "VST3_PATH", "VST_PATH"] {
        if let Some(v) = env(var) {
            let sep = if os == Os::Windows { ';' } else { ':' };
            paths.extend(
                v.to_string_lossy()
                    .split(sep)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
            );
        }
    }
    let var = |k: &str| env(k).map(PathBuf::from);
    match os {
        Os::Windows => {
            if let Some(c) = var("COMMONPROGRAMFILES") {
                paths.push(c.join("VST3"));
                paths.push(c.join("CLAP"));
                paths.push(c.join("VST2"));
            }
            if let Some(p) = var("PROGRAMFILES") {
                paths.push(p.join("VSTPlugins"));
                paths.push(p.join("Steinberg").join("VSTPlugins"));
                paths.push(p.join("Common Files").join("VST2"));
                paths.push(p.join("VstPlugins"));
            }
            // 32-bit plugins live here: they are found to say they cannot be used
            if let Some(p) = var("PROGRAMFILES(X86)") {
                paths.push(p.join("VSTPlugins"));
                paths.push(p.join("Steinberg").join("VSTPlugins"));
            }
            if let Some(l) = var("LOCALAPPDATA") {
                paths.push(l.join("Programs").join("Common").join("VST3"));
                paths.push(l.join("Programs").join("Common").join("CLAP"));
            }
        }
        Os::Mac => {
            let lib = PathBuf::from("/Library/Audio/Plug-Ins");
            let home = var("HOME").map(|h| h.join("Library/Audio/Plug-Ins"));
            for kind in ["VST3", "CLAP", "VST"] {
                if let Some(h) = &home {
                    paths.push(h.join(kind));
                }
                paths.push(lib.join(kind));
            }
        }
        Os::Linux => {
            let home = var("HOME");
            for kind in [".vst3", ".clap", ".vst", ".lxvst"] {
                if let Some(h) = &home {
                    paths.push(h.join(kind));
                }
            }
            for kind in ["vst3", "clap", "vst", "lxvst"] {
                paths.push(PathBuf::from("/usr/lib").join(kind));
                paths.push(PathBuf::from("/usr/local/lib").join(kind));
            }
        }
    }
    let mut seen = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
    paths
}

/// The standard plugin folders of this system.
pub fn system_paths() -> Vec<PathBuf> {
    default_paths(Os::current(), &|k| std::env::var_os(k))
}

/// The format of a file or bundle found in a plugin folder, if it is a plugin's.
pub fn format_of(path: &Path, os: Os, is_dir: bool) -> Option<Format> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "clap" => Some(Format::Clap),
        "vst3" => Some(Format::Vst3),
        "vst" if os == Os::Mac && is_dir => Some(Format::Vst2),
        "dll" if os == Os::Windows && !is_dir => Some(Format::Vst2),
        "so" if os == Os::Linux && !is_dir => Some(Format::Vst2),
        _ => None,
    }
}

/// Folders that are some other program's or kind's bundle: never looked into.
fn is_foreign_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|x| {
        let x = x.to_string_lossy().to_ascii_lowercase();
        matches!(
            x.as_str(),
            "component" | "app" | "bundle" | "framework" | "aaxplugin" | "vst" | "lv2"
        )
    })
}

/// The plugin files and bundles under `dir`, down to [`MAX_DEPTH`] folders, for `os`.
/// A folder reached twice (through a link) is looked at once.
pub fn collect(dir: &Path, os: Os, out: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    walk(dir, os, 0, &mut seen, out);
}

fn walk(dir: &Path, os: Os, depth: usize, seen: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(real) {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let is_dir = p.is_dir();
        if format_of(&p, os, is_dir).is_some() {
            out.push(p);
        } else if is_dir && depth + 1 < MAX_DEPTH && !is_foreign_bundle(&p) {
            walk(&p, os, depth + 1, seen, out);
        }
    }
}

/// Every plugin file in `dirs`, sorted, each once.
pub fn collect_all(dirs: &[PathBuf], os: Os) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for d in dirs {
        walk(d, os, 0, &mut seen, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

// ---------------------------------------------------------------------------
// What a binary is made for
// ---------------------------------------------------------------------------

/// A processor a binary is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86,
    X86_64,
    Arm64,
    Other,
}

impl Arch {
    pub fn current() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::Arm64
        } else if cfg!(target_arch = "x86") {
            Self::X86
        } else {
            Self::Other
        }
    }
}

/// What the first bytes of a library say it is: its system and processor, and
/// whether it is 64-bit. `None` when it is not a library this knows.
pub fn binary_kind(head: &[u8]) -> Option<(Os, Arch, bool)> {
    let u16le = |at: usize| {
        head.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32le = |at: usize| {
        head.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if head.starts_with(b"MZ") {
        let pe = u32le(0x3c)? as usize;
        if head.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        let (arch, wide) = match u16le(pe + 4)? {
            0x014c => (Arch::X86, false),
            0x8664 => (Arch::X86_64, true),
            0xaa64 => (Arch::Arm64, true),
            _ => (Arch::Other, true),
        };
        return Some((Os::Windows, arch, wide));
    }
    if head.starts_with(b"\x7fELF") {
        let wide = *head.get(4)? == 2;
        let arch = match u16le(18)? {
            3 => Arch::X86,
            62 => Arch::X86_64,
            183 => Arch::Arm64,
            _ => Arch::Other,
        };
        return Some((Os::Linux, arch, wide));
    }
    match u32le(0)? {
        0xfeed_face => Some((Os::Mac, Arch::Other, false)),
        0xfeed_facf => {
            let arch = match u32le(4)? {
                0x0100_0007 => Arch::X86_64,
                0x0100_000c => Arch::Arm64,
                _ => Arch::Other,
            };
            Some((Os::Mac, arch, true))
        }
        // a universal binary (big endian magic): it has a slice for each processor
        0xbeba_feca => Some((Os::Mac, Arch::current(), true)),
        _ => None,
    }
}

/// Why a library with these first bytes cannot run here (`os` and `arch`), if it
/// cannot.
pub fn binary_problem(head: &[u8], os: Os, arch: Arch) -> Option<String> {
    let Some((bos, barch, wide)) = binary_kind(head) else {
        return Some("not a plugin (not a library this system can load)".into());
    };
    if !wide {
        return Some(
            "32-bit, not supported (Lumen runs 64-bit plugins: install the 64-bit version)".into(),
        );
    }
    if bos != os {
        let name = match bos {
            Os::Windows => "Windows",
            Os::Mac => "macOS",
            Os::Linux => "Linux",
        };
        return Some(format!("made for {name}, not this system"));
    }
    if barch != arch && arch != Arch::Other && barch != Arch::Other {
        let name = match barch {
            Arch::Arm64 => "ARM processors",
            Arch::X86_64 => "Intel and AMD processors",
            _ => "another processor",
        };
        return Some(format!("made for {name}, not this computer's"));
    }
    None
}

/// The library inside a plugin file or bundle.
pub fn binary_of(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let is = |x: &str| path.extension().is_some_and(|e| e.eq_ignore_ascii_case(x));
    if is("vst3") {
        crate::vst3::module_binary(path)
    } else if is("vst") {
        crate::vst2::binary(path)
    } else {
        // a macOS .clap bundle
        let stem = path.file_stem()?;
        let bin = path.join("Contents/MacOS").join(stem);
        bin.is_file().then_some(bin)
    }
}

/// Why the plugin at `path` cannot be used here, judged from its binary alone.
pub fn precheck(path: &Path) -> Option<String> {
    let Some(bin) = binary_of(path) else {
        return Some(
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
            {
                format!(
                    "no {} binary in the bundle (it may be 32-bit only or made for another system)",
                    match Os::current() {
                        Os::Windows => "64-bit Windows",
                        Os::Mac => "macOS",
                        Os::Linux => "64-bit Linux",
                    }
                )
            } else {
                "no plugin binary in the bundle".into()
            },
        );
    };
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(&bin)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    head.truncate(n);
    binary_problem(&head, Os::current(), Arch::current())
}

// ---------------------------------------------------------------------------
// Asking a file what it is
// ---------------------------------------------------------------------------

/// The plugins in one file, asked in this process; or why there are none.
pub fn describe_local(path: &Path) -> Result<Vec<PluginInfo>, String> {
    if let Some(why) = precheck(path) {
        return Err(why);
    }
    let found = match format_of(path, Os::current(), path.is_dir()) {
        Some(Format::Vst3) => crate::vst3::describe(path),
        Some(Format::Vst2) => return crate::vst2::describe(path),
        _ => crate::describe(path),
    };
    if found.is_empty() {
        Err("the library loaded but has no plugins in it (or did not start)".into())
    } else {
        Ok(found)
    }
}

/// The plugins in one file, asked in a process of its own when the plugin host
/// program is there (a plugin that crashes or hangs then takes only that with it).
pub fn describe_isolated(path: &Path) -> Result<Vec<PluginInfo>, String> {
    if let Some(why) = precheck(path) {
        return Err(why);
    }
    match crate::bridge::host_exe() {
        Some(exe) if crate::bridge::sandboxed() => describe_with(&exe, path, DESCRIBE_TIMEOUT),
        _ => describe_local(path),
    }
}

/// Ask `exe --describe path`, giving up after `timeout`.
pub fn describe_with(
    exe: &Path,
    path: &Path,
    timeout: Duration,
) -> Result<Vec<PluginInfo>, String> {
    let mut child = Command::new(exe)
        .arg("--describe")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("the plugin host program did not start: {e}"))?;
    let mut out = child.stdout.take().ok_or("no pipe from the plugin host")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let until = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "took too long to load (over {} seconds), skipped",
                    timeout.as_secs()
                ));
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    let answer: Option<Value> = text
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok());
    match answer {
        Some(v) => match v.get("plugins").and_then(Value::as_array) {
            Some(list) => Ok(list.iter().filter_map(info_from_json).collect()),
            None => Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("an unknown error")
                .to_string()),
        },
        None if status.success() => Err("the plugin host program gave no answer".into()),
        None => Err("crashed while loading".into()),
    }
}

/// What `lumen-plugin-host --describe <path>` prints: one line of JSON.
pub fn describe_answer(path: &Path) -> String {
    match describe_local(path) {
        Ok(list) => json!({ "plugins": list.iter().map(info_to_json).collect::<Vec<_>>() }),
        Err(e) => json!({ "error": e }),
    }
    .to_string()
}

pub fn format_tag(f: Format) -> &'static str {
    match f {
        Format::Clap => "CLAP",
        Format::Vst3 => "VST3",
        Format::Vst2 => "VST2",
    }
}

fn format_from_tag(s: &str) -> Option<Format> {
    match s {
        "CLAP" => Some(Format::Clap),
        "VST3" => Some(Format::Vst3),
        "VST2" => Some(Format::Vst2),
        _ => None,
    }
}

pub fn info_to_json(i: &PluginInfo) -> Value {
    json!({
        "path": i.path.to_string_lossy(),
        "id": i.id,
        "name": i.name,
        "vendor": i.vendor,
        "features": i.features,
        "format": format_tag(i.format),
    })
}

pub fn info_from_json(v: &Value) -> Option<PluginInfo> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(PluginInfo {
        path: PathBuf::from(s("path")?),
        id: s("id")?,
        name: s("name")?,
        vendor: s("vendor").unwrap_or_default(),
        features: v
            .get("features")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        format: format_from_tag(&s("format")?)?,
    })
}

// ---------------------------------------------------------------------------
// The cache, and a whole scan
// ---------------------------------------------------------------------------

/// When a file last changed, and its size: what a cached answer is checked against.
/// A bundle is stamped by its binary.
pub fn stamp(path: &Path) -> String {
    let of = |p: &Path| {
        std::fs::metadata(p).ok().map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            format!("{}.{:09}:{}", t.as_secs(), t.subsec_nanos(), m.len())
        })
    };
    let bin = binary_of(path).and_then(|b| of(&b));
    format!(
        "{}|{}",
        of(path).unwrap_or_default(),
        bin.unwrap_or_default()
    )
}

/// What a scan learnt about one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub stamp: String,
    pub result: Result<Vec<PluginInfo>, String>,
}

/// What was learnt about files before, by path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cache {
    pub entries: Vec<Entry>,
}

const CACHE_VERSION: u64 = 1;

impl Cache {
    pub fn load(file: &Path) -> Self {
        let Some(v) = std::fs::read(file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            return Self::default();
        };
        // a cache another version of the program wrote is started again
        if v.get("version").and_then(Value::as_u64) != Some(CACHE_VERSION) {
            return Self::default();
        }
        let entries = v
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let path = PathBuf::from(e.get("path")?.as_str()?);
                let stamp = e.get("stamp")?.as_str()?.to_string();
                let result = match e.get("error").and_then(Value::as_str) {
                    Some(err) => Err(err.to_string()),
                    None => Ok(e
                        .get("plugins")?
                        .as_array()?
                        .iter()
                        .filter_map(info_from_json)
                        .collect()),
                };
                Some(Entry {
                    path,
                    stamp,
                    result,
                })
            })
            .collect();
        Self { entries }
    }

    pub fn save(&self, file: &Path) {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                let mut v = json!({ "path": e.path.to_string_lossy(), "stamp": e.stamp });
                match &e.result {
                    Ok(list) => v["plugins"] = list.iter().map(info_to_json).collect(),
                    Err(err) => v["error"] = json!(err),
                }
                v
            })
            .collect();
        if let Some(d) = file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let text = json!({ "version": CACHE_VERSION, "entries": entries }).to_string();
        // written whole then moved, so a crash never leaves half a cache
        let tmp = file.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }

    /// The answer kept for `path`, if the file has not changed since.
    pub fn get(&self, path: &Path, stamp: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.path == path && e.stamp == stamp)
    }
}

/// A file that has no usable plugin, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub path: PathBuf,
    pub reason: String,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub plugins: Vec<PluginInfo>,
    pub failed: Vec<Failure>,
    /// Plugin files looked at.
    pub files: usize,
    /// Of those, the ones whose answer came from the cache.
    pub cached: usize,
    /// The folders looked in.
    pub dirs: Vec<PathBuf>,
}

/// Where a scan is.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// The name of the file being asked.
    pub current: String,
}

/// Scan `dirs`: each plugin file is asked what it is with `ask` unless `cache` has the
/// answer for it as it is now. `progress` hears of each file before it is asked. The
/// cache is brought up to date (files no longer there are dropped from it).
pub fn scan_with(
    dirs: &[PathBuf],
    os: Os,
    cache: &mut Cache,
    ask: &dyn Fn(&Path) -> Result<Vec<PluginInfo>, String>,
    progress: &mut dyn FnMut(&Progress),
) -> Report {
    let files = collect_all(dirs, os);
    let mut report = Report {
        files: files.len(),
        dirs: dirs.to_vec(),
        ..Default::default()
    };
    let mut fresh = Vec::with_capacity(files.len());
    for (i, f) in files.iter().enumerate() {
        let st = stamp(f);
        let entry = match cache.get(f, &st) {
            Some(e) => {
                report.cached += 1;
                e.clone()
            }
            None => {
                progress(&Progress {
                    done: i,
                    total: files.len(),
                    current: f
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                });
                Entry {
                    path: f.clone(),
                    stamp: st,
                    result: ask(f),
                }
            }
        };
        match &entry.result {
            Ok(list) => report.plugins.extend(list.iter().cloned()),
            Err(e) => report.failed.push(Failure {
                path: f.clone(),
                reason: e.clone(),
            }),
        }
        fresh.push(entry);
    }
    // the same plugin installed twice (a VST3 in two folders) is listed once
    let mut ids = HashSet::new();
    report.plugins.retain(|p| ids.insert(p.id.clone()));
    cache.entries = fresh;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under the system's temporary folder, removed at the end.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir().join(format!("las-scan-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
        fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(files: &[PathBuf], root: &Path) -> Vec<String> {
        files
            .iter()
            .map(|f| {
                f.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    /// A tree like a Windows system's plugin folders.
    fn windows_tree(t: &Temp) {
        // a VST3 bundle, the binary inside it
        t.file(
            "VST3/Vendor/Synth.vst3/Contents/x86_64-win/Synth.vst3",
            b"MZ",
        );
        t.file("VST3/Vendor/Synth.vst3/Contents/Resources/x.dll", b"MZ");
        // a VST3 as a single file
        t.file("VST3/Old.vst3", b"MZ");
        // VST2 libraries in vendor folders, deep
        t.file("VSTPlugins/LennarDigital/Sylenth1/Sylenth1.dll", b"MZ");
        t.file("VSTPlugins/A/B/C/D/Deep.dll", b"MZ");
        t.file("VSTPlugins/A/B/C/D/E/F/TooDeep.dll", b"MZ");
        t.file("VSTPlugins/readme.txt", b"hi");
        t.file("CLAP/Vendor/Thing.clap", b"MZ");
    }

    #[test]
    fn collection_finds_bundles_files_and_vst2_libraries_in_vendor_folders() {
        let t = Temp::new("tree");
        windows_tree(&t);
        let files = collect_all(std::slice::from_ref(&t.0), Os::Windows);
        assert_eq!(
            names(&files, &t.0),
            vec![
                "CLAP/Vendor/Thing.clap",
                "VST3/Old.vst3",
                "VST3/Vendor/Synth.vst3",
                "VSTPlugins/A/B/C/D/Deep.dll",
                "VSTPlugins/LennarDigital/Sylenth1/Sylenth1.dll",
            ],
            "a bundle is one plugin and is not looked into; too deep is left out"
        );
        assert_eq!(format_of(&files[2], Os::Windows, true), Some(Format::Vst3));
        assert_eq!(format_of(&files[4], Os::Windows, false), Some(Format::Vst2));
        // on Linux a .dll is no plugin, a .so is
        let linux = collect_all(std::slice::from_ref(&t.0), Os::Linux);
        assert!(linux.iter().all(|f| f.extension().unwrap() != "dll"));
        t.file("vst/Synth.so", b"\x7fELF");
        let linux = collect_all(std::slice::from_ref(&t.0), Os::Linux);
        assert!(names(&linux, &t.0).contains(&"vst/Synth.so".to_string()));
    }

    #[test]
    fn mac_vst_bundles_are_plugins_and_other_bundles_are_skipped() {
        let t = Temp::new("mac");
        t.file("VST/Synth.vst/Contents/MacOS/Synth", b"");
        t.file("VST/Synth.component/Contents/MacOS/x.vst3", b"");
        t.file("CLAP/Synth.clap/Contents/MacOS/Synth", b"");
        let files = collect_all(std::slice::from_ref(&t.0), Os::Mac);
        assert_eq!(
            names(&files, &t.0),
            vec!["CLAP/Synth.clap", "VST/Synth.vst"]
        );
        assert_eq!(format_of(&files[1], Os::Mac, true), Some(Format::Vst2));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_up_the_tree_is_followed_once() {
        let t = Temp::new("loop");
        t.file("VST3/Vendor/A.vst3", b"");
        std::os::unix::fs::symlink(&t.0, t.0.join("VST3/Vendor/again")).unwrap();
        let files = collect_all(std::slice::from_ref(&t.0), Os::Linux);
        assert_eq!(names(&files, &t.0), vec!["VST3/Vendor/A.vst3"]);
    }

    #[test]
    fn default_folders_on_each_system() {
        let env = |k: &str| -> Option<OsString> {
            match k {
                "COMMONPROGRAMFILES" => Some(r"C:\Program Files\Common Files".into()),
                "PROGRAMFILES" => Some(r"C:\Program Files".into()),
                "LOCALAPPDATA" => Some(r"C:\Users\u\AppData\Local".into()),
                "HOME" => Some("/home/u".into()),
                "VST_PATH" => Some("/opt/a:/opt/b".into()),
                _ => None,
            }
        };
        let win: Vec<String> = default_paths(Os::Windows, &env)
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        for want in [
            "C:/Program Files/Common Files/VST3",
            "C:/Program Files/Common Files/CLAP",
            "C:/Program Files/VSTPlugins",
            "C:/Program Files/Steinberg/VSTPlugins",
            "C:/Program Files/Common Files/VST2",
            "C:/Program Files/VstPlugins",
            "C:/Users/u/AppData/Local/Programs/Common/VST3",
        ] {
            assert!(win.iter().any(|w| w == want), "{want} in {win:?}");
        }
        // the list has each folder once
        let mut d = win.clone();
        d.dedup();
        assert_eq!(d.len(), win.len());

        let mac = default_paths(Os::Mac, &env);
        for want in [
            "/Library/Audio/Plug-Ins/VST",
            "/Library/Audio/Plug-Ins/VST3",
            "/Library/Audio/Plug-Ins/CLAP",
            "/home/u/Library/Audio/Plug-Ins/VST",
        ] {
            assert!(mac.contains(&PathBuf::from(want)), "{want}");
        }
        let linux = default_paths(Os::Linux, &env);
        for want in [
            "/opt/a",
            "/opt/b",
            "/home/u/.vst",
            "/home/u/.vst3",
            "/home/u/.clap",
            "/usr/lib/vst",
            "/usr/lib/vst3",
        ] {
            assert!(linux.contains(&PathBuf::from(want)), "{want}");
        }
        assert_eq!(
            linux[0],
            PathBuf::from("/opt/a"),
            "the variable comes first"
        );
    }

    fn pe(machine: u16) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        b
    }

    fn elf(class: u8, machine: u16) -> Vec<u8> {
        let mut b = vec![0u8; 64];
        b[0..4].copy_from_slice(b"\x7fELF");
        b[4] = class;
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        b
    }

    #[test]
    fn a_binary_says_what_it_is_made_for() {
        let here = (Os::Windows, Arch::X86_64);
        assert_eq!(binary_problem(&pe(0x8664), here.0, here.1), None);
        let p32 = binary_problem(&pe(0x014c), here.0, here.1).unwrap();
        assert!(p32.starts_with("32-bit, not supported"), "{p32}");
        assert!(binary_problem(&pe(0xaa64), here.0, here.1)
            .unwrap()
            .contains("ARM"));
        assert!(binary_problem(b"hello there", here.0, here.1)
            .unwrap()
            .starts_with("not a plugin"));
        // a Windows plugin on Linux, a 32-bit Linux one
        assert!(binary_problem(&pe(0x8664), Os::Linux, Arch::X86_64)
            .unwrap()
            .contains("Windows"));
        assert_eq!(binary_problem(&elf(2, 62), Os::Linux, Arch::X86_64), None);
        assert!(binary_problem(&elf(1, 3), Os::Linux, Arch::X86_64)
            .unwrap()
            .starts_with("32-bit"));
    }

    #[test]
    fn a_scan_reports_why_files_failed_and_uses_its_cache_until_a_file_changes() {
        let t = Temp::new("cache");
        let ok = t.file("VST3/Good.vst3", b"x");
        let bad = t.file("VST3/Bad.vst3", b"x");
        let asked = std::cell::RefCell::new(Vec::new());
        let ask = |p: &Path| -> Result<Vec<PluginInfo>, String> {
            asked.borrow_mut().push(p.to_path_buf());
            if p == ok {
                Ok(vec![PluginInfo {
                    path: p.to_path_buf(),
                    id: "good".into(),
                    name: "Good".into(),
                    vendor: "V".into(),
                    features: vec!["instrument".into()],
                    format: Format::Vst3,
                }])
            } else {
                Err("32-bit, not supported".into())
            }
        };
        let dirs = vec![t.0.clone()];
        let cache_file = t.0.join("cache.json");
        let mut cache = Cache::load(&cache_file);
        let mut seen = Vec::new();
        let r = scan_with(&dirs, Os::Linux, &mut cache, &ask, &mut |p| {
            seen.push(p.clone())
        });
        assert_eq!(r.files, 2);
        assert_eq!(r.cached, 0);
        assert_eq!(r.plugins.len(), 1);
        assert_eq!(
            r.failed,
            vec![Failure {
                path: bad.clone(),
                reason: "32-bit, not supported".into()
            }]
        );
        assert_eq!(seen.len(), 2);
        assert_eq!((seen[0].done, seen[0].total), (0, 2));
        assert_eq!(seen[0].current, "Bad");
        cache.save(&cache_file);

        // a rescan opens nothing
        asked.borrow_mut().clear();
        let mut cache = Cache::load(&cache_file);
        assert_eq!(cache.entries.len(), 2);
        let again = scan_with(&dirs, Os::Linux, &mut cache, &ask, &mut |_| {});
        assert!(asked.borrow().is_empty());
        assert_eq!(again.cached, 2);
        assert_eq!(again.plugins, r.plugins);
        assert_eq!(again.failed, r.failed);

        // a changed file is asked again; a removed one leaves the cache
        let f = std::fs::File::options().write(true).open(&ok).unwrap();
        f.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1000))
            .unwrap();
        drop(f);
        std::fs::remove_file(&bad).unwrap();
        let third = scan_with(&dirs, Os::Linux, &mut cache, &ask, &mut |_| {});
        assert_eq!(*asked.borrow(), vec![ok.clone()]);
        assert_eq!((third.files, third.cached), (1, 0));
        assert_eq!(cache.entries.len(), 1);

        // a cache of another version is not used
        std::fs::write(
            &cache_file,
            r#"{"version":0,"entries":[{"path":"a","stamp":"b","plugins":[]}]}"#,
        )
        .unwrap();
        assert!(Cache::load(&cache_file).entries.is_empty());
    }

    #[test]
    fn plugin_info_survives_json() {
        let i = PluginInfo {
            path: "/a/b.dll".into(),
            id: "vst2:4c547632:b".into(),
            name: "B".into(),
            vendor: "V".into(),
            features: vec!["instrument".into(), "audio-effect".into()],
            format: Format::Vst2,
        };
        assert_eq!(info_from_json(&info_to_json(&i)), Some(i));
    }

    #[cfg(unix)]
    fn script(t: &Temp, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = t.file(name, format!("#!/bin/sh\n{body}\n").as_bytes());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn a_plugin_that_hangs_or_crashes_while_described_is_reported_not_fatal() {
        let t = Temp::new("isolated");
        let plug = t.file("x.so", b"");
        let hang = script(&t, "hang.sh", "sleep 10");
        let e = describe_with(&hang, &plug, Duration::from_millis(300)).unwrap_err();
        assert!(e.starts_with("took too long"), "{e}");
        let crash = script(&t, "crash.sh", "kill -SEGV $$");
        assert_eq!(
            describe_with(&crash, &plug, DESCRIBE_TIMEOUT).unwrap_err(),
            "crashed while loading"
        );
        let says = script(&t, "says.sh", r#"echo '{"error":"not a plugin"}'"#);
        assert_eq!(
            describe_with(&says, &plug, DESCRIBE_TIMEOUT).unwrap_err(),
            "not a plugin"
        );
    }
}
