// SPDX-License-Identifier: GPL-3.0-or-later
//! Lumen Audio Studio: the native application.

// no console window next to the app on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod auto;
mod fxui;
mod inst;
mod menus;
mod mfx;
mod midi;
mod mixer;
mod playlist;
mod presets;
mod project;
mod rack;
mod roll;
mod shell;
mod steps;
mod theme;
mod widgets;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("lumen {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Lumen Audio Studio")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lumen Audio Studio",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
