// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared by the tests: the test plugin's library.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// The test plugin's library, built if the workspace build has not.
pub fn testplug() -> PathBuf {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(build_testplug).clone()
}

/// Where the built library of a test plugin crate is.
pub fn library_path(krate: &str) -> PathBuf {
    library(krate).1
}

/// The built library of a test plugin crate, built if the workspace build has not.
fn library(krate: &str) -> (PathBuf, PathBuf) {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let lib = dir.join(format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        krate.replace('-', "_"),
        std::env::consts::DLL_SUFFIX
    ));
    if !lib.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.args(["build", "-p", krate]);
        if dir.ends_with("release") {
            cmd.arg("--release");
        }
        assert!(cmd.status().unwrap().success());
    }
    (dir, lib)
}

/// The test VST3 plugin, installed as a bundle the way a host finds it.
pub fn testvst3() -> PathBuf {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let (dir, lib) = library("las-testvst3");
        let root = dir.join(format!("vst3-test-{}", std::process::id()));
        let bundle = root.join("las-test.vst3");
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        let inside = if cfg!(target_os = "macos") {
            "Contents/MacOS/las-test".to_string()
        } else if cfg!(windows) {
            format!("Contents/{arch}-win/las-test.vst3")
        } else {
            format!("Contents/{arch}-linux/las-test.so")
        };
        let to = bundle.join(inside);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(&lib, &to).unwrap();
        bundle
    })
    .clone()
}

/// The test VST2 plugin, installed in a vendor folder the way a host finds it: a
/// library on Windows and Linux, a `.vst` bundle on macOS. The folder to scan is two
/// levels up.
pub fn testvst2() -> PathBuf {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let (dir, lib) = library("las-testvst2");
        let folder = dir
            .join(format!("vst2-test-{}", std::process::id()))
            .join("Lumen")
            .join("Test");
        let (plugin, to) = if cfg!(target_os = "macos") {
            let b = folder.join("las-test2.vst");
            (b.clone(), b.join("Contents/MacOS/las-test2"))
        } else {
            let f = folder.join(format!("las-test2{}", std::env::consts::DLL_SUFFIX));
            (f.clone(), f)
        };
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(&lib, &to).unwrap();
        plugin
    })
    .clone()
}

fn build_testplug() -> PathBuf {
    let (dir, lib) = library("las-testplug");
    // a `.clap` copy, the way plugins are installed
    let folder = dir.join(format!("clap-test-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let to = folder.join("las-test.clap");
    std::fs::copy(&lib, &to).unwrap();
    to
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub use x::*;

#[cfg(target_os = "linux")]
mod x {
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// A display to open windows on: the one there is, or an Xvfb started for the test.
    pub fn display() -> Option<Xvfb> {
        if std::env::var_os("DISPLAY").is_some() {
            return None;
        }
        // a display no other server holds (a server that ended badly leaves its lock)
        let start = 100 + std::process::id() % 400;
        let num = (start..start + 400)
            .find(|n| !std::path::Path::new(&format!("/tmp/.X{n}-lock")).exists())
            .expect("a free display number");
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
    pub struct Xvfb(Child);

    impl Drop for Xvfb {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
