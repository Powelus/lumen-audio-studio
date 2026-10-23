// SPDX-License-Identifier: GPL-3.0-or-later
//! A VST2 plugin's own window, made the way the CLAP and VST3 ones are: the engine plays
//! its own instance, so the window is shown by a second instance on the app's thread,
//! set to the project's state. What the user turns in it comes back from
//! [`Vst2Editor::idle`] (the plugin reports it with `audioMasterAutomate`), and
//! [`Vst2Editor::state`] gives its whole state (a preset chosen in the window, which
//! changes more than the parameters say) for the project to keep.

#![allow(non_upper_case_globals, non_snake_case)]

use super::abi::*;
use super::Instance;
use crate::PluginInfo;
use std::ffi::c_void;

/// A VST2 plugin's window, open on the app's thread.
pub struct Vst2Editor {
    // the order matters: the window goes before the instance is closed
    window: las_window::Window,
    inst: Instance,
    open: bool,
    names: Vec<(u32, String)>,
}

impl Vst2Editor {
    /// Open the window of the plugin `info`, set to `state` (when there is one) and then
    /// to `params` (values from 0 to 1).
    pub fn open(
        info: &PluginInfo,
        state: Option<&[u8]>,
        params: &[(u32, f64)],
        title: &str,
    ) -> Result<Self, String> {
        let mut inst = Instance::open(&info.path, 44_100.0)?;
        if inst.effect().flags & effFlagsHasEditor == 0 {
            return Err("the plugin has no window of its own".into());
        }
        if let Some(s) = state {
            inst.load_state(s)?;
        }
        for &(id, v) in params {
            inst.set_param(id as i32, v.clamp(0.0, 1.0) as f32);
        }
        inst.start();
        let names = inst.params().into_iter().map(|p| (p.id, p.name)).collect();
        let (w, h) = rect(&inst).unwrap_or((400, 300));
        let window = las_window::Window::new(title, w.max(100), h.max(60))?;
        // the result is not looked at: plugins differ in what they return
        inst.dispatch(effEditOpen, 0, 0, window.handle(), 0.0);
        // some plugins know their size only once the window is open
        let mut ed = Self {
            window,
            inst,
            open: true,
            names,
        };
        if let Some((w, h)) = rect(&ed.inst) {
            ed.window.resize(w, h);
        }
        // what the plugin reported while it was set up is not the user's doing
        super::lock(&ed.inst.data.edits).clear();
        Ok(ed)
    }

    /// Serve the window; what the user changed in it since the last call.
    pub fn idle(&mut self) -> Vec<(u32, f64)> {
        self.window.pump();
        self.inst
            .dispatch(effEditIdle, 0, 0, std::ptr::null_mut(), 0.0);
        if let Some((w, h)) = super::lock(&self.inst.data.resize).take() {
            self.window.resize(w, h);
        }
        std::mem::take(&mut *super::lock(&self.inst.data.edits))
    }

    /// Show a value that changed elsewhere.
    pub fn set_param(&mut self, id: u32, value: f64) {
        self.inst.set_param(id as i32, value.clamp(0.0, 1.0) as f32);
        // a plugin may report the value it was set to: that is no edit
        super::lock(&self.inst.data.edits).retain(|&(i, _)| i != id);
    }

    /// The window's whole state, and the value of every parameter.
    pub fn state(&mut self) -> (Vec<u8>, Vec<(u32, f64)>) {
        let values = (0..self.inst.effect().numParams.max(0))
            .map(|i| (i as u32, f64::from(self.inst.get_param(i))))
            .collect();
        (self.inst.save_state(), values)
    }

    /// The name of a parameter.
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

/// The size of the plugin's window, as it says.
fn rect(inst: &Instance) -> Option<(u32, u32)> {
    let mut r: *mut ERect = std::ptr::null_mut();
    inst.dispatch(
        effEditGetRect,
        0,
        0,
        (&mut r as *mut *mut ERect).cast::<c_void>(),
        0.0,
    );
    if r.is_null() {
        return None;
    }
    // SAFETY: the plugin gave a rectangle it keeps
    let r = unsafe { *r };
    let (w, h) = (
        i32::from(r.right) - i32::from(r.left),
        i32::from(r.bottom) - i32::from(r.top),
    );
    (w > 0 && h > 0).then(|| (w.min(8192) as u32, h.min(8192) as u32))
}

impl Drop for Vst2Editor {
    fn drop(&mut self) {
        if self.open {
            self.inst
                .dispatch(effEditClose, 0, 0, std::ptr::null_mut(), 0.0);
            self.open = false;
        }
    }
}
