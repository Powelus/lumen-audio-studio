// SPDX-License-Identifier: GPL-3.0-or-later
//! Hosted (CLAP, VST3) plugins in the app: the lists in the browser, adding them as
//! channels and effects, a window of generic controls for their parameters, and the
//! plugin's own window. What is turned in a plugin's window comes back
//! as parameter changes, so its parameters are what a project keeps of it.

use crate::app::App;
use crate::theme::pal;
use eframe::egui::{self, RichText, Ui};
use las_host::{find, params_of, plugin_state, Editor, PluginInfo, HOSTED};
use serde_json::{json, Value};

/// The plugin id a channel or slot state names.
pub fn plugin_id(state: &Value) -> &str {
    state.get("id").and_then(Value::as_str).unwrap_or("")
}

/// The name to show for the plugin a state names.
pub fn name_of(state: &Value) -> String {
    let id = plugin_id(state);
    find(id).map_or_else(|| id.to_string(), |p| p.name)
}

/// The value a state holds for a parameter (the plugin's default when it holds none).
pub fn value_of(state: &Value, id: u32, default: f64) -> f64 {
    state
        .get("params")
        .and_then(|p| p.get(id.to_string()))
        .and_then(Value::as_f64)
        .unwrap_or(default)
}

/// Write parameter values into a state.
pub fn apply(state: &mut Value, changes: &[(u32, f64)]) {
    if !state.is_object() {
        *state = json!({});
    }
    if !state.get("params").is_some_and(Value::is_object) {
        state["params"] = json!({});
    }
    for &(id, v) in changes {
        state["params"][id.to_string()] = json!(v);
    }
}

/// What the user did in the parameter list.
#[derive(Default)]
pub struct ParamsOut {
    pub changes: Vec<(u32, f64)>,
    /// A parameter to automate: its id, name and where it is now (0 to 1).
    pub automate: Option<(u32, String, f64)>,
}

/// Sliders for the parameters of the plugin `state` names. Right click one to automate it.
pub fn params_ui(ui: &mut Ui, state: &Value) -> ParamsOut {
    let mut out = ParamsOut::default();
    let id = plugin_id(state);
    if find(id).is_none() {
        ui.label(
            RichText::new(format!(
                "The plugin {id} is not installed here, so it is silent."
            ))
            .color(pal().muted),
        );
        return out;
    }
    let params = params_of(id);
    if params.is_empty() {
        ui.label(RichText::new("This plugin has no parameters to show.").weak());
    }
    egui::Grid::new(("hosted", id))
        .num_columns(2)
        .show(ui, |ui| {
            for p in params.iter().filter(|p| p.max > p.min) {
                let mut v = value_of(state, p.id, p.default);
                ui.label(&p.name);
                let r = ui.add(egui::Slider::new(&mut v, p.min..=p.max));
                if r.changed() {
                    out.changes.push((p.id, v));
                }
                if r.double_clicked() {
                    out.changes.push((p.id, p.default));
                }
                r.context_menu(|ui| {
                    if ui.button("Create automation clip").clicked() {
                        let at = ((v - p.min) / (p.max - p.min)).clamp(0.0, 1.0);
                        out.automate = Some((p.id, p.name.clone(), at));
                        ui.close();
                    }
                });
                ui.end_row();
            }
        });
    out
}

/// What a plugin window belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Owner {
    /// A channel, by id.
    Channel(String),
    /// An effect slot: the insert and the slot's id.
    Slot(usize, String),
}

/// A plugin's own window that is open.
pub struct OpenEditor {
    pub owner: Owner,
    pub editor: Editor,
}

/// The values a state holds for every parameter of its plugin.
fn values(state: &Value) -> Vec<(u32, f64)> {
    params_of(plugin_id(state))
        .iter()
        .map(|p| (p.id, value_of(state, p.id, p.default)))
        .collect()
}

/// Whether the plugin a state names is installed (CLAP and VST3 windows are hosted).
fn has_window(state: &Value) -> bool {
    find(plugin_id(state)).is_some()
}

fn format_name(f: las_host::Format) -> &'static str {
    match f {
        las_host::Format::Clap => "CLAP",
        las_host::Format::Vst3 => "VST3",
    }
}

impl App {
    /// Open the plugin's own window for `owner`, or bring nothing new when it is open.
    fn open_editor(&mut self, owner: Owner, state: &Value, title: &str) {
        if self.editors.iter().any(|e| e.owner == owner) {
            return;
        }
        let Some(info) = find(plugin_id(state)) else {
            return;
        };
        match Editor::open(&info, &values(state), title) {
            Ok(editor) => self.editors.push(OpenEditor { owner, editor }),
            Err(e) => self.error = Some(format!("{}: {e}", info.name)),
        }
    }

    /// Show values changed in the generic controls in the plugin's window too.
    fn tell_editor(&mut self, owner: &Owner, changes: &[(u32, f64)]) {
        if let Some(e) = self.editors.iter_mut().find(|e| &e.owner == owner) {
            for &(id, v) in changes {
                e.editor.set_param(id, v);
            }
        }
    }

    /// The button that opens a plugin's own window.
    fn editor_button(&mut self, ui: &mut Ui, owner: Owner, state: &Value, title: &str) {
        if !has_window(state) {
            return;
        }
        let open = self.editors.iter().any(|e| e.owner == owner);
        if ui
            .add_enabled(!open, egui::Button::new("Plugin window"))
            .on_hover_text("Open the plugin's own window")
            .clicked()
        {
            self.open_editor(owner, state, title);
        }
    }

    /// Serve the open plugin windows: what was turned in them goes into the project,
    /// and a window closes when the user closed it or its owner is gone.
    pub(crate) fn serve_editors(&mut self) {
        if self.editors.is_empty() {
            return;
        }
        let mut edits = Vec::new();
        for e in &mut self.editors {
            let changes = e.editor.idle();
            if !changes.is_empty() {
                edits.push((e.owner.clone(), changes));
            }
        }
        for (owner, changes) in edits {
            self.project
                .modify_coalesced("hosted-window", move |p| match &owner {
                    Owner::Channel(id) => {
                        if let Some(c) = p.channels.iter_mut().find(|c| &c.id == id) {
                            apply(&mut c.state, &changes);
                        }
                    }
                    Owner::Slot(insert, id) => {
                        if let Some(s) = p
                            .mixer
                            .get_mut(*insert)
                            .and_then(|t| t.slots.iter_mut().flatten().find(|s| &s.id == id))
                        {
                            apply(&mut s.state, &changes);
                        }
                    }
                });
        }
        let project = self.project.loaded.as_ref().map(|l| &l.project);
        let alive = |o: &Owner| {
            project.is_some_and(|p| match o {
                Owner::Channel(id) => p.channels.iter().any(|c| &c.id == id),
                Owner::Slot(insert, id) => p
                    .mixer
                    .get(*insert)
                    .is_some_and(|t| t.slots.iter().flatten().any(|s| &s.id == id)),
            })
        };
        self.editors
            .retain(|e| !e.editor.closed() && alive(&e.owner));
    }

    /// The window of a channel that is a hosted plugin.
    pub(crate) fn hosted_channel(&mut self, ui: &mut Ui, channel: usize) {
        let Some(c) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(channel))
        else {
            return;
        };
        let (name, state, channel_id) = (c.name.clone(), c.state.clone(), c.id.clone());
        ui.label(RichText::new(&name).size(16.0).strong());
        if let Some(info) = find(plugin_id(&state)) {
            ui.label(
                RichText::new(format!(
                    "{} ({}, {})",
                    info.name,
                    info.vendor,
                    format_name(info.format)
                ))
                .weak(),
            );
        }
        self.editor_button(ui, Owner::Channel(channel_id.clone()), &state, &name);
        ui.separator();
        let ParamsOut { changes, automate } = egui::ScrollArea::vertical()
            .show(ui, |ui| params_ui(ui, &state))
            .inner;
        self.tell_editor(&Owner::Channel(channel_id.clone()), &changes);
        if let Some((id, pname, start)) = automate {
            self.project.create_automation(&crate::auto::AutoRequest {
                target: crate::auto::plugin_target(&channel_id, &format!("p.{id}")),
                name: format!("{name} {pname}"),
                start,
            });
        }
        if !changes.is_empty() {
            self.project.modify_coalesced("hosted", move |p| {
                if let Some(c) = p.channels.get_mut(channel) {
                    apply(&mut c.state, &changes);
                }
            });
        }
    }

    /// The window of an effect slot that is a hosted plugin.
    pub(crate) fn hosted_effect(&mut self, ui: &mut Ui, insert: usize, slot: usize) {
        let Some(s) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.mixer.get(insert))
            .and_then(|t| t.slots.get(slot))
            .and_then(Option::as_ref)
        else {
            return;
        };
        let (state, mut on, mut mix, slot_id) = (s.state.clone(), s.enabled, s.mix, s.id.clone());
        let (mut on_changed, mut mix_changed, mut remove) = (false, false, false);
        let owner = Owner::Slot(insert, slot_id.clone());
        self.editor_button(ui, owner.clone(), &state, &name_of(&state));
        ui.horizontal(|ui| {
            on_changed = ui.checkbox(&mut on, "").changed();
            ui.label(
                RichText::new(name_of(&state))
                    .strong()
                    .size(16.0)
                    .color(pal().accent_text),
            );
            ui.label("Mix");
            mix_changed = ui
                .add(egui::Slider::new(&mut mix, 0.0..=1.0).show_value(false))
                .changed();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                remove = ui.button("Remove").clicked();
            });
        });
        ui.separator();
        let ParamsOut { changes, automate } = egui::ScrollArea::vertical()
            .show(ui, |ui| params_ui(ui, &state))
            .inner;
        self.tell_editor(&owner, &changes);
        if let Some((id, pname, start)) = automate {
            self.project.create_automation(&crate::auto::AutoRequest {
                target: crate::auto::effect_target(insert, &slot_id, &format!("p.{id}")),
                name: format!("{} {pname}", name_of(&state)),
                start,
            });
        }
        if remove {
            self.project.modify(true, |p| {
                if let Some(s) = p.mixer.get_mut(insert).and_then(|t| t.slots.get_mut(slot)) {
                    *s = None;
                }
            });
            self.project.open_effect = None;
        } else if on_changed || mix_changed || !changes.is_empty() {
            self.project.modify_coalesced("hosted-effect", move |p| {
                if let Some(Some(s)) = p.mixer.get_mut(insert).and_then(|t| t.slots.get_mut(slot)) {
                    if on_changed {
                        s.enabled = on;
                    }
                    if mix_changed {
                        s.mix = mix;
                    }
                    apply(&mut s.state, &changes);
                }
            });
        }
    }

    /// Add a hosted instrument as a new channel.
    pub(crate) fn add_hosted_channel(&mut self, info: &PluginInfo) {
        let (id, name) = (info.id.clone(), info.name.clone());
        self.add_channel(HOSTED);
        let at = self.project.channel;
        self.project.modify(true, |p| {
            if let Some(c) = p.channels.get_mut(at) {
                c.state = plugin_state(&id, &[]);
                c.name = name.clone();
                let insert = c.mixer_track;
                if let Some(t) = p.mixer.get_mut(insert) {
                    t.name = name;
                }
            }
        });
    }

    /// The browser's list of hosted plugins.
    pub(crate) fn hosted_list(&mut self, ui: &mut Ui) {
        let p = pal();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("EXTERNAL (CLAP, VST3)")
                    .size(11.0)
                    .strong()
                    .color(p.muted),
            );
            if ui
                .small_button("Rescan")
                .on_hover_text("Look for plugins again")
                .clicked()
            {
                std::thread::spawn(|| {
                    las_host::rescan(&las_host::all_search_paths());
                });
            }
        });
        let list = las_host::known();
        if list.is_empty() {
            ui.label(
                RichText::new(
                    "None found. Plugins are looked for in the usual CLAP and VST3 folders, and in CLAP_PATH and VST3_PATH.",
                )
                .weak()
                .size(11.0),
            );
        }
        for info in list {
            ui.horizontal(|ui| {
                let kind = match (info.is_instrument(), info.is_effect()) {
                    (true, true) => "instrument and effect",
                    (true, false) => "instrument",
                    (false, true) => "effect",
                    _ => "plugin",
                };
                if info.is_instrument()
                    && ui
                        .button(&info.name)
                        .on_hover_text("Add as a channel")
                        .clicked()
                {
                    self.add_hosted_channel(&info);
                } else if !info.is_instrument() {
                    ui.label(&info.name);
                }
                ui.label(
                    RichText::new(format!("{} - {kind}", info.vendor))
                        .size(11.0)
                        .color(p.muted),
                );
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_kept_in_the_state_by_id() {
        let mut s = plugin_state("a.b", &[]);
        assert_eq!(value_of(&s, 1, 0.5), 0.5);
        apply(&mut s, &[(1, 2.0), (7, -1.0)]);
        assert_eq!(value_of(&s, 1, 0.5), 2.0);
        assert_eq!(value_of(&s, 7, 0.0), -1.0);
        assert_eq!(plugin_id(&s), "a.b");
        // a state without parameters takes them
        let mut empty = json!(null);
        apply(&mut empty, &[(3, 0.25)]);
        assert_eq!(value_of(&empty, 3, 0.0), 0.25);
    }
}
