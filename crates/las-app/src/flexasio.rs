//! FlexASIO, the free ASIO driver over Windows audio that Windows builds bundle
//! (github.com/dechamps/FlexASIO, MIT licence). Settings > Audio installs it and
//! writes its settings file, `%USERPROFILE%\FlexASIO.toml`, in shared or exclusive mode.
//! The text and file checks here are plain and tested on every OS; only Windows shows them.

#![cfg_attr(not(windows), allow(dead_code))]

use eframe::egui;
use std::path::{Path, PathBuf};

/// The first line of a settings file Lumen wrote, so it is never mistaken for the user's own.
pub const HEADER: &str = "# written by Lumen Audio Studio";

/// The name FlexASIO has in the ASIO device list.
pub const DEVICE: &str = "FlexASIO";

/// How FlexASIO shares the sound card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Other programs keep playing (WASAPI shared mode). The default.
    Shared,
    /// Lumen has the device to itself: lowest latency, other programs go silent.
    Exclusive,
}

/// The settings file text for a mode.
pub fn config_text(mode: Mode) -> String {
    let (exclusive, buffer, latency) = match mode {
        // shared mode mixes with Windows' own 10 ms period, so ask for about that
        Mode::Shared => (false, 480, ""),
        Mode::Exclusive => (true, 256, "suggestedLatencySeconds = 0.0\n"),
    };
    format!(
        "{HEADER}\n\
         # Lumen rewrites this file when you change the FlexASIO mode in Settings > Audio.\n\
         # Delete this header line to keep your own changes; Lumen will leave the file alone.\n\
         backend = \"Windows WASAPI\"\n\
         bufferSizeSamples = {buffer}\n\
         \n\
         [input]\n\
         {latency}wasapiExclusiveMode = {exclusive}\n\
         \n\
         [output]\n\
         {latency}wasapiExclusiveMode = {exclusive}\n"
    )
}

/// What a settings file on disk is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    /// No file: FlexASIO uses its defaults until Lumen writes one.
    Missing,
    /// Lumen wrote it, in this mode.
    Lumen(Mode),
    /// The user's own file, which Lumen does not touch.
    Custom,
}

/// Reads a settings file's text (None: there is no file).
pub fn file_state(text: Option<&str>) -> FileState {
    let Some(t) = text else {
        return FileState::Missing;
    };
    if t.lines().next().map(str::trim) != Some(HEADER) {
        return FileState::Custom;
    }
    let exclusive = t
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").replace(' ', ""))
        .any(|l| l == "wasapiExclusiveMode=true");
    FileState::Lumen(if exclusive {
        Mode::Exclusive
    } else {
        Mode::Shared
    })
}

/// Where FlexASIO reads its settings: the user profile folder.
pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join("FlexASIO.toml"))
}

/// The state of the settings file at `path`.
pub fn read_state(path: &Path) -> FileState {
    file_state(std::fs::read_to_string(path).ok().as_deref())
}

/// Writes the settings file for `mode`, unless it is the user's own. Returns whether it wrote.
pub fn write_mode(path: &Path, mode: Mode) -> std::io::Result<bool> {
    if read_state(path) == FileState::Custom {
        return Ok(false);
    }
    std::fs::write(path, config_text(mode))?;
    Ok(true)
}

/// The installer in a folder: `FlexASIO-<version>.exe`, the newest name if there are several.
pub fn installer_in(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("FlexASIO-") && n.ends_with(".exe"))
        })
        .collect();
    found.sort();
    found.pop()
}

/// The installer shipped next to the app, in `drivers`.
pub fn bundled_installer() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    installer_in(&exe.parent()?.join("drivers"))
}

/// Whether FlexASIO is installed: it is in the ASIO device list, or its files are in
/// Program Files (where its installer puts them).
pub fn installed(asio_devices: &[String]) -> bool {
    asio_devices.iter().any(|d| d.eq_ignore_ascii_case(DEVICE))
        || ["ProgramFiles", "ProgramFiles(x86)"].iter().any(|v| {
            std::env::var_os(v).is_some_and(|p| PathBuf::from(p).join("FlexASIO").is_dir())
        })
}

/// Runs the installer elevated (one Windows permission prompt) and waits for it to finish.
/// Returns whether it ran and finished well.
pub fn run_installer(path: &Path) -> bool {
    // single quotes are doubled inside a PowerShell single-quoted string
    let p = path.display().to_string().replace('\'', "''");
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("Start-Process -FilePath '{p}' -Verb RunAs -Wait"),
        ])
        .status()
        .is_ok_and(|s| s.success())
}

/// What the FlexASIO section of Settings asks the app to do.
#[derive(Default)]
pub struct UiOut {
    /// The installer finished: read the devices again and switch to FlexASIO.
    pub installed_now: bool,
    /// The mode changed: reopen the audio so FlexASIO reads its settings again.
    pub restart: bool,
}

/// The installer running in the background, kept between frames.
#[derive(Clone, Default)]
struct Install(std::sync::Arc<std::sync::Mutex<Option<bool>>>);

/// The "Low latency driver (FlexASIO)" section of Settings > Audio (Windows).
pub fn settings_ui(ui: &mut egui::Ui, asio_devices: &[String]) -> UiOut {
    use egui::RichText;
    let mut out = UiOut::default();
    let p = crate::theme::pal();
    let id = egui::Id::unique("flexasio-install");
    let job: Option<Install> = ui.ctx().data(|d| d.get_temp(id));
    let mut running = false;
    if let Some(j) = &job {
        let done = *j.0.lock().unwrap_or_else(|e| e.into_inner());
        match done {
            None => running = true,
            Some(ok) => {
                ui.ctx().data_mut(|d| d.remove::<Install>(id));
                out.installed_now = ok;
            }
        }
    }
    ui.add_space(6.0);
    ui.strong("Low latency driver (FlexASIO)");
    ui.label(
        RichText::new(
            "FlexASIO is a free ASIO driver that works with any sound card. It gives quicker response than plain Windows audio, and in shared mode your other programs keep playing.",
        )
        .color(p.muted)
        .size(11.5),
    );
    let is_installed = installed(asio_devices);
    ui.horizontal(|ui| {
        ui.label("Status:");
        if is_installed {
            ui.label(RichText::new("Installed").color(p.ok));
        } else if running {
            ui.spinner();
            ui.label("Installing... answer the Windows prompt to continue");
        } else {
            ui.label(RichText::new("Not installed").color(p.muted));
            let bundled = bundled_installer();
            let tip = match &bundled {
                Some(_) => "Runs the FlexASIO installer that came with Lumen. Windows asks once for permission to install a driver; then Lumen switches to FlexASIO for you.",
                None => "The FlexASIO installer was not found in the drivers folder next to Lumen. Download it from github.com/dechamps/FlexASIO.",
            };
            if ui
                .add_enabled(bundled.is_some(), egui::Button::new("Install FlexASIO"))
                .on_hover_text(tip)
                .on_disabled_hover_text(tip)
                .clicked()
            {
                if let Some(path) = bundled {
                    let j = Install::default();
                    let flag = j.0.clone();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let ok = run_installer(&path);
                        *flag.lock().unwrap_or_else(|e| e.into_inner()) = Some(ok);
                        ctx.request_repaint();
                    });
                    ui.ctx().data_mut(|d| d.insert_temp(id, j));
                }
            }
        }
    });
    if running {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
    let Some(path) = config_path() else {
        return out;
    };
    match read_state(&path) {
        FileState::Custom => {
            ui.label(
                RichText::new(format!(
                    "Custom FlexASIO settings found in {}. Lumen leaves them as they are.",
                    path.display()
                ))
                .color(p.muted)
                .size(11.5),
            );
        }
        state => {
            let mut mode = match state {
                FileState::Lumen(m) => m,
                _ => Mode::Shared,
            };
            let before = mode;
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.radio_value(&mut mode, Mode::Shared, "Shared (other apps keep playing)")
                    .on_hover_text("Lumen plays alongside your browser, games and other programs. Recommended.");
                ui.radio_value(
                    &mut mode,
                    Mode::Exclusive,
                    "Exclusive (lowest latency, other apps go silent)",
                )
                .on_hover_text("Lumen takes the sound card to itself for the quickest response. Other programs make no sound until you close Lumen or switch back.");
            });
            if mode != before && write_mode(&path, mode).is_ok() {
                out.restart = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_text_for_each_mode() {
        let s = config_text(Mode::Shared);
        assert!(s.starts_with(HEADER));
        assert!(s.contains("backend = \"Windows WASAPI\""));
        assert!(s.contains("[output]\nwasapiExclusiveMode = false"));
        assert!(!s.contains("= true"));
        let e = config_text(Mode::Exclusive);
        assert!(e.contains("[output]\nsuggestedLatencySeconds = 0.0\nwasapiExclusiveMode = true"));
        assert_eq!(file_state(Some(&s)), FileState::Lumen(Mode::Shared));
        assert_eq!(file_state(Some(&e)), FileState::Lumen(Mode::Exclusive));
    }

    #[test]
    fn a_users_own_file_is_left_alone() {
        assert_eq!(file_state(None), FileState::Missing);
        assert_eq!(
            file_state(Some("backend = \"Windows WASAPI\"\n")),
            FileState::Custom
        );
        let dir = std::env::temp_dir().join(format!("las-flexasio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("FlexASIO.toml");
        let _ = std::fs::remove_file(&f);
        assert!(write_mode(&f, Mode::Exclusive).unwrap());
        assert_eq!(read_state(&f), FileState::Lumen(Mode::Exclusive));
        assert!(write_mode(&f, Mode::Shared).unwrap());
        assert_eq!(read_state(&f), FileState::Lumen(Mode::Shared));
        std::fs::write(&f, "[output]\nwasapiExclusiveMode = true\n").unwrap();
        assert!(!write_mode(&f, Mode::Shared).unwrap());
        assert_eq!(read_state(&f), FileState::Custom);
        // the installer is found by name
        std::fs::write(dir.join("FlexASIO-1.10b.exe"), b"").unwrap();
        assert_eq!(installer_in(&dir), Some(dir.join("FlexASIO-1.10b.exe")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_device_list_shows_it_installed() {
        assert!(installed(&["FlexASIO".to_string()]));
    }
}
