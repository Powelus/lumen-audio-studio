// SPDX-License-Identifier: GPL-3.0-or-later
//! Instrument windows tied to the project's channels. A channel's patch and effects
//! load into the synth panels, every edit is written back to the channel (through
//! undo, so it is saved with the project), and the channels whose instruments have no
//! panel yet get a basic editor for their parameters.

use crate::app::{index, App, FxItem};
use crate::audio::Instrument;
use eframe::egui::{self, RichText, Ui};
use las_dsp::patch::merge_patch;
use serde_json::{json, Map, Value};

/// What a channel's instrument window edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bound {
    /// A channel the synth panels edit.
    Synth(usize),
    /// A sampler channel.
    Sampler(usize),
    /// A channel whose parameters the basic editor lists.
    Basic(usize),
    /// A hosted plugin.
    Hosted(usize),
}

impl Bound {
    pub fn channel(self) -> usize {
        match self {
            Bound::Synth(c) | Bound::Sampler(c) | Bound::Basic(c) | Bound::Hosted(c) => c,
        }
    }
}

/// The synth panel that edits a plugin, if there is one. The wavetable family
/// (Polaris, Vortex, Monolith, Zenith, TriOsc) shares one patch format.
pub fn panel_for(plugin_id: &str) -> Option<Instrument> {
    match plugin_id {
        "facet" => Some(Instrument::Facet),
        "mist" => Some(Instrument::Mist),
        "polaris" | "vortex" | "monolith" | "zenith" | "triosc" => Some(Instrument::Polaris),
        _ => None,
    }
}

/// A rack as the project stores it: `{type, on, state}` items, keeping the ids and any
/// other fields of the items that were there.
pub fn fx_to_json(items: &[FxItem], old: &Value) -> Value {
    let old = old.as_array();
    Value::Array(
        items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let mut o = old
                    .and_then(|l| l.get(i))
                    .and_then(Value::as_object)
                    .filter(|o| o.get("type").and_then(Value::as_str) == Some(&it.id))
                    .cloned()
                    .unwrap_or_else(Map::new);
                o.insert("type".into(), json!(it.id));
                o.insert("on".into(), json!(it.on));
                o.insert("state".into(), it.state.clone());
                if !o.contains_key("id") {
                    o.insert("id".into(), json!(las_project::create::uid("fx")));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// One editable value of a plugin state.
#[derive(Clone, Debug, PartialEq)]
pub enum Leaf {
    Number(f64),
    Flag(bool),
    Text(String),
}

/// The values of a state that the basic editor lists, with their paths (`a.b.0.c`),
/// skipping what is not a plain setting (ids, nulls).
pub fn leaves(v: &Value) -> Vec<(String, Leaf)> {
    fn walk(v: &Value, path: &str, out: &mut Vec<(String, Leaf)>) {
        match v {
            Value::Number(n) => {
                out.push((path.to_string(), Leaf::Number(n.as_f64().unwrap_or(0.0))))
            }
            Value::Bool(b) => out.push((path.to_string(), Leaf::Flag(*b))),
            Value::String(s) if !path.ends_with("id") && !path.ends_with(".id") => {
                out.push((path.to_string(), Leaf::Text(s.clone())));
            }
            Value::Object(m) => {
                for (k, x) in m {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(x, &p, out);
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    let p = if path.is_empty() {
                        i.to_string()
                    } else {
                        format!("{path}.{i}")
                    };
                    walk(x, &p, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, "", &mut out);
    out
}

/// The value at a dotted path.
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, part| match cur {
        Value::Array(a) => a.get(part.parse::<usize>().ok()?),
        Value::Object(m) => m.get(part),
        _ => None,
    })
}

/// Set the value at a dotted path (it must exist).
pub fn set_at(v: &mut Value, path: &str, new: Value) -> bool {
    let mut cur = v;
    for part in path.split('.') {
        let next = match cur {
            Value::Array(a) => part.parse::<usize>().ok().and_then(|i| a.get_mut(i)),
            Value::Object(m) => m.get_mut(part),
            _ => None,
        };
        match next {
            Some(n) => cur = n,
            None => return false,
        }
    }
    *cur = new;
    true
}

impl App {
    /// Open the instrument window of a channel.
    pub(crate) fn open_channel(&mut self, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            return;
        };
        let (plugin, state) = (c.plugin_id.clone(), c.state.clone());
        self.project.channel = channel;
        match panel_for(&plugin) {
            Some(inst) => {
                let k = index(inst);
                self.patches[k] = merge_patch(
                    &self.defaults[k],
                    state.get("patch").unwrap_or(&Value::Null),
                );
                self.switch(inst);
                self.push_patch(inst);
                let fx = state
                    .get("fx")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.load_fx_list(inst, &fx);
                self.bound = Some(Bound::Synth(channel));
                self.view = crate::app::View::Sound;
            }
            None if plugin == "sampler" => self.bound = Some(Bound::Sampler(channel)),
            None if plugin == las_host::HOSTED => self.bound = Some(Bound::Hosted(channel)),
            None => self.bound = Some(Bound::Basic(channel)),
        }
        // loading is not an edit
        self.synth_dirty = false;
        self.shell.open.synth = true;
    }

    /// Write the synth panels' patch and rack back into the bound channel.
    pub(crate) fn sync_bound(&mut self) {
        if !self.synth_dirty {
            return;
        }
        self.synth_dirty = false;
        let Some(Bound::Synth(ch)) = self.bound else {
            return;
        };
        let k = index(self.inst);
        let patch = self.patches[k].clone();
        let items = self.fx[k].clone();
        self.project.modify_coalesced("synth", move |p| {
            if let Some(c) = p.channels.get_mut(ch) {
                if !c.state.is_object() {
                    c.state = json!({});
                }
                let old_fx = c.state.get("fx").cloned().unwrap_or(Value::Null);
                c.state["patch"] = patch;
                c.state["fx"] = fx_to_json(&items, &old_fx);
            }
        });
    }

    /// The window of a channel whose instrument has no panel yet.
    pub(crate) fn basic_editor(&mut self, ui: &mut Ui, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            return;
        };
        let state = c.state.clone();
        ui.label(RichText::new(&c.name).size(16.0).strong());
        ui.label(
            RichText::new(format!(
                "{}: a basic editor. This instrument's own window is not in the native app yet.",
                c.plugin_id
            ))
            .weak()
            .size(11.0),
        );
        ui.separator();
        let mut changes: Vec<(String, Value)> = Vec::new();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("basic")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (path, leaf) in leaves(&state) {
                        ui.label(RichText::new(&path).size(11.5));
                        match leaf {
                            Leaf::Number(mut n) => {
                                let step = if n.abs() < 2.0 {
                                    0.01
                                } else {
                                    (n.abs() * 0.01).max(0.1)
                                };
                                if ui.add(egui::DragValue::new(&mut n).speed(step)).changed() {
                                    changes.push((path, json!(n)));
                                }
                            }
                            Leaf::Flag(mut b) => {
                                if ui.checkbox(&mut b, "").changed() {
                                    changes.push((path, json!(b)));
                                }
                            }
                            Leaf::Text(mut s) => {
                                if ui.text_edit_singleline(&mut s).changed() {
                                    changes.push((path, json!(s)));
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
        });
        if !changes.is_empty() {
            self.project.modify_coalesced("basic", move |p| {
                if let Some(c) = p.channels.get_mut(channel) {
                    for (path, v) in changes {
                        set_at(&mut c.state, &path, v);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wavetable_family_shares_the_polaris_panel() {
        for id in ["polaris", "vortex", "monolith", "zenith", "triosc"] {
            assert_eq!(panel_for(id), Some(Instrument::Polaris), "{id}");
        }
        assert_eq!(panel_for("facet"), Some(Instrument::Facet));
        assert_eq!(panel_for("mist"), Some(Instrument::Mist));
        for id in ["nebula", "sampler", "lumapad", "audioclip", "nothing"] {
            assert_eq!(panel_for(id), None, "{id}");
        }
    }

    #[test]
    fn a_rack_round_trips_and_keeps_ids() {
        let old = json!([{ "id": "keep", "type": "las-eq", "on": true, "state": {}, "extra": 1 }]);
        let items = vec![
            FxItem {
                id: "las-eq".into(),
                on: false,
                mix: 1.0,
                state: json!({ "x": 2 }),
            },
            FxItem {
                id: "las-hall".into(),
                on: true,
                mix: 1.0,
                state: json!({}),
            },
        ];
        let out = fx_to_json(&items, &old);
        assert_eq!(out[0]["id"], "keep");
        assert_eq!(out[0]["extra"], 1);
        assert_eq!(out[0]["on"], false);
        assert_eq!(out[0]["state"]["x"], 2);
        assert_eq!(out[1]["type"], "las-hall");
        assert!(out[1]["id"].as_str().is_some_and(|s| s.starts_with("fx")));
        // an item of another type at the same place does not lend its fields
        let swapped = fx_to_json(&items[1..], &old);
        assert_ne!(swapped[0]["id"], "keep");
        assert!(swapped[0].get("extra").is_none());
    }

    #[test]
    fn the_basic_editor_lists_and_sets_plain_settings() {
        let mut v = json!({ "gain": 0.5, "on": true, "name": "x", "id": "skip", "layers": [{ "pan": -1, "id": "l1" }], "none": null });
        let l = leaves(&v);
        let paths: Vec<_> = l.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"gain") && paths.contains(&"on") && paths.contains(&"name"));
        assert!(paths.contains(&"layers.0.pan"));
        assert!(
            !paths.contains(&"id") && !paths.contains(&"layers.0.id") && !paths.contains(&"none")
        );
        assert_eq!(at(&v, "layers.0.pan"), Some(&json!(-1)));
        assert!(set_at(&mut v, "layers.0.pan", json!(0.25)));
        assert_eq!(v["layers"][0]["pan"], 0.25);
        assert!(!set_at(&mut v, "layers.5.pan", json!(1)));
        assert!(!set_at(&mut v, "gain.deep", json!(1)));
    }
}
