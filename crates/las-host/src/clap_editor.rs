// SPDX-License-Identifier: GPL-3.0-or-later
//! A CLAP plugin's own window, made the way [`crate::vst3::Vst3Editor`] makes a VST3
//! one: the engine plays its own instance of the plugin, so the window is shown by a
//! second, inactive instance on the app's thread. It is set to the project's values,
//! and what the user turns in it is read back with a parameter flush and returned by
//! [`ClapEditor::idle`] for the project, which the playing instance then follows.
//!
//! The host side serves what plugin windows need: the GUI callbacks (size requests and
//! closing), parameter flush requests, and on Linux the timers and file descriptors
//! that drive a plugin's event loop.

use crate::PluginInfo;
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui, Window as ClapWindow,
};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
    PluginParams,
};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::events::event_types::ParamValueEvent;
use clack_host::prelude::*;
use std::cell::RefCell;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What any thread of the plugin may tell the host.
struct Shared {
    resize: Mutex<Option<(u32, u32)>>,
    closed: AtomicBool,
}

impl SharedHandler<'_> for Shared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

impl HostGuiImpl for Shared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, size: GuiSize) -> Result<(), HostError> {
        *self.resize.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((size.width.max(1), size.height.max(1)));
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl HostParamsImplShared for Shared {
    // the window is flushed on every idle call anyway
    fn request_flush(&self) {}
}

struct Timer {
    id: u32,
    every: Duration,
    next: Instant,
}

/// What only the app's thread is told: timers, and on Linux file descriptors.
struct Main {
    timers: RefCell<Vec<Timer>>,
    next_timer: RefCell<u32>,
    #[cfg(unix)]
    fds: RefCell<Vec<(i32, clack_extensions::posix_fd::FdFlags)>>,
}

impl MainThreadHandler<'_> for Main {}

impl HostParamsImplMainThread for Main {
    fn rescan(&self, _flags: ParamRescanFlags) {}
    fn clear(&self, _id: ClapId, _flags: ParamClearFlags) {}
}

impl HostTimerImpl for Main {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        let mut n = self.next_timer.borrow_mut();
        *n += 1;
        let every = Duration::from_millis(u64::from(period_ms.max(1)));
        self.timers.borrow_mut().push(Timer {
            id: *n,
            every,
            next: Instant::now() + every,
        });
        Ok(TimerId(*n))
    }

    fn unregister_timer(&self, id: TimerId) -> Result<(), HostError> {
        self.timers.borrow_mut().retain(|t| t.id != id.0);
        Ok(())
    }
}

#[cfg(unix)]
impl clack_extensions::posix_fd::HostPosixFdImpl for Main {
    fn register_fd(
        &self,
        fd: i32,
        flags: clack_extensions::posix_fd::FdFlags,
    ) -> Result<(), HostError> {
        let mut fds = self.fds.borrow_mut();
        fds.retain(|(f, _)| *f != fd);
        fds.push((fd, flags));
        Ok(())
    }

    fn modify_fd(
        &self,
        fd: i32,
        flags: clack_extensions::posix_fd::FdFlags,
    ) -> Result<(), HostError> {
        self.register_fd(fd, flags)
    }

    fn unregister_fd(&self, fd: i32) -> Result<(), HostError> {
        self.fds.borrow_mut().retain(|(f, _)| *f != fd);
        Ok(())
    }
}

struct EditorHost;

impl HostHandlers for EditorHost {
    type Shared<'a> = Shared;
    type MainThread<'a> = Main;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostGui>()
            .register::<HostParams>()
            .register::<HostTimer>();
        #[cfg(unix)]
        builder.register::<clack_extensions::posix_fd::HostPosixFd>();
    }
}

/// The API a window of this system is attached with.
fn api(kind: las_window::Kind) -> GuiApiType<'static> {
    match kind {
        las_window::Kind::X11 => GuiApiType::X11,
        las_window::Kind::Win32 => GuiApiType::WIN32,
        las_window::Kind::Cocoa => GuiApiType::COCOA,
    }
}

/// A CLAP plugin's window, open on the app's thread.
pub struct ClapEditor {
    // the order matters: the plugin goes before the window it draws in
    instance: PluginInstance<EditorHost>,
    window: las_window::Window,
    gui: PluginGui,
    params: Option<PluginParams>,
    events: EventBuffer,
    out_events: EventBuffer,
    /// Changes the plugin reported while the window opened, for the first idle call.
    early: Vec<(u32, f64)>,
}

impl ClapEditor {
    /// Open the window of the plugin `info`, set to `params`.
    pub fn open(info: &PluginInfo, params: &[(u32, f64)], title: &str) -> Result<Self, String> {
        // SAFETY: loading a plugin library runs its code; that is what a host does
        let entry = unsafe { PluginEntry::load(&info.path) }.map_err(|e| e.to_string())?;
        let host = HostInfo::new("Lumen Audio Studio", "Lumen", "", env!("CARGO_PKG_VERSION"))
            .map_err(|e| e.to_string())?;
        let id = CString::new(info.id.clone()).map_err(|e| e.to_string())?;
        let mut instance = PluginInstance::<EditorHost>::new(
            |_| Shared {
                resize: Mutex::new(None),
                closed: AtomicBool::new(false),
            },
            |_| Main {
                timers: RefCell::new(Vec::new()),
                next_timer: RefCell::new(0),
                #[cfg(unix)]
                fds: RefCell::new(Vec::new()),
            },
            &entry,
            &id,
            &host,
        )
        .map_err(|e| e.to_string())?;

        let handle = instance.plugin_handle();
        let gui = handle
            .get_extension::<PluginGui>()
            .ok_or("the plugin has no window of its own")?;
        let params_ext = handle.get_extension::<PluginParams>();
        let kind = las_window::Window::kind_of_system();
        let config = GuiConfiguration {
            api_type: api(kind),
            is_floating: false,
        };
        if !gui.is_api_supported(&handle, config) {
            return Err("the plugin's window does not support this system".into());
        }
        gui.create(&handle, config)
            .map_err(|_| "the plugin could not make its window".to_string())?;
        let (w, h) = gui
            .get_size(&handle)
            .map_or((400, 300), |s| (s.width.max(1), s.height.max(1)));
        let window = match las_window::Window::new(title, w.min(8192), h.min(8192)) {
            Ok(win) => win,
            Err(e) => {
                gui.destroy(&handle);
                return Err(e);
            }
        };
        let parent = ClapWindow::from_generic_ptr(api(kind), window.handle());
        // SAFETY: the window outlives the plugin's view: it is dropped after the
        // instance, and the GUI is destroyed in `drop` first
        let attached = unsafe { gui.set_parent(&handle, parent) };
        if attached.is_err() || gui.show(&handle).is_err() {
            gui.destroy(&handle);
            return Err("the plugin could not open its window".into());
        }

        let mut editor = Self {
            instance,
            window,
            gui,
            params: params_ext,
            events: EventBuffer::with_capacity(64),
            out_events: EventBuffer::with_capacity(64),
            early: Vec::new(),
        };
        // the project's values
        for &(id, v) in params {
            editor.events.push(&ParamValueEvent::new(
                0,
                ClapId::new(id),
                Pckn::match_all(),
                v,
            ));
        }
        editor.early = editor.flush();
        Ok(editor)
    }

    /// Send the queued values and take what the plugin reports; the parameter changes.
    fn flush(&mut self) -> Vec<(u32, f64)> {
        let Some(params) = self.params else {
            self.events.clear();
            return Vec::new();
        };
        self.out_events.clear();
        if let Some(mut handle) = self.instance.inactive_plugin_handle() {
            let input = self.events.as_input();
            let mut output = self.out_events.as_output();
            params.flush(&mut handle, &input, &mut output);
        }
        self.events.clear();
        let mut out: Vec<(u32, f64)> = Vec::new();
        for e in self.out_events.iter() {
            if let Some(p) = e.as_event::<ParamValueEvent>() {
                if let Some(id) = p.param_id() {
                    let id = id.get();
                    out.retain(|&(i, _)| i != id);
                    out.push((id, p.value()));
                }
            }
        }
        out
    }

    /// Serve the window, and take what the user changed in it since the last call.
    pub fn idle(&mut self) -> Vec<(u32, f64)> {
        self.window.pump();
        self.run_loop();
        let asked = self
            .instance
            .access_shared_handler(|s| s.resize.lock().unwrap_or_else(|e| e.into_inner()).take());
        if let Some((w, h)) = asked {
            self.window.resize(w, h);
            let handle = self.instance.plugin_handle();
            let _ = self.gui.set_size(
                &handle,
                GuiSize {
                    width: w,
                    height: h,
                },
            );
        }
        let mut out = std::mem::take(&mut self.early);
        for (id, v) in self.flush() {
            out.retain(|&(i, _)| i != id);
            out.push((id, v));
        }
        out
    }

    /// Fire the timers that are due and serve the file descriptors that are ready.
    fn run_loop(&mut self) {
        let now = Instant::now();
        let due: Vec<u32> = self.instance.access_handler(|m| {
            m.timers
                .borrow_mut()
                .iter_mut()
                .filter(|t| t.next <= now)
                .map(|t| {
                    t.next = now + t.every;
                    t.id
                })
                .collect()
        });
        let handle = self.instance.plugin_handle();
        if let Some(timer) = handle.get_extension::<PluginTimer>() {
            for id in due {
                timer.on_timer(&handle, TimerId(id));
            }
        }
        #[cfg(unix)]
        {
            use clack_extensions::posix_fd::{FdFlags, PluginPosixFd};
            let fds: Vec<(i32, FdFlags)> = self.instance.access_handler(|m| m.fds.borrow().clone());
            let handle = self.instance.plugin_handle();
            let Some(ext) = handle.get_extension::<PluginPosixFd>() else {
                return;
            };
            if fds.is_empty() {
                return;
            }
            let mut polls: Vec<libc::pollfd> = fds
                .iter()
                .map(|&(fd, f)| libc::pollfd {
                    fd,
                    events: if f.contains(FdFlags::WRITE) {
                        libc::POLLIN | libc::POLLOUT
                    } else {
                        libc::POLLIN
                    },
                    revents: 0,
                })
                .collect();
            // SAFETY: a poll of the plugin's descriptors that does not wait
            let n = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 0) };
            if n <= 0 {
                return;
            }
            for p in &polls {
                let mut flags = FdFlags::empty();
                if p.revents & libc::POLLIN != 0 {
                    flags |= FdFlags::READ;
                }
                if p.revents & libc::POLLOUT != 0 {
                    flags |= FdFlags::WRITE;
                }
                if p.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                    flags |= FdFlags::ERROR;
                }
                if !flags.is_empty() {
                    ext.on_fd(&handle, p.fd, flags);
                }
            }
        }
    }

    /// Show a value that changed elsewhere.
    pub fn set_param(&mut self, id: u32, value: f64) {
        self.events.push(&ParamValueEvent::new(
            0,
            ClapId::new(id),
            Pckn::match_all(),
            value,
        ));
    }

    /// Whether the user closed the window (or the plugin did).
    pub fn closed(&self) -> bool {
        self.window.closed()
            || self
                .instance
                .access_shared_handler(|s| s.closed.load(Ordering::Relaxed))
    }
}

impl Drop for ClapEditor {
    fn drop(&mut self) {
        let handle = self.instance.plugin_handle();
        let _ = self.gui.hide(&handle);
        self.gui.destroy(&handle);
    }
}
