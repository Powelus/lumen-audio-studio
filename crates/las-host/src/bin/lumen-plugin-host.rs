// SPDX-License-Identifier: GPL-3.0-or-later
//! `lumen-plugin-host`: runs one hosted plugin for Lumen Audio Studio, so that a plugin
//! that crashes or hangs takes only this process with it. The app starts it and talks
//! to it over standard input and output (see `las_host::bridge`).
//!
//! `lumen-plugin-host --describe <file>` instead says what plugins a file has, as one
//! line of JSON, so a scan never loads a plugin into the app itself.

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--describe") {
        let Some(path) = args.get(2) else {
            eprintln!("lumen-plugin-host: --describe needs a file");
            std::process::exit(2);
        };
        println!(
            "{}",
            las_host::scan::describe_answer(std::path::Path::new(path))
        );
        return;
    }
    if let Err(e) = las_host::bridge::serve() {
        eprintln!("lumen-plugin-host: {e}");
        std::process::exit(1);
    }
}
