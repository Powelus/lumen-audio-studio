// SPDX-License-Identifier: GPL-3.0-or-later
//! A plain native window for a plugin to draw its editor into: an X11 window on Linux,
//! a Win32 window on Windows and an `NSWindow` on macOS. The plugin attaches its view
//! to [`Window::handle`]; this crate only makes the window, sizes it and says when the
//! user closed it. It is made and used on the thread that runs the app's windows.

use std::ffi::c_void;

/// What [`Window::handle`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An X11 window id.
    X11,
    /// An `HWND`.
    Win32,
    /// An `NSView`, the window's content view.
    Cocoa,
}

/// A native window for a plugin editor.
pub struct Window {
    imp: imp::Window,
}

impl Window {
    /// A window of the given inner size, shown at once.
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
        Ok(Self {
            imp: imp::Window::new(title, width.max(1), height.max(1))?,
        })
    }

    /// The native handle a plugin attaches to.
    pub fn handle(&self) -> *mut c_void {
        self.imp.handle()
    }

    pub fn kind(&self) -> Kind {
        imp::KIND
    }

    /// The kind of window this system makes.
    pub fn kind_of_system() -> Kind {
        imp::KIND
    }

    /// Change the inner size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.imp.resize(width.max(1), height.max(1));
    }

    /// Handle the window's own events. Call it often while the window is open.
    pub fn pump(&mut self) {
        self.imp.pump();
    }

    /// Whether the user closed the window.
    pub fn closed(&self) -> bool {
        self.imp.closed()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::c_void;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask, PropMode,
        WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    pub const KIND: super::Kind = super::Kind::X11;

    pub struct Window {
        conn: RustConnection,
        id: u32,
        delete: u32,
        closed: bool,
    }

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    impl Window {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let (conn, screen) = x11rb::connect(None).map_err(err)?;
            let root = conn.setup().roots[screen].clone();
            let id = conn.generate_id().map_err(err)?;
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                root.root,
                0,
                0,
                width as u16,
                height as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                root.root_visual,
                &CreateWindowAux::new()
                    .background_pixel(root.black_pixel)
                    .event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(err)?;
            conn.change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            )
            .map_err(err)?;
            let atom = |name: &str| -> Result<u32, String> {
                Ok(conn
                    .intern_atom(false, name.as_bytes())
                    .map_err(err)?
                    .reply()
                    .map_err(err)?
                    .atom)
            };
            let protocols = atom("WM_PROTOCOLS")?;
            let delete = atom("WM_DELETE_WINDOW")?;
            conn.change_property32(PropMode::REPLACE, id, protocols, AtomEnum::ATOM, &[delete])
                .map_err(err)?;
            conn.map_window(id).map_err(err)?;
            conn.flush().map_err(err)?;
            Ok(Self {
                conn,
                id,
                delete,
                closed: false,
            })
        }

        pub fn handle(&self) -> *mut c_void {
            self.id as usize as *mut c_void
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            let _ = self.conn.configure_window(
                self.id,
                &ConfigureWindowAux::new().width(width).height(height),
            );
            let _ = self.conn.flush();
        }

        pub fn pump(&mut self) {
            while let Ok(Some(ev)) = self.conn.poll_for_event() {
                match ev {
                    Event::ClientMessage(m) if m.data.as_data32()[0] == self.delete => {
                        self.closed = true;
                    }
                    Event::DestroyNotify(d) if d.window == self.id => self.closed = true,
                    _ => {}
                }
            }
        }

        pub fn closed(&self) -> bool {
            self.closed
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            let _ = self.conn.destroy_window(self.id);
            let _ = self.conn.flush();
        }
    }
}

/// The system's mouse cursor, in physical pixels of the screen, where the system
/// can say (Windows). egui hears no pointer moves while files are dragged in from
/// Explorer, so the app reads it from here to place a drop.
#[cfg(windows)]
pub fn cursor_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos only writes the POINT it is given.
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// The system's mouse cursor: not read on this system (the pointer events are right).
#[cfg(not(windows))]
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub const KIND: super::Kind = super::Kind::Win32;

    pub struct Window {
        hwnd: HWND,
        /// Set by the window procedure when the user closes the window.
        closed: Box<AtomicBool>,
    }

    const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_CLOSE {
            let flag = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const AtomicBool;
            if !flag.is_null() {
                (*flag).store(true, Ordering::Relaxed);
            }
            // the owner destroys the window once the plugin has let go of it
            return 0;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    /// The outer size for an inner size.
    fn outer(width: u32, height: u32) -> (i32, i32) {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        // SAFETY: a plain call on a local rectangle
        unsafe { AdjustWindowRectEx(&mut r, STYLE, 0, 0) };
        (r.right - r.left, r.bottom - r.top)
    }

    impl Window {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let class = wide("LumenPluginWindow");
            let title = wide(title);
            let (w, h) = outer(width, height);
            // SAFETY: Win32 calls with live, nul ended strings; registering the class
            // twice fails harmlessly
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: instance,
                    hIcon: std::ptr::null_mut(),
                    hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    STYLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    w,
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return Err("the window could not be made".into());
                }
                let closed = Box::new(AtomicBool::new(false));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*closed as *const AtomicBool as isize);
                ShowWindow(hwnd, SW_SHOW);
                Ok(Self { hwnd, closed })
            }
        }

        pub fn handle(&self) -> *mut c_void {
            self.hwnd
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            let (w, h) = outer(width, height);
            // SAFETY: the window is alive
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    w,
                    h,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        /// The app's message loop runs on this thread and serves this window too.
        pub fn pump(&mut self) {}

        pub fn closed(&self) -> bool {
            self.closed.load(Ordering::Relaxed)
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            // SAFETY: the window is alive; its flag is cleared before the box goes
            unsafe {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::ffi::c_void;

    pub const KIND: super::Kind = super::Kind::Cocoa;

    pub struct Window {
        window: Retained<NSWindow>,
    }

    impl Window {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let mtm = MainThreadMarker::new().ok_or("windows are made on the main thread")?;
            let rect = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(f64::from(width), f64::from(height)),
            );
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;
            // SAFETY: AppKit calls on the main thread; the window is kept after closing
            // so the plugin's view stays valid until it is removed
            let window = unsafe {
                let w = NSWindow::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    rect,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                );
                w.setReleasedWhenClosed(false);
                w
            };
            window.setTitle(&NSString::from_str(title));
            window.center();
            window.makeKeyAndOrderFront(None);
            Ok(Self { window })
        }

        pub fn handle(&self) -> *mut c_void {
            self.window.contentView().map_or(std::ptr::null_mut(), |v| {
                Retained::as_ptr(&v) as *mut c_void
            })
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            self.window
                .setContentSize(NSSize::new(f64::from(width), f64::from(height)));
        }

        /// The app's event loop runs on this thread and serves this window too.
        pub fn pump(&mut self) {}

        pub fn closed(&self) -> bool {
            !self.window.isVisible()
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            self.window.close();
        }
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use std::ffi::c_void;

    pub const KIND: super::Kind = super::Kind::X11;

    pub struct Window;

    impl Window {
        pub fn new(_title: &str, _width: u32, _height: u32) -> Result<Self, String> {
            Err("plugin windows are not supported on this system".into())
        }
        pub fn handle(&self) -> *mut c_void {
            std::ptr::null_mut()
        }
        pub fn resize(&mut self, _width: u32, _height: u32) {}
        pub fn pump(&mut self) {}
        pub fn closed(&self) -> bool {
            true
        }
    }
}
