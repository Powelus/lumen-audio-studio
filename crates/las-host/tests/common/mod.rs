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

fn build_testplug() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let lib = dir.join(format!(
        "{}las_testplug{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    if !lib.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.args(["build", "-p", "las-testplug"]);
        if dir.ends_with("release") {
            cmd.arg("--release");
        }
        assert!(cmd.status().unwrap().success());
    }
    // a `.clap` copy, the way plugins are installed
    let folder = dir.join(format!("clap-test-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let to = folder.join("las-test.clap");
    std::fs::copy(&lib, &to).unwrap();
    to
}
