// SPDX-License-Identifier: GPL-3.0-or-later
//! `lumen-plugin-host`: runs one hosted plugin for Lumen Audio Studio, so that a plugin
//! that crashes or hangs takes only this process with it. The app starts it and talks
//! to it over standard input and output (see `las_host::bridge`).

fn main() {
    if let Err(e) = las_host::bridge::serve() {
        eprintln!("lumen-plugin-host: {e}");
        std::process::exit(1);
    }
}
