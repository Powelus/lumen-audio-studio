// SPDX-License-Identifier: GPL-3.0-or-later
//! Linking hardware knobs to controls, as FL Studio's "Link to controller": right click
//! a knob or fader, choose "Link to MIDI controller", move a knob on the MIDI keyboard,
//! and from then on that knob moves the control. The links are kept in the project
//! (`midiMap`) and listed in Settings, where they can be removed.

use crate::auto::AutoRequest;
use eframe::egui::Ui;
use las_project::create::uid;
use las_project::{MidiMapping, Project};
use serde_json::Value;
use std::cell::RefCell;

thread_local! {
    /// The control waiting for a hardware knob to be moved.
    static WAITING: RefCell<Option<AutoRequest>> = const { RefCell::new(None) };
}

/// Wait for the next controller moved to link it to `req`'s target.
pub fn learn(req: AutoRequest) {
    WAITING.with(|w| *w.borrow_mut() = Some(req));
}

/// The control waiting for a knob, if any.
pub fn waiting() -> Option<AutoRequest> {
    WAITING.with(|w| w.borrow().clone())
}

/// Stop waiting (a knob was moved, or the user cancelled).
pub fn take() -> Option<AutoRequest> {
    WAITING.with(|w| w.borrow_mut().take())
}

/// The "Link to MIDI controller" item of a control's right click menu. `req` describes
/// the control as an automation request does (its target, name and value now).
pub fn menu_item(ui: &mut Ui, req: impl FnOnce() -> AutoRequest) {
    if ui
        .button("Link to MIDI controller...")
        .on_hover_text(
            "Then move a knob or slider on your MIDI keyboard: it will move this control",
        )
        .clicked()
    {
        learn(req());
        ui.close();
    }
}

/// Link controller `cc` on MIDI channel `channel` to a target. A control has one link
/// and a controller moves one control, so older links of either are replaced.
pub fn link(p: &mut Project, channel: u8, cc: u8, target: &Value) {
    let map = p.midi_map.get_or_insert_with(Vec::new);
    map.retain(|m| &m.target != target && !(m.cc == cc && m.channel == channel));
    map.push(MidiMapping {
        id: uid("midi_"),
        channel,
        cc,
        target: target.clone(),
        ..Default::default()
    });
}

/// Remove a link by its id.
pub fn unlink(p: &mut Project, id: &str) {
    if let Some(map) = &mut p.midi_map {
        map.retain(|m| m.id != id);
        if map.is_empty() {
            p.midi_map = None;
        }
    }
}

/// The targets controller `cc` on `channel` moves.
pub fn targets(p: &Project, channel: u8, cc: u8) -> Vec<Value> {
    p.midi_map
        .iter()
        .flatten()
        .filter(|m| m.cc == cc && m.channel == channel)
        .map(|m| m.target.clone())
        .collect()
}

/// The range of a hosted plugin's parameter, given the plugin's saved state and the
/// parameter's id.
pub type RangeOf<'a> = &'a dyn Fn(&Value, u32) -> Option<(f64, f64)>;

/// Set a target to `v` (0 to 1 over its range, as automation does). Returns whether
/// it was a target that could be set.
pub fn set(p: &mut Project, target: &Value, v: f64, range_of: RangeOf) -> bool {
    let v = v.clamp(0.0, 1.0);
    let s = |k: &str| {
        target
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let track = target
        .get("track")
        .and_then(Value::as_u64)
        .map(|t| t as usize);
    let param = s("param");
    let set_param = |state: &mut Value, param: &str| -> bool {
        let Some(id) = param.strip_prefix("p.").and_then(|x| x.parse::<u32>().ok()) else {
            return false;
        };
        let Some((lo, hi)) = range_of(state, id) else {
            return false;
        };
        crate::hosted::apply(state, &[(id, lo + (hi - lo) * v)]);
        true
    };
    match s("kind").as_str() {
        "mixer" => {
            let Some(t) = track.and_then(|t| p.mixer.get_mut(t)) else {
                return false;
            };
            match param.as_str() {
                "volume" => t.volume = v * 1.25,
                "pan" => t.pan = v * 2.0 - 1.0,
                _ => return false,
            }
        }
        "master" if param == "volume" => p.master_volume = v * 1.25,
        "channel" => {
            let id = s("channelId");
            let Some(c) = p.channels.iter_mut().find(|c| c.id == id) else {
                return false;
            };
            match param.as_str() {
                "volume" => c.volume = v * 1.25,
                "pan" => c.pan = v * 2.0 - 1.0,
                "pitch" => c.pitch = v * 24.0 - 12.0,
                _ => return false,
            }
        }
        "plugin" => {
            let id = s("channelId");
            let Some(c) = p.channels.iter_mut().find(|c| c.id == id) else {
                return false;
            };
            if c.plugin_id == las_engine::weave::WEAVE_ID {
                return set_weave_param(&mut c.state, &param, v);
            }
            if set_param(&mut c.state, &param) {
                return true;
            }
            return set_instrument_param(&c.plugin_id.clone(), &mut c.state, &param, v);
        }
        "effect" => {
            let slot_id = s("slotId");
            let Some(slot) = track
                .and_then(|t| p.mixer.get_mut(t))
                .and_then(|t| t.slots.iter_mut().flatten().find(|x| x.id == slot_id))
            else {
                return false;
            };
            return set_param(&mut slot.state, &param);
        }
        _ => return false,
    }
    true
}

/// Set a built-in instrument's parameter (a dotted path into its state, such as
/// `macros.drive`) to `v`, 0 to 1 over the range automation gives it.
fn set_instrument_param(plugin_id: &str, state: &mut Value, path: &str, v: f64) -> bool {
    use las_engine::fxauto::map_automation;
    use las_engine::pluginparams::plugin_param_def;
    const BUILT_IN: [&str; 10] = [
        "sampler", "nebula", "polaris", "vortex", "monolith", "zenith", "triosc", "facet", "mist",
        "lumapad",
    ];
    if path.is_empty() || path.starts_with("p.") || !BUILT_IN.contains(&plugin_id) {
        return false;
    }
    let def = plugin_param_def(plugin_id, path);
    let Some(&(_, x)) = map_automation(Some(&def), &[(0.0, v)]).first() else {
        return false;
    };
    if !state.is_object() {
        *state = serde_json::json!({});
    }
    let mut at = state;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, k) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            at[*k] = serde_json::json!(x);
        } else {
            if !at.get(*k).is_some_and(Value::is_object) {
                at[*k] = serde_json::json!({});
            }
            at = &mut at[*k];
        }
    }
    true
}

/// Set parameter `p.<id>` of the Weave patch a channel's state holds to `v` (0 to 1
/// over the parameter's range, as Weave's automation does).
fn set_weave_param(state: &mut Value, path: &str, v: f64) -> bool {
    let Some(id) = path.strip_prefix("p.").and_then(|x| x.parse::<u32>().ok()) else {
        return false;
    };
    let mut patch = las_engine::weave::patch_of(state);
    let (node, index) = las_dsp::modular::split_param_id(id);
    let Some(n) = patch.nodes.iter_mut().find(|n| n.id == node) else {
        return false;
    };
    let Some(ps) = n.kind.spec().params.get(index) else {
        return false;
    };
    n.params.insert(ps.name.to_string(), ps.denormalize(v));
    if !state.is_object() {
        *state = serde_json::json!({});
    }
    state["patch"] = patch.to_json();
    true
}

/// The channel (by index) of a Weave parameter a link moves, if it is one.
fn weave_channel(p: &Project, target: &Value) -> Option<usize> {
    if target.get("kind").and_then(Value::as_str) != Some("plugin") {
        return None;
    }
    let id = target.get("channelId").and_then(Value::as_str)?;
    p.channels
        .iter()
        .position(|c| c.id == id && c.plugin_id == las_engine::weave::WEAVE_ID)
}

/// The range of a hosted plugin's parameter, from the plugin's own list.
fn hosted_range(state: &Value, id: u32) -> Option<(f64, f64)> {
    let list = las_host::params_of(crate::hosted::plugin_id(state));
    list.iter()
        .find(|p| p.id == id && p.max > p.min)
        .map(|p| (p.min, p.max))
}

impl crate::app::App {
    /// A controller moved: link it to the control waiting for one, or move the controls
    /// it is linked to.
    pub(crate) fn midi_control(&mut self, channel: u8, cc: u8, value: u8) {
        let v = f64::from(value) / 127.0;
        if self.project.loaded.is_none() {
            return;
        }
        if let Some(req) = take() {
            self.project.modify(true, |p| {
                link(p, channel, cc, &req.target);
                set(p, &req.target, v, &hosted_range);
            });
            self.shell.status = format!(
                "{} is linked to controller {cc}. Remove the link in Settings > MIDI.",
                req.name
            );
            return;
        }
        let Some(l) = &self.project.loaded else {
            return;
        };
        let linked = targets(&l.project, channel, cc);
        if linked.is_empty() {
            return;
        }
        // Weave takes new values while it plays, without a new player
        let weave: Vec<usize> = linked
            .iter()
            .filter_map(|t| weave_channel(&l.project, t))
            .collect();
        let live = weave.len() == linked.len() && self.project.takes_live_edits();
        let f = |p: &mut Project| {
            for t in &linked {
                set(p, t, v, &hosted_range);
            }
        };
        if live {
            self.project.modify_coalesced_live("midi-controller", f);
            for ch in weave {
                let patch = self
                    .project
                    .loaded
                    .as_ref()
                    .and_then(|l| l.project.channels.get(ch))
                    .map(|c| las_engine::weave::patch_of(&c.state));
                if let Some(patch) = patch {
                    self.send_weave_live(ch, &patch);
                }
            }
        } else {
            self.project.modify_coalesced("midi-controller", f);
        }
    }

    /// While a control waits for a knob: a note saying so, with Cancel.
    pub(crate) fn learn_banner(&mut self, ctx: &eframe::egui::Context) {
        let Some(req) = waiting() else {
            return;
        };
        let p = crate::theme::pal();
        let mut cancel = false;
        eframe::egui::Area::new(eframe::egui::Id::unique("midi-learn"))
            .anchor(eframe::egui::Align2::CENTER_TOP, [0.0, 90.0])
            .order(eframe::egui::Order::Foreground)
            .show(ctx, |ui| {
                eframe::egui::Frame::popup(ui.style())
                    .fill(p.panel2)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                eframe::egui::RichText::new(format!(
                                    "Move a knob or slider on your MIDI keyboard to link it to {}",
                                    req.name
                                ))
                                .strong(),
                            );
                            if self.midi.names.is_empty() {
                                ui.label(
                                    eframe::egui::RichText::new("(no MIDI input connected)")
                                        .color(p.hot),
                                );
                            }
                            cancel = ui.button("Cancel").clicked();
                        });
                    });
            });
        if cancel || ctx.input(|i| i.key_pressed(eframe::egui::Key::Escape)) {
            take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::{channel_target, master_target, mixer_target, plugin_target};
    use serde_json::json;

    fn no_range(_: &Value, _: u32) -> Option<(f64, f64)> {
        None
    }

    #[test]
    fn a_link_replaces_older_links_of_its_control_and_its_knob() {
        let mut p = las_project::create::empty_project("x");
        link(&mut p, 0, 20, &mixer_target(1, "volume"));
        link(&mut p, 0, 21, &mixer_target(1, "volume"));
        assert_eq!(p.midi_map.as_ref().unwrap().len(), 1);
        assert_eq!(targets(&p, 0, 21), vec![mixer_target(1, "volume")]);
        assert!(targets(&p, 0, 20).is_empty());
        // the same knob on another control moves it instead
        link(&mut p, 0, 21, &mixer_target(2, "pan"));
        assert_eq!(targets(&p, 0, 21), vec![mixer_target(2, "pan")]);
        // another MIDI channel is another knob
        link(&mut p, 1, 21, &master_target("volume"));
        assert_eq!(p.midi_map.as_ref().unwrap().len(), 2);
        let id = p.midi_map.as_ref().unwrap()[0].id.clone();
        unlink(&mut p, &id);
        assert_eq!(p.midi_map.as_ref().unwrap().len(), 1);
        let id = p.midi_map.as_ref().unwrap()[0].id.clone();
        unlink(&mut p, &id);
        assert!(p.midi_map.is_none(), "an empty list is not saved");
    }

    #[test]
    fn links_are_saved_in_the_project() {
        let mut p = las_project::create::empty_project("x");
        link(&mut p, 0, 74, &mixer_target(3, "pan"));
        let back: Project = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(targets(&back, 0, 74), vec![mixer_target(3, "pan")]);
    }

    #[test]
    fn a_knob_sets_faders_and_knobs_over_their_range() {
        let mut p = las_project::create::empty_project("x");
        let i = las_project::create::add_channel(&mut p, "sampler");
        let ch = p.channels[i].id.clone();
        assert!(set(&mut p, &mixer_target(2, "volume"), 0.5, &no_range));
        assert!((p.mixer[2].volume - 0.625).abs() < 1e-9);
        assert!(set(&mut p, &mixer_target(2, "pan"), 0.0, &no_range));
        assert_eq!(p.mixer[2].pan, -1.0);
        assert!(set(&mut p, &master_target("volume"), 1.0, &no_range));
        assert_eq!(p.master_volume, 1.25);
        assert!(set(&mut p, &channel_target(&ch, "pan"), 1.0, &no_range));
        assert_eq!(p.channels[i].pan, 1.0);
        assert!(set(&mut p, &channel_target(&ch, "volume"), 2.0, &no_range));
        assert_eq!(p.channels[i].volume, 1.25, "clamped");
        assert!(!set(&mut p, &channel_target("nope", "pan"), 0.5, &no_range));
        assert!(!set(&mut p, &json!({"kind": "what"}), 0.5, &no_range));
    }

    #[test]
    fn a_knob_sets_a_hosted_plugin_parameter() {
        let mut p = las_project::create::empty_project("x");
        let i = las_project::create::add_channel(&mut p, "nothing");
        p.channels[i].state = json!({ "id": "com.example.synth" });
        let ch = p.channels[i].id.clone();
        let range = |_: &Value, id: u32| (id == 7).then_some((100.0, 200.0));
        assert!(set(&mut p, &plugin_target(&ch, "p.7"), 0.25, &range));
        assert_eq!(crate::hosted::value_of(&p.channels[i].state, 7, 0.0), 125.0);
        assert!(!set(&mut p, &plugin_target(&ch, "p.8"), 0.25, &range));
        assert!(!set(&mut p, &plugin_target(&ch, "cutoff"), 0.25, &range));
    }

    #[test]
    fn a_knob_moves_nebulas_drive_width_and_detune_macros() {
        let mut p = las_project::create::empty_project("x");
        let i = las_project::create::add_channel(&mut p, "nebula");
        let ch = p.channels[i].id.clone();
        for (m, v) in [("drive", 0.75), ("width", 0.25), ("detune", 1.0)] {
            let t = plugin_target(&ch, &format!("macros.{m}"));
            assert!(set(&mut p, &t, v, &no_range), "{m}");
            assert!((p.channels[i].state["macros"][m].as_f64().unwrap() - v).abs() < 1e-9);
        }
    }

    #[test]
    fn a_knob_moves_a_weave_parameter_over_its_range() {
        use las_dsp::modular::{param_id, Kind, Patch};
        let mut p = las_project::create::empty_project("x");
        let i = las_project::create::add_channel(&mut p, "weave");
        let ch = p.channels[i].id.clone();
        let patch = Patch::default();
        // the default patch's filter: node 4, parameter 1 (its cutoff, on a log scale)
        let n = patch.node(4).unwrap();
        assert_eq!(n.kind, Kind::Filter);
        let ps = n.kind.spec().params[1];
        let t = plugin_target(&ch, &format!("p.{}", param_id(4, 1)));
        assert!(set(&mut p, &t, 0.0, &no_range));
        let got = las_engine::weave::patch_of(&p.channels[i].state);
        assert!((got.node(4).unwrap().params[ps.name] - ps.min).abs() < 1e-9);
        assert!(set(&mut p, &t, 0.5, &no_range));
        let got = las_engine::weave::patch_of(&p.channels[i].state);
        assert!((got.node(4).unwrap().params[ps.name] - ps.denormalize(0.5)).abs() < 1e-6);
        assert_eq!(weave_channel(&p, &t), Some(i));
        // a module that is not there, or a parameter it does not have
        assert!(!set(
            &mut p,
            &plugin_target(&ch, &format!("p.{}", param_id(99, 0))),
            0.5,
            &no_range
        ));
        assert!(!set(
            &mut p,
            &plugin_target(&ch, &format!("p.{}", param_id(4, 60))),
            0.5,
            &no_range
        ));
    }

    #[test]
    fn learning_waits_for_one_knob() {
        learn(AutoRequest {
            target: mixer_target(1, "volume"),
            name: "Kick volume".into(),
            start: 0.5,
        });
        assert_eq!(waiting().map(|r| r.name), Some("Kick volume".into()));
        assert!(take().is_some());
        assert!(waiting().is_none());
    }
}
