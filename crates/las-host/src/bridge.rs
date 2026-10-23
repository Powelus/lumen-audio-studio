// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugins in their own process. A plugin's code is outside this program's control: if
//! it crashes or hangs inside the app, it takes the app with it. So each hosted plugin
//! is run by `lumen-plugin-host`, a small program next to the app, and [`Remote`] talks
//! to it over its standard input and output.
//!
//! Every message is a frame: its length as a `u32` and then its bytes, little endian
//! throughout. The app sends a request and waits for the frame that answers it. A
//! reader thread takes the frames off the pipe, so a plugin that hangs is given up
//! after a while instead of stopping the audio; one that crashes ends its process.
//! Either way the plugin is then silent, and [`Remote::crashed`] says so.

use crate::{Format, NoteMsg, Param, Plugin, PluginInfo};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

const QUIT: u8 = 0;
const LOAD: u8 = 1;
const PROCESS: u8 = 2;
const VALUE: u8 = 3;
const SAVE: u8 = 4;
const RESTORE: u8 = 5;

/// How long a block may take before the plugin is given up on.
const BLOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// How long loading may take.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// A frame being written.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f64(&mut self, v: f64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }
    fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    fn f32s(&mut self, s: &[f32]) -> &mut Self {
        for &v in s {
            self.f32(v);
        }
        self
    }
    fn send(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&(self.0.len() as u32).to_le_bytes())?;
        w.write_all(&self.0)?;
        w.flush()
    }
}

/// A frame being read.
struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a short frame",
            ));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }
    fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_le_bytes(
            self.take(8)?.try_into().unwrap_or_default(),
        ))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&self.bytes()?).into_owned())
    }
    fn f32s(&mut self, out: &mut [f32]) -> io::Result<()> {
        for o in out {
            *o = self.f32()?;
        }
        Ok(())
    }
}

fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// The plugin host program's side
// ---------------------------------------------------------------------------

/// Run as `lumen-plugin-host`: serve requests on standard input until it closes.
pub fn serve() -> io::Result<()> {
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    let mut plugin: Option<Plugin> = None;
    let (mut l, mut r, mut il, mut ir) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    loop {
        let Ok(frame) = read_frame(&mut input) else {
            return Ok(());
        };
        let mut m = In(&frame);
        let mut reply = Out::default();
        match m.u8()? {
            QUIT => return Ok(()),
            LOAD => {
                let format = match m.u8()? {
                    1 => Format::Vst3,
                    2 => Format::Vst2,
                    _ => Format::Clap,
                };
                let info = PluginInfo {
                    path: PathBuf::from(m.str()?),
                    id: m.str()?,
                    name: String::new(),
                    vendor: String::new(),
                    features: Vec::new(),
                    format,
                };
                let sr = m.f64()?;
                match Plugin::load_local(&info, sr) {
                    Ok(p) => {
                        reply.u8(1).u8(u8::from(p.takes_audio()));
                        reply.u32(p.params().len() as u32);
                        for q in p.params() {
                            reply
                                .u32(q.id)
                                .str(&q.name)
                                .f64(q.min)
                                .f64(q.max)
                                .f64(q.default);
                        }
                        plugin = Some(p);
                    }
                    Err(e) => {
                        reply.u8(0).str(&e);
                    }
                }
            }
            PROCESS => {
                let n = m.u32()? as usize;
                let with_input = m.u8()? == 1;
                for b in [&mut l, &mut r, &mut il, &mut ir] {
                    b.resize(n, 0.0);
                }
                if with_input {
                    m.f32s(&mut il)?;
                    m.f32s(&mut ir)?;
                }
                let mut notes = Vec::new();
                for _ in 0..m.u32()? {
                    let on = m.u8()? == 1;
                    let (offset, key, vel) = (m.u32()?, m.u8()?, m.f32()?);
                    notes.push(if on {
                        NoteMsg::On { offset, key, vel }
                    } else {
                        NoteMsg::Off { offset, key }
                    });
                }
                let count = m.u32()?;
                if let Some(p) = plugin.as_mut() {
                    for _ in 0..count {
                        let (id, v) = (m.u32()?, m.f64()?);
                        p.set_param(id, v);
                    }
                    let input = with_input.then_some((&il[..], &ir[..]));
                    p.process(input, &notes, &mut l, &mut r);
                } else {
                    l.fill(0.0);
                    r.fill(0.0);
                }
                reply.f32s(&l).f32s(&r);
            }
            VALUE => {
                let id = m.u32()?;
                match plugin.as_mut().and_then(|p| p.param_value(id)) {
                    Some(v) => reply.u8(1).f64(v),
                    None => reply.u8(0),
                };
            }
            SAVE => {
                match plugin.as_mut().and_then(Plugin::save_state) {
                    Some(b) => reply.u8(1).bytes(&b),
                    None => reply.u8(0),
                };
            }
            RESTORE => {
                let b = m.bytes()?;
                match plugin.as_mut().map(|p| p.load_state(&b)) {
                    Some(Ok(())) => reply.u8(1),
                    Some(Err(e)) => reply.u8(0).str(&e),
                    None => reply.u8(0).str("no plugin"),
                };
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown request",
                ))
            }
        }
        reply.send(&mut output)?;
    }
}

// ---------------------------------------------------------------------------
// The app's side
// ---------------------------------------------------------------------------

/// The plugin host program: `LUMEN_PLUGIN_HOST`, or `lumen-plugin-host` next to this
/// program (or one folder up, where the tests run from).
pub fn host_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LUMEN_PLUGIN_HOST") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let name = format!("lumen-plugin-host{}", std::env::consts::EXE_SUFFIX);
    exe.ancestors()
        .skip(1)
        .take(2)
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

/// Whether plugins run in their own process: when the host program is there, unless
/// `LUMEN_PLUGINS_IN_PROCESS` is set.
pub fn sandboxed() -> bool {
    std::env::var_os("LUMEN_PLUGINS_IN_PROCESS").is_none() && host_exe().is_some()
}

/// A plugin running in its own process.
pub struct Remote {
    info: PluginInfo,
    params: Vec<Param>,
    takes_audio: bool,
    child: Child,
    to: BufWriter<ChildStdin>,
    from: Receiver<Vec<u8>>,
    pending: Vec<(u32, f64)>,
    crashed: bool,
}

impl Remote {
    pub fn load(info: &PluginInfo, sample_rate: f64) -> Result<Self, String> {
        let exe = host_exe().ok_or("the plugin host program is missing")?;
        Self::load_with(&exe, info, sample_rate)
    }

    /// Load with a given host program.
    pub fn load_with(
        exe: &std::path::Path,
        info: &PluginInfo,
        sample_rate: f64,
    ) -> Result<Self, String> {
        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("the plugin host program did not start: {e}"))?;
        let to = BufWriter::new(child.stdin.take().ok_or("no pipe to the plugin host")?);
        let mut out = BufReader::new(child.stdout.take().ok_or("no pipe from the plugin host")?);
        let (tx, from) = mpsc::channel();
        std::thread::Builder::new()
            .name("plugin host reader".into())
            .spawn(move || {
                while let Ok(f) = read_frame(&mut out) {
                    if tx.send(f).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let mut me = Self {
            info: info.clone(),
            params: Vec::new(),
            takes_audio: false,
            child,
            to,
            from,
            pending: Vec::new(),
            crashed: false,
        };
        let mut req = Out::default();
        req.u8(LOAD)
            .u8(match info.format {
                Format::Clap => 0,
                Format::Vst3 => 1,
                Format::Vst2 => 2,
            })
            .str(&info.path.to_string_lossy())
            .str(&info.id)
            .f64(sample_rate);
        let frame = me
            .ask(&req, LOAD_TIMEOUT)
            .ok_or("the plugin host program stopped while loading the plugin")?;
        let mut m = In(&frame);
        let parse = |m: &mut In| -> io::Result<Result<(bool, Vec<Param>), String>> {
            if m.u8()? != 1 {
                return Ok(Err(m.str()?));
            }
            let audio = m.u8()? == 1;
            let mut params = Vec::new();
            for _ in 0..m.u32()? {
                params.push(Param {
                    id: m.u32()?,
                    name: m.str()?,
                    min: m.f64()?,
                    max: m.f64()?,
                    default: m.f64()?,
                });
            }
            Ok(Ok((audio, params)))
        };
        let (audio, params) = parse(&mut m).map_err(|e| e.to_string())??;
        me.takes_audio = audio;
        me.params = params;
        Ok(me)
    }

    /// Send a request and wait for its answer; `None` when the plugin is gone.
    fn ask(&mut self, req: &Out, wait: Duration) -> Option<Vec<u8>> {
        if self.crashed {
            return None;
        }
        if req.send(&mut self.to).is_err() {
            self.give_up();
            return None;
        }
        match self.from.recv_timeout(wait) {
            Ok(f) => Some(f),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                self.give_up();
                None
            }
        }
    }

    /// The plugin crashed or hangs: stop its process; it is silent from now on.
    fn give_up(&mut self) {
        if !self.crashed {
            self.crashed = true;
            eprintln!(
                "the plugin {} stopped answering; it is now silent",
                self.info.id
            );
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Whether the plugin crashed (or hung) and is now silent.
    pub fn crashed(&self) -> bool {
        self.crashed
    }

    /// The id of the plugin's process.
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    pub fn params(&self) -> &[Param] {
        &self.params
    }

    pub fn takes_audio(&self) -> bool {
        self.takes_audio
    }

    pub fn param_value(&mut self, id: u32) -> Option<f64> {
        let mut req = Out::default();
        req.u8(VALUE).u32(id);
        let f = self.ask(&req, BLOCK_TIMEOUT)?;
        let mut m = In(&f);
        (m.u8().ok()? == 1).then(|| m.f64().ok()).flatten()
    }

    pub fn set_param(&mut self, id: u32, value: f64) {
        self.pending.push((id, value));
    }

    pub fn save_state(&mut self) -> Option<Vec<u8>> {
        let mut req = Out::default();
        req.u8(SAVE);
        let f = self.ask(&req, LOAD_TIMEOUT)?;
        let mut m = In(&f);
        (m.u8().ok()? == 1).then(|| m.bytes().ok()).flatten()
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut req = Out::default();
        req.u8(RESTORE).bytes(bytes);
        let f = self
            .ask(&req, LOAD_TIMEOUT)
            .ok_or("the plugin stopped answering")?;
        let mut m = In(&f);
        match m.u8() {
            Ok(1) => Ok(()),
            _ => Err(m.str().unwrap_or_default()),
        }
    }

    /// The same contract as [`crate::Clap::process`]; silence once the plugin is gone.
    pub fn process(
        &mut self,
        input: Option<(&[f32], &[f32])>,
        notes: &[NoteMsg],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = out_l.len().min(out_r.len());
        let mut req = Out::default();
        req.u8(PROCESS).u32(n as u32);
        match input {
            Some((l, r)) => {
                req.u8(1).f32s(&l[..n]).f32s(&r[..n]);
            }
            None => {
                req.u8(0);
            }
        }
        req.u32(notes.len() as u32);
        for m in notes {
            match *m {
                NoteMsg::On { offset, key, vel } => req.u8(1).u32(offset).u8(key).f32(vel),
                NoteMsg::Off { offset, key } => req.u8(0).u32(offset).u8(key).f32(0.0),
            };
        }
        req.u32(self.pending.len() as u32);
        for (id, v) in self.pending.drain(..) {
            req.u32(id).f64(v);
        }
        let answer = self.ask(&req, BLOCK_TIMEOUT);
        let ok = answer.is_some_and(|f| {
            let mut m = In(&f);
            m.f32s(&mut out_l[..n]).is_ok() && m.f32s(&mut out_r[..n]).is_ok()
        });
        if !ok {
            out_l.fill(0.0);
            out_r.fill(0.0);
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        if !self.crashed {
            let mut req = Out::default();
            req.u8(QUIT);
            let _ = req.send(&mut self.to);
            // give it a moment to end the plugin cleanly, then make sure it is gone
            for _ in 0..50 {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
