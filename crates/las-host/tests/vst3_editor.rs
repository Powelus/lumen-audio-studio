// SPDX-License-Identifier: GPL-3.0-or-later
//! The window of the test VST3 plugin, opened in a real X server (Xvfb when there is
//! no display). The test view reports an edit when it is attached and another from a
//! timer it asks the host's run loop for, with a new size.
#![cfg(target_os = "linux")]

mod common;

use common::testvst3;
use las_host::{describe_any, Editor};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

// what `las-testvst3` has (it is loaded as a library, not linked)
const GAIN: u32 = 1;
const EDIT_ON_OPEN: f64 = 0.25;
const EDIT_ON_TIMER: f64 = 0.75;

/// A display to open windows on: the one there is, or an Xvfb started for the test.
fn display() -> Option<Xvfb> {
    if std::env::var_os("DISPLAY").is_some() {
        return None;
    }
    let num = 90 + std::process::id() % 9;
    let child = Command::new("Xvfb")
        .arg(format!(":{num}"))
        .arg("-nolisten")
        .arg("tcp")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Xvfb to open a window on (install xvfb)");
    std::env::set_var("DISPLAY", format!(":{num}"));
    let socket = format!("/tmp/.X11-unix/X{num}");
    let until = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&socket).exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(Xvfb(child))
}

/// Stops the server when the test ends, also when it fails.
struct Xvfb(Child);

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_plugin_window_sends_what_the_user_turns() {
    let _xvfb = display();
    let info = describe_any(&testvst3()).pop().expect("the test plugin");
    let mut ed = Editor::open(&info, &[(GAIN, 0.5)], "Test").expect("the window opens");
    // the edit made when the view was attached
    assert_eq!(ed.idle(), vec![(GAIN, EDIT_ON_OPEN)]);
    // the timer the view asked the run loop for fires within a moment
    let until = Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    while got.is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
        got = ed.idle();
    }
    assert_eq!(got, vec![(GAIN, EDIT_ON_TIMER)]);
    assert!(!ed.closed());
    // a value set elsewhere reaches the controller, and nothing is sent back for it
    ed.set_param(GAIN, 0.1);
    assert!(ed.idle().is_empty());
    drop(ed);
}
