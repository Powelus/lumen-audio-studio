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

fn build_testplug() -> PathBuf {
    let (dir, lib) = library("las-testplug");
    // a `.clap` copy, the way plugins are installed
    let folder = dir.join(format!("clap-test-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let to = folder.join("las-test.clap");
    std::fs::copy(&lib, &to).unwrap();
    to
}
