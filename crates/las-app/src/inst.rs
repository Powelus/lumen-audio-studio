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
    /// Nebula's window.
    Nebula(usize),
    /// LumaPad's window.
    Pads(usize),
    /// Weave's patch editor.
    Weave(usize),
}

impl Bound {
    pub fn channel(self) -> usize {
        match self {
            Bound::Synth(c)
            | Bound::Sampler(c)
            | Bound::Basic(c)
            | Bound::Hosted(c)
            | Bound::Nebula(c)
            | Bound::Pads(c)
            | Bound::Weave(c) => c,
        }
    }
}

/// Whether keys played live (computer keyboard, on-screen piano, MIDI) go to the
/// project's channel rather than the standalone synth. The standalone synth only
/// stands in for a channel whose window is a synth panel (it holds that channel's
/// patch, edited without a rebuild); every other instrument window (Nebula, LumaPad,
/// Weave, the sampler, ...) plays its channel, so a preset or edit there is heard at
/// once instead of the standalone synth's old sound.
pub fn keys_to_project(synth_open: bool, bound: Option<Bound>) -> bool {
    !synth_open || !matches!(bound, None | Some(Bound::Synth(_)))
}

impl crate::app::App {
    /// Whether keys played live go to the project's channel; see [`keys_to_project`].
    pub(crate) fn keys_to_project(&self) -> bool {
        keys_to_project(self.shell.open.synth, self.bound)
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
        // the window opens and comes to the front, as a double click does in FL Studio
        self.shell.open.show(crate::raise::Win::Synth);
        match panel_for(&plugin) {
            Some(inst) => {
                let k = index(inst);
                // a new channel starts from its face's own init patch
                let mut base = self.defaults[k].clone();
                if matches!(plugin.as_str(), "monolith" | "zenith" | "triosc") {
                    if let Some(b) = las_dsp::presets::bank(&plugin) {
                        base = merge_patch(&base, &b.init);
                    }
                }
                self.patches[k] = merge_patch(&base, state.get("patch").unwrap_or(&Value::Null));
                self.switch(inst);
                self.push_patch(inst);
                let fx = state
                    .get("fx")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.load_fx_list(inst, &fx);
                self.bound = Some(Bound::Synth(channel));
                self.sync_bank();
                // the preset list shows the channel's own preset, not the one last
                // picked on another channel of this synth
                let name = state.get("preset").and_then(Value::as_str);
                let list = &self.entries[k];
                self.presets[k] = name
                    .and_then(|n| list.iter().position(|e| e.name == n))
                    .or_else(|| {
                        list.iter()
                            .position(|e| merge_patch(&base, &e.patch) == self.patches[k])
                    })
                    .unwrap_or(0);
                self.view = crate::app::View::Sound;
            }
            None if plugin == "sampler" => self.bound = Some(Bound::Sampler(channel)),
            None if plugin == las_host::HOSTED => {
                self.bound = Some(Bound::Hosted(channel));
                // the plugin's own window, as FL Studio opens it; the sliders when it has none
                if self.open_plugin_window(channel) {
                    self.shell.open.synth = false;
                }
            }
            None if plugin == "nebula" => self.bound = Some(Bound::Nebula(channel)),
            None if plugin == "lumapad" => self.bound = Some(Bound::Pads(channel)),
            None if plugin == las_engine::weave::WEAVE_ID => {
                self.bound = Some(Bound::Weave(channel))
            }
            None => self.bound = Some(Bound::Basic(channel)),
        }
        // loading is not an edit
        self.synth_dirty = false;
        self.shell.open.show(crate::raise::Win::Synth);
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
        let preset = self.entries[k]
            .get(self.presets[k])
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let Some(c) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(ch))
        else {
            return;
        };
        let old_fx = c.state.get("fx").cloned().unwrap_or(Value::Null);
        let fx = fx_to_json(&items, &old_fx);
        // a playing project hears a knob of the patch at once: the player takes the
        // new patch. A change to the rack (or no player) builds the player again.
        let mut state = if c.state.is_object() {
            c.state.clone()
        } else {
            json!({})
        };
        state["patch"] = patch;
        state["fx"] = fx.clone();
        state["preset"] = json!(preset);
        let same_rack = fx == old_fx || (old_fx.is_null() && items.is_empty());
        let plugin = c.plugin_id.clone();
        // a knob of a rack effect: its value goes to the player as an automation
        // clip's would (another change to the rack builds the player again)
        let rack_live = same_rack || {
            let mut before = c.state.clone();
            if before.is_object() {
                before["patch"] = state["patch"].clone();
                before["preset"] = state["preset"].clone();
            }
            self.live_state(ch, &before, &state)
        };
        let live = rack_live
            && self.project.takes_live_edits()
            && self
                .audio
                .as_mut()
                .is_none_or(|a| a.channel_patch(ch, &plugin, &state));
        if live {
            self.live_patches.push(ch);
        }
        let update = move |p: &mut las_project::Project| {
            if let Some(c) = p.channels.get_mut(ch) {
                c.state = state;
            }
        };
        if live {
            self.project.modify_coalesced_live("synth", update);
        } else {
            self.project.modify_coalesced("synth", update);
        }
    }

    /// Hand a playing project's channel the values that changed from `old` to `new` in
    /// its state, when its instrument takes them all live. Whether it did (then the
    /// player need not be built again).
    pub(crate) fn live_state(&mut self, channel: usize, old: &Value, new: &Value) -> bool {
        if !self.project.takes_live_edits() {
            return false;
        }
        let Some(plugin) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(channel))
            .map(|c| c.plugin_id.clone())
        else {
            return false;
        };
        let (a, b) = (leaves(old), leaves(new));
        if a.len() != b.len() {
            return false;
        }
        let mut sends = Vec::new();
        for ((pa, la), (pb, lb)) in a.iter().zip(&b) {
            if pa != pb {
                return false;
            }
            if la == lb {
                continue;
            }
            let Leaf::Number(v) = lb else {
                return false;
            };
            if !las_engine::instrument::live_param(&plugin, pa) {
                return false;
            }
            // the player takes the value over the parameter's range: one outside it
            // would be cut short, so that builds the player again
            let def = las_engine::pluginparams::param_def_in(&plugin, pa, old);
            let n = def.to_normalized(*v);
            if (def.from_normalized(n) - v).abs() > 1e-6 * v.abs().max(1.0) {
                return false;
            }
            sends.push((pa.clone(), n));
        }
        for (path, n) in sends {
            self.live_params.push((channel, path.clone()));
            if let Some(a) = &mut self.audio {
                a.send(crate::audio::Command::PluginParam {
                    channel,
                    path: path.into(),
                    normalised: n,
                });
            }
        }
        true
    }

    /// Give a channel a new state (an edit in its window; `key` makes a drag one undo
    /// step): heard at once when the instrument takes the changes live.
    pub(crate) fn edit_state(&mut self, channel: usize, key: &str, state: Value) {
        let Some(old) = self
            .project
            .loaded
            .as_ref()
            .and_then(|l| l.project.channels.get(channel))
            .map(|c| c.state.clone())
        else {
            return;
        };
        let live = self.live_state(channel, &old, &state);
        let f = move |p: &mut las_project::Project| {
            if let Some(c) = p.channels.get_mut(channel) {
                c.state = state;
            }
        };
        if live {
            self.project.modify_coalesced_live(key, f);
        } else {
            self.project.modify_coalesced(key, f);
        }
    }

    /// The window of a channel whose instrument has no panel yet.
    pub(crate) fn basic_editor(&mut self, ui: &mut Ui, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let Some(c) = l.project.channels.get(channel) else {
            return;
        };
        if c.plugin_id == "audioclip" {
            self.audio_clip_window(ui, channel);
            return;
        }
        if c.plugin_id == las_project::slicer::SLICER_ID {
            self.slicer_window(ui, channel);
            return;
        }
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
            let mut st = state;
            for (path, v) in changes {
                set_at(&mut st, &path, v);
            }
            self.edit_state(channel, "basic", st);
        }
    }
}

impl App {
    /// The window of an audio clip channel: the edits of its clips, as FL Studio's audio
    /// clip channel settings. They apply to every clip of this sound in the playlist.
    pub(crate) fn audio_clip_window(&mut self, ui: &mut Ui, channel: usize) {
        let Some(l) = &self.project.loaded else {
            return;
        };
        let project = l.project.clone();
        let Some(c) = project.channels.get(channel) else {
            return;
        };
        ui.label(RichText::new(&c.name).size(16.0).strong());
        // the sound's outline, and the way to edit it
        if let Some(wave) = c
            .sample_id
            .as_ref()
            .and_then(|s| self.project.playlist.waves.get(s))
            .cloned()
        {
            let r = crate::sampleedit::wave_strip(ui, &wave, 80.0)
                .on_hover_text("Double click to open the sample editor");
            if r.double_clicked() {
                crate::sampleedit::request(crate::sampleedit::Request::Edit(c.id.clone()));
            }
        }
        ui.horizontal(|ui| crate::sampleedit::menu_items(ui, &c.id));
        let clips: Vec<&las_project::Clip> = project
            .playlist
            .clips
            .iter()
            .filter(|x| x.kind == las_project::ClipKind::Audio && x.ref_id == c.id)
            .collect();
        let Some(first) = clips.first() else {
            ui.label(
                RichText::new("This sound is not in the playlist yet. Drag it from the browser to a playlist track.")
                    .weak(),
            );
            return;
        };
        let seconds = crate::playlist::sample_seconds(&project, first);
        ui.label(
            RichText::new(format!(
                "{}{} in the playlist. Changes here apply to all of them; right click one clip in the playlist to change only that one.",
                seconds.map_or(String::new(), |s| format!("{s:.2} s, ")),
                if clips.len() == 1 {
                    "1 clip".to_string()
                } else {
                    format!("{} clips", clips.len())
                }
            ))
            .weak()
            .size(11.0),
        );
        ui.separator();
        let before = first.audio.clone().unwrap_or_default();
        let out = crate::clipedit::panel(ui, &before, seconds, project.bpm, first.length);
        let ids: Vec<String> = clips.iter().map(|x| x.id.clone()).collect();
        if let Some(e) = out.edit {
            let plain = crate::clipedit::is_plain(&e);
            let edit = move |p: &mut las_project::Project| {
                for x in p.playlist.clips.iter_mut().filter(|x| ids.contains(&x.id)) {
                    x.audio = (!plain).then(|| e.clone());
                }
            };
            if out.begin {
                self.project.modify(true, edit);
            } else {
                self.project.modify(false, edit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_play_the_channel_unless_a_synth_panel_stands_in_for_it() {
        assert!(keys_to_project(false, None));
        assert!(keys_to_project(false, Some(Bound::Synth(0))));
        assert!(!keys_to_project(true, None));
        assert!(!keys_to_project(true, Some(Bound::Synth(0))));
        // Nebula's window open: a preset picked there is heard from the channel
        for b in [
            Bound::Nebula(1),
            Bound::Pads(1),
            Bound::Weave(1),
            Bound::Sampler(1),
            Bound::Basic(1),
            Bound::Hosted(1),
        ] {
            assert!(keys_to_project(true, Some(b)), "{b:?}");
        }
    }

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
