// SPDX-License-Identifier: GPL-3.0-or-later
//! A VST3 plugin's own window. The engine runs its own instance of a plugin on the
//! audio thread, so the window is shown by a second instance on the app's thread:
//! its component is given the parameters the project holds, its controller makes the
//! view, and what the user turns in the view comes back from [`Vst3Editor::idle`] as
//! parameter changes for the project, which the playing instance then follows.

use super::{finish, from_hex, instantiate, wstr, HostApp, MemStream, Module};
use crate::PluginInfo;
use std::ffi::c_char;
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use vst3::Steinberg::Linux::{IEventHandlerTrait, ITimerHandlerTrait};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};

/// What the controller tells the host about edits in the view.
struct Handler {
    edits: Mutex<Vec<(u32, f64)>>,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        let mut e = self.edits.lock().unwrap_or_else(|e| e.into_inner());
        // a drag sends many values; the last one of each parameter is what counts
        e.retain(|&(i, _)| i != id);
        e.push((id, value));
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

/// The frame a view lives in: it asks for new sizes, and on Linux it is also the run
/// loop the view's file descriptors and timers are served from.
struct Frame {
    resize: Mutex<Option<(u32, u32)>>,
    #[cfg(target_os = "linux")]
    fds: Mutex<Vec<(ComPtr<Linux::IEventHandler>, i32)>>,
    #[cfg(target_os = "linux")]
    timers: Mutex<Vec<Timer>>,
}

#[cfg(target_os = "linux")]
struct Timer {
    handler: ComPtr<Linux::ITimerHandler>,
    every: std::time::Duration,
    next: std::time::Instant,
}

#[cfg(target_os = "linux")]
impl Class for Frame {
    type Interfaces = (IPlugFrame, Linux::IRunLoop);
}

#[cfg(not(target_os = "linux"))]
impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, _view: *mut IPlugView, size: *mut ViewRect) -> tresult {
        if size.is_null() {
            return kInvalidArgument;
        }
        let r = &*size;
        *self.resize.lock().unwrap_or_else(|e| e.into_inner()) = Some((
            (r.right - r.left).max(1) as u32,
            (r.bottom - r.top).max(1) as u32,
        ));
        kResultOk
    }
}

#[cfg(target_os = "linux")]
impl Linux::IRunLoopTrait for Frame {
    unsafe fn registerEventHandler(
        &self,
        handler: *mut Linux::IEventHandler,
        fd: Linux::FileDescriptor,
    ) -> tresult {
        let Some(h) = vst3::ComRef::from_raw(handler).map(|r| r.to_com_ptr()) else {
            return kInvalidArgument;
        };
        self.fds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((h, fd));
        kResultOk
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut Linux::IEventHandler) -> tresult {
        self.fds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(h, _)| h.as_ptr() != handler);
        kResultOk
    }

    unsafe fn registerTimer(
        &self,
        handler: *mut Linux::ITimerHandler,
        ms: Linux::TimerInterval,
    ) -> tresult {
        let Some(h) = vst3::ComRef::from_raw(handler).map(|r| r.to_com_ptr()) else {
            return kInvalidArgument;
        };
        let every = std::time::Duration::from_millis(ms.max(1));
        self.timers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Timer {
                handler: h,
                every,
                next: std::time::Instant::now() + every,
            });
        kResultOk
    }

    unsafe fn unregisterTimer(&self, handler: *mut Linux::ITimerHandler) -> tresult {
        self.timers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|t| t.handler.as_ptr() != handler);
        kResultOk
    }
}

impl Frame {
    /// Serve the file descriptors that are ready and the timers that are due. The
    /// lists are copied first: a handler may register or unregister while it runs.
    #[cfg(target_os = "linux")]
    fn run(&self) {
        let fds: Vec<_> = self.fds.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !fds.is_empty() {
            let mut polls: Vec<libc::pollfd> = fds
                .iter()
                .map(|&(_, fd)| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            // SAFETY: a poll of live descriptors that does not wait
            let n = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 0) };
            if n > 0 {
                for ((h, fd), p) in fds.iter().zip(&polls) {
                    if p.revents != 0 {
                        // SAFETY: the handler was registered and is held
                        unsafe { h.onFDIsSet(*fd) };
                    }
                }
            }
        }
        let now = std::time::Instant::now();
        let due: Vec<_> = {
            let mut timers = self.timers.lock().unwrap_or_else(|e| e.into_inner());
            timers
                .iter_mut()
                .filter(|t| t.next <= now)
                .map(|t| {
                    t.next = now + t.every;
                    t.handler.clone()
                })
                .collect()
        };
        for h in due {
            // SAFETY: as above
            unsafe { h.onTimer() };
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn run(&self) {}
}

/// The platform type a view is attached as.
fn platform(kind: las_window::Kind) -> FIDString {
    match kind {
        las_window::Kind::X11 => kPlatformTypeX11EmbedWindowID,
        las_window::Kind::Win32 => kPlatformTypeHWND,
        las_window::Kind::Cocoa => kPlatformTypeNSView,
    }
}

/// A VST3 plugin's window, open on the app's thread.
pub struct Vst3Editor {
    // the order matters: the view goes first, the module last
    view: ComPtr<IPlugView>,
    window: las_window::Window,
    frame: ComWrapper<Frame>,
    handler: ComWrapper<Handler>,
    controller: ComPtr<IEditController>,
    component: ComPtr<IComponent>,
    separate: bool,
    _host: ComWrapper<HostApp>,
    _module: Module,
    names: Vec<(u32, String)>,
}

impl Vst3Editor {
    /// Open the window of the plugin `info`, set to `params` (values from 0 to 1).
    pub fn open(info: &PluginInfo, params: &[(u32, f64)], title: &str) -> Result<Self, String> {
        let module = Module::open(&info.path)?;
        let factory = module.factory()?;
        let cid = from_hex(&info.id).ok_or("a bad plugin id")?;
        let host = ComWrapper::new(HostApp);
        let host_ctx = host
            .to_com_ptr::<FUnknown>()
            .ok_or("the host object has no interface")?;
        // SAFETY: calls into the plugin in the order the VST3 specification gives; on
        // an error what was begun is ended
        unsafe {
            let (component, controller, separate) = instantiate(&factory, &cid, host_ctx.as_ptr())?;
            let Some(controller) = controller else {
                finish(&component, None, separate);
                return Err("the plugin has no controller".into());
            };
            let fail = |msg: &str| {
                finish(&component, Some(&controller), separate);
                Err(msg.to_string())
            };
            // the controller learns the component's state, then the project's values
            let state = MemStream::new(Vec::new());
            if let Some(s) = state.to_com_ptr::<IBStream>() {
                if component.getState(s.as_ptr()) == kResultOk {
                    let again = MemStream::new(state.bytes());
                    if let Some(p) = again.to_com_ptr::<IBStream>() {
                        controller.setComponentState(p.as_ptr());
                    }
                }
            }
            for &(id, v) in params {
                controller.setParamNormalized(id, v);
            }
            let handler = ComWrapper::new(Handler {
                edits: Mutex::new(Vec::new()),
            });
            if let Some(h) = handler.to_com_ptr::<IComponentHandler>() {
                controller.setComponentHandler(h.as_ptr());
            }
            let mut names = Vec::new();
            for i in 0..controller.getParameterCount() {
                let mut pi: ParameterInfo = std::mem::zeroed();
                if controller.getParameterInfo(i, &mut pi) == kResultOk {
                    names.push((pi.id, wstr(&pi.title)));
                }
            }

            let raw = controller.createView(kEditor);
            let Some(view) = ComPtr::from_raw(raw) else {
                return fail("the plugin has no window of its own");
            };
            let mut size: ViewRect = std::mem::zeroed();
            view.getSize(&mut size);
            let (w, h) = (
                (size.right - size.left).clamp(1, 8192) as u32,
                (size.bottom - size.top).clamp(1, 8192) as u32,
            );
            let window = match las_window::Window::new(title, w.max(200), h.max(100)) {
                Ok(win) => win,
                Err(e) => {
                    drop(view);
                    return fail(&e);
                }
            };
            let kind = platform(window.kind());
            if view.isPlatformTypeSupported(kind) != kResultTrue {
                drop(view);
                return fail("the plugin's window does not support this system");
            }
            let frame = ComWrapper::new(Frame {
                resize: Mutex::new(None),
                #[cfg(target_os = "linux")]
                fds: Mutex::new(Vec::new()),
                #[cfg(target_os = "linux")]
                timers: Mutex::new(Vec::new()),
            });
            if let Some(f) = frame.to_com_ptr::<IPlugFrame>() {
                view.setFrame(f.as_ptr());
            }
            if view.attached(window.handle(), kind) != kResultOk {
                view.setFrame(std::ptr::null_mut());
                drop(view);
                return fail("the plugin could not open its window");
            }
            let mut editor = Self {
                view,
                window,
                frame,
                handler,
                controller,
                component,
                separate,
                _host: host,
                _module: module,
                names,
            };
            editor.window.resize(w, h);
            Ok(editor)
        }
    }

    /// Serve the window, and take what the user changed in it since the last call.
    pub fn idle(&mut self) -> Vec<(u32, f64)> {
        self.window.pump();
        self.frame.run();
        let asked = self
            .frame
            .resize
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((w, h)) = asked {
            self.window.resize(w, h);
            let mut r = ViewRect {
                left: 0,
                top: 0,
                right: w as int32,
                bottom: h as int32,
            };
            // SAFETY: a call into the attached view
            unsafe { self.view.onSize(&mut r) };
        }
        std::mem::take(&mut *self.handler.edits.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Show a value that changed elsewhere (a slider, an undo).
    pub fn set_param(&mut self, id: u32, value: f64) {
        // SAFETY: a call into the controller
        unsafe { self.controller.setParamNormalized(id, value) };
    }

    /// The name of a parameter, for the undo list.
    pub fn param_name(&self, id: u32) -> Option<&str> {
        self.names
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
    }

    /// Whether the user closed the window.
    pub fn closed(&self) -> bool {
        self.window.closed()
    }
}

impl Drop for Vst3Editor {
    fn drop(&mut self) {
        // SAFETY: the view is removed before its window goes, then the lifecycle ends
        unsafe {
            self.view.removed();
            self.view.setFrame(std::ptr::null_mut());
            self.controller.setComponentHandler(std::ptr::null_mut());
            finish(&self.component, Some(&self.controller), self.separate);
        }
    }
}

const kEditor: *const c_char = c"editor".as_ptr();
