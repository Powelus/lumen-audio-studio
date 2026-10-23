// SPDX-License-Identifier: GPL-3.0-or-later
//! The plugin manager (Settings > Plugin manager, or Tools > Plugin manager): every
//! plugin a scan found and every file that failed, in a table that filters and sorts.
//! Each plugin can be turned off; the menus, the browser and the quick search then
//! leave it out. What is turned off is kept in the settings, by file and plugin id, so
//! it survives rescans; plugins found for the first time are on.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{self, RichText, Ui};
use las_host::scan::Report;
use las_host::PluginInfo;
use std::path::PathBuf;

/// A column of the table, to sort by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Column {
    #[default]
    Name,
    Vendor,
    Format,
    Kind,
    Path,
    Status,
}

const COLUMNS: [(Column, &str); 6] = [
    (Column::Name, "Name"),
    (Column::Vendor, "Vendor"),
    (Column::Format, "Format"),
    (Column::Kind, "Type"),
    (Column::Path, "Path"),
    (Column::Status, "Status"),
];

/// The window's own state.
#[derive(Default)]
pub struct Manager {
    pub open: bool,
    pub filter: String,
    pub sort: Column,
    pub descending: bool,
}

/// One row: a plugin found, or a file that has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub vendor: String,
    pub format: String,
    pub kind: String,
    pub path: PathBuf,
    /// "OK", or why the file failed.
    pub status: String,
    /// The plugin, for a row that is one.
    pub info: Option<PluginInfo>,
}

impl Row {
    fn text(&self, c: Column) -> String {
        match c {
            Column::Name => self.name.to_lowercase(),
            Column::Vendor => self.vendor.to_lowercase(),
            Column::Format => self.format.clone(),
            Column::Kind => self.kind.clone(),
            Column::Path => self.path.to_string_lossy().to_lowercase(),
            Column::Status => self.status.to_lowercase(),
        }
    }
}

fn kind_of(i: &PluginInfo) -> &'static str {
    match (i.is_instrument(), i.is_effect()) {
        (true, true) => "instrument, effect",
        (true, false) => "instrument",
        (false, true) => "effect",
        _ => "other",
    }
}

/// The rows of a scan's report (or, before one, of the plugins known), filtered by
/// `filter` (words, all of which a row must have somewhere) and sorted.
pub fn rows(
    report: Option<&Report>,
    known: &[PluginInfo],
    filter: &str,
    sort: Column,
    descending: bool,
) -> Vec<Row> {
    let plugins = report.map_or(known, |r| &r.plugins[..]);
    let mut out: Vec<Row> = plugins
        .iter()
        .map(|i| Row {
            name: i.name.clone(),
            vendor: i.vendor.clone(),
            format: i.format.tag().into(),
            kind: kind_of(i).into(),
            path: i.path.clone(),
            status: "OK".into(),
            info: Some(i.clone()),
        })
        .collect();
    for f in report.map(|r| &r.failed[..]).unwrap_or_default() {
        out.push(Row {
            name: f
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            vendor: String::new(),
            format: las_host::scan::format_of(
                &f.path,
                las_host::scan::Os::current(),
                f.path.is_dir(),
            )
            .map_or("", las_host::Format::tag)
            .into(),
            kind: String::new(),
            path: f.path.clone(),
            status: f.reason.clone(),
            info: None,
        });
    }
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    out.retain(|r| {
        let all = COLUMNS
            .iter()
            .map(|(c, _)| r.text(*c))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        words.iter().all(|w| all.contains(w.as_str()))
    });
    out.sort_by(|a, b| {
        let o = a
            .text(sort)
            .cmp(&b.text(sort))
            .then_with(|| a.text(Column::Name).cmp(&b.text(Column::Name)));
        if descending {
            o.reverse()
        } else {
            o
        }
    });
    out
}

/// Where the last scan is: progress, or how many plugins it found and how many files
/// failed (for Settings).
pub fn scan_summary(ui: &mut Ui) {
    if let Some(s) = crate::hosted::scanning_text() {
        ui.label(RichText::new(s).color(pal().accent_text));
        return;
    }
    let Some(r) = las_host::last_report() else {
        return;
    };
    ui.label(format!(
        "Found {} plugin{} in {} file{} ({} from the cache).",
        r.plugins.len(),
        if r.plugins.len() == 1 { "" } else { "s" },
        r.files,
        if r.files == 1 { "" } else { "s" },
        r.cached
    ));
    if !r.failed.is_empty() {
        ui.collapsing(
            format!("{} file(s) could not be used", r.failed.len()),
            |ui| {
                for f in &r.failed {
                    ui.label(
                        RichText::new(format!("{}: {}", f.path.display(), f.reason))
                            .size(11.0)
                            .color(pal().warn),
                    );
                }
            },
        );
    }
}

impl App {
    /// Turn plugins on or off, keeping the choice in the settings.
    pub(crate) fn set_plugins_enabled(&mut self, infos: &[PluginInfo], on: bool) {
        let list = &mut self.settings.disabled_plugins;
        for i in infos {
            let key = las_host::enable_key(i);
            list.retain(|k| *k != key);
            if !on {
                list.push(key);
            }
        }
        self.settings.apply_disabled();
        self.settings.save();
    }

    pub(crate) fn plugin_manager_window(&mut self, ctx: &egui::Context) {
        if !self.plugin_manager.open {
            return;
        }
        let mut open = true;
        let report = las_host::last_report();
        let known = las_host::known();
        egui::Window::new("Plugin manager")
            .open(&mut open)
            .default_size([900.0, 520.0])
            .show(ctx, |ui| {
                let m = &mut self.plugin_manager;
                let (mut rescan, mut rescan_one, mut toggle, mut all) = (false, None, None, None);
                ui.horizontal(|ui| {
                    ui.label("Filter");
                    ui.add(egui::TextEdit::singleline(&mut m.filter).hint_text("name, vendor, VST2..."));
                    rescan = ui.button("Rescan").clicked();
                    if ui.button("Enable all").clicked() {
                        all = Some(true);
                    }
                    if ui.button("Disable all").clicked() {
                        all = Some(false);
                    }
                });
                scan_summary(ui);
                let list = rows(report.as_ref(), &known, &m.filter, m.sort, m.descending);
                ui.separator();
                egui::ScrollArea::both().show(ui, |ui| {
                    egui::Grid::new("plugin-manager").striped(true).num_columns(8).show(ui, |ui| {
                        ui.label("On");
                        for (c, name) in COLUMNS {
                            let mark = match (m.sort == c, m.descending) {
                                (true, false) => " ^",
                                (true, true) => " v",
                                _ => "",
                            };
                            if ui.button(format!("{name}{mark}")).clicked() {
                                if m.sort == c {
                                    m.descending = !m.descending;
                                } else {
                                    m.sort = c;
                                    m.descending = false;
                                }
                            }
                        }
                        ui.label("");
                        ui.end_row();
                        for r in &list {
                            match &r.info {
                                Some(i) => {
                                    let mut on = las_host::is_enabled(i);
                                    if ui
                                        .checkbox(&mut on, "")
                                        .on_hover_text("Show it in the menus, the browser and quick search")
                                        .changed()
                                    {
                                        toggle = Some((i.clone(), on));
                                    }
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.label(&r.name);
                            ui.label(&r.vendor);
                            ui.label(&r.format);
                            ui.label(&r.kind);
                            ui.label(RichText::new(r.path.display().to_string()).size(11.0).weak());
                            let color = if r.info.is_some() { pal().text } else { pal().warn };
                            ui.label(RichText::new(&r.status).color(color));
                            if ui.small_button("Rescan this plugin").clicked() {
                                rescan_one = Some(r.path.clone());
                            }
                            ui.end_row();
                        }
                    });
                });
                if let Some((i, on)) = toggle {
                    self.set_plugins_enabled(&[i], on);
                }
                if let Some(on) = all {
                    let infos: Vec<_> = list.iter().filter_map(|r| r.info.clone()).collect();
                    self.set_plugins_enabled(&infos, on);
                }
                if rescan {
                    self.rescan_plugins();
                }
                if rescan_one.is_some() {
                    self.rescan_plugin_file(rescan_one);
                }
            });
        self.plugin_manager.open &= open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use las_host::scan::Failure;
    use las_host::Format;

    fn info(name: &str, vendor: &str, format: Format, inst: bool) -> PluginInfo {
        PluginInfo {
            path: format!("/p/{name}").into(),
            id: format!("id.{name}"),
            name: name.into(),
            vendor: vendor.into(),
            features: vec![if inst { "instrument" } else { "audio-effect" }.into()],
            format,
        }
    }

    fn report() -> Report {
        Report {
            plugins: vec![
                info("Sylenth1", "LennarDigital", Format::Vst2, true),
                info("Reverb", "Acme", Format::Vst3, false),
                info("Bass", "Acme", Format::Clap, true),
            ],
            failed: vec![Failure {
                path: "/p/Old32.dll".into(),
                reason: "32-bit, not supported".into(),
            }],
            ..Default::default()
        }
    }

    fn names(r: &[Row]) -> Vec<&str> {
        r.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn the_table_filters_by_any_column_and_sorts() {
        let r = report();
        let all = rows(Some(&r), &[], "", Column::Name, false);
        assert_eq!(names(&all), vec!["Bass", "Old32", "Reverb", "Sylenth1"]);
        assert_eq!(all[1].status, "32-bit, not supported");
        assert!(all[1].info.is_none());
        assert_eq!(
            names(&rows(Some(&r), &[], "acme", Column::Name, false)),
            vec!["Bass", "Reverb"]
        );
        assert_eq!(
            names(&rows(Some(&r), &[], "vst2", Column::Name, false)),
            vec!["Sylenth1"]
        );
        assert_eq!(
            names(&rows(Some(&r), &[], "32-bit", Column::Name, false)),
            vec!["Old32"]
        );
        assert_eq!(
            names(&rows(Some(&r), &[], "acme instrument", Column::Name, false)),
            vec!["Bass"]
        );
        let by_vendor = rows(Some(&r), &[], "", Column::Vendor, true);
        assert_eq!(by_vendor[0].name, "Sylenth1");
        let by_format = rows(Some(&r), &[], "acme", Column::Format, false);
        assert_eq!(names(&by_format), vec!["Bass", "Reverb"]);
        // before a report, the known plugins
        assert_eq!(rows(None, &r.plugins, "syl", Column::Name, false).len(), 1);
    }
}
