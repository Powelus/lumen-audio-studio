// SPDX-License-Identifier: GPL-3.0-or-later
//! Lumen Audio Studio: the native application.

// no console window next to the app on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod auto;
mod fxui;
mod hosted;
mod inst;
mod instwin;
mod menus;
mod mfx;
mod midi;
mod mixer;
mod playlist;
mod presets;
mod project;
mod rack;
mod record;
mod roll;
mod samplerui;
mod shell;
mod skin;
mod skin_tables;
mod steps;
mod theme;
mod weave_ui;
mod widgets;
mod workflow;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("lumen {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // plugins are looked for in the background; the browser lists what is found
    las_host::install();
    std::thread::spawn(|| {
        las_host::rescan(&las_host::all_search_paths());
    });
    // `--size 1920x1080` opens the window at that size
    let size = std::env::args()
        .skip_while(|a| a != "--size")
        .nth(1)
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])
        })
        .unwrap_or([1280.0, 800.0]);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Lumen Audio Studio")
            .with_inner_size(size)
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lumen Audio Studio",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
