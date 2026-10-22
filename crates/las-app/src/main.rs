// SPDX-License-Identifier: GPL-3.0-or-later
//! Lumen Audio Studio: the native application.

// no console window next to the app on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyser;
mod app;
mod audio;
mod audioctl;
mod auto;
mod clipedit;
mod faces;
mod flexasio;
mod fxui;
mod hosted;
mod inst;
mod instwin;
mod menus;
mod mfx;
mod midi;
mod midilearn;
mod mixer;
mod nebulawin;
mod noterec;
mod playlist;
mod presets;
mod project;
mod rack;
mod raise;
mod record;
mod roll;
mod sampleedit;
mod samplerui;
mod settings;
mod shell;
mod skin;
mod skin_tables;
mod steps;
mod theme;
#[cfg(test)]
mod uitest;
mod weave_ui;
mod widgets;
mod workflow;

/// A panic anywhere (the window, the audio thread, a background job) is written to
/// `crash.log` in the config folder as well as to stderr, which a Windows build does
/// not show: so a crash never goes unexplained.
fn install_panic_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let text = format!(
            "lumen {} panicked on thread {}: {info}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Some(dir) = presets::config_dir() {
            let _ = std::fs::create_dir_all(&dir);
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("crash.log"))
            {
                let _ = f.write_all(text.as_bytes());
            }
        }
        default(info);
    }));
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("lumen {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    install_panic_log();
    // plugins are looked for in the background; the browser lists what is found
    las_host::install();
    {
        let dirs = settings::Settings::load().plugin_dirs();
        std::thread::spawn(move || {
            las_host::rescan(&dirs);
        });
    }
    // `--size 1920x1080` opens the window at that size (in screen pixels) instead of
    // maximised, for screenshots and for systems that do not maximise
    let size = std::env::args()
        .skip_while(|a| a != "--size")
        .nth(1)
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])
        });
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Lumen Audio Studio")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .unwrap_or_default(),
            )
            .with_inner_size(size.unwrap_or([1280.0, 800.0]))
            .with_maximized(size.is_none())
            .with_min_inner_size([1024.0, 576.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lumen Audio Studio",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
