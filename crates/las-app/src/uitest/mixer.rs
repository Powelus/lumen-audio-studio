// SPDX-License-Identifier: GPL-3.0-or-later
//! The mixer clicked through as a user does: effects in slots, their windows, mix
//! knobs, routing, renaming, the insert menu, and a save that keeps it all.

use super::Harness;
use eframe::egui::{accesskit::Role, vec2, Key, Modifiers, PointerButton};

/// The demo song with only the mixer open, insert 1 selected.
fn mixer() -> Harness {
    // tall enough for the strips' routing buttons without scrolling
    let mut h = Harness::demo_sized(1600.0, 1300.0);
    h.app.shell.open.rack = false;
    h.app.shell.open.playlist = false;
    h.key(Key::F9);
    h.run(3);
    pill(&mut h, 1);
    h
}

/// The name pills of the strips, left to right.
fn pills(h: &Harness) -> Vec<super::Node> {
    let mut v = h.find_prefix("Click to select, double-click to rename");
    v.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    v
}

fn pill(h: &mut Harness, i: usize) {
    let p = pills(h)[i].clone();
    h.click_at(p.rect.center());
    assert_eq!(h.app.project.mixer_state.selected, i);
}

fn empty_insert(h: &mut Harness, i: usize) {
    h.app.project.modify(true, |p| {
        p.mixer[i].slots = vec![None; las_project::MIXER_SLOTS];
    });
    h.run(2);
}

/// The slot mix knobs of the selected insert, top to bottom.
fn mix_knobs(h: &Harness) -> Vec<super::Node> {
    let mut v = h.find_prefix("Mix ");
    v.retain(|n| n.label.contains("0% dry"));
    v.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
    v
}

#[test]
fn every_effect_goes_in_a_slot_opens_switches_off_and_undo_takes_it_away() {
    let mut h = mixer();
    for d in las_dsp::fx::registry::descriptors() {
        empty_insert(&mut h, 1);
        let undo_at = h.app.project.undo_len();
        h.app.project.edit_mixer(&[
            crate::mixer::MixEdit::Begin,
            crate::mixer::MixEdit::AddEffect {
                insert: 1,
                slot: 0,
                plugin: d.id.clone(),
            },
        ]);
        h.run(2);
        let label = d.id.strip_prefix("las-").unwrap_or(&d.id).to_string();
        let b = h
            .find_all(&label)
            .into_iter()
            .find(|n| n.role == Role::Button)
            .unwrap_or_else(|| panic!("the slot of {} shows", d.id));
        h.click_at(b.rect.center());
        h.run(2);
        assert_eq!(h.app.project.open_effect, Some((1, 0)), "{} opens", d.id);
        // every button of its window (presets, switches, modes) works
        super::flows::mash(&mut h, "Effect");
        if h.project().mixer[1].slots[0].is_none() {
            // its Remove button was among them: put it back for the rest
            h.key_with(Key::Z, Modifiers::COMMAND);
        }
        assert!(h.project().mixer[1].slots[0].is_some(), "{}", d.id);
        // the light switches it off and on
        let led = h
            .find_all("Switch on or off")
            .into_iter()
            .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
            .unwrap();
        let was = h.project().mixer[1].slots[0].as_ref().unwrap().enabled;
        h.click_at(led.rect.center());
        assert_ne!(
            h.project().mixer[1].slots[0].as_ref().unwrap().enabled,
            was,
            "{}",
            d.id
        );
        // its mix knob turns with the wheel
        let k = mix_knobs(&h)[0].clone();
        h.wheel_at(k.rect.center(), -60.0);
        let mix = h.project().mixer[1].slots[0].as_ref().unwrap().mix;
        assert!(mix < 1.0, "{}: the wheel turns the mix down: {mix}", d.id);
        // undo takes all of it back
        while h.app.project.undo_len() > undo_at {
            h.key_with(Key::Z, Modifiers::COMMAND);
        }
        assert!(
            h.project().mixer[1].slots[0].is_none(),
            "{}: undo empties the slot",
            d.id
        );
        h.app.project.open_effect = None;
    }
}

#[test]
fn the_slot_mix_knob_resets_automates_and_reorders_with_its_slot() {
    let mut h = mixer();
    empty_insert(&mut h, 1);
    for (slot, id) in [(0, "las-delay"), (1, "las-reverb")] {
        h.app
            .project
            .edit_mixer(&[crate::mixer::MixEdit::AddEffect {
                insert: 1,
                slot,
                plugin: id.into(),
            }]);
    }
    h.run(2);
    let k = mix_knobs(&h);
    assert_eq!(k.len(), 2, "a mix knob beside each effect");
    // drag down, then Ctrl+click back to 100%
    h.drag(k[1].rect.center(), k[1].rect.center() + vec2(0.0, 40.0));
    let m = h.project().mixer[1].slots[1].as_ref().unwrap().mix;
    assert!(m < 0.9, "the drag lowers the mix: {m}");
    h.click_with(
        k[1].rect.center(),
        PointerButton::Primary,
        Modifiers::COMMAND,
    );
    assert_eq!(h.project().mixer[1].slots[1].as_ref().unwrap().mix, 1.0);
    h.drag(k[1].rect.center(), k[1].rect.center() + vec2(0.0, 30.0));
    let m = h.project().mixer[1].slots[1].as_ref().unwrap().mix;
    // move the reverb up: its mix goes with it
    let rv = h
        .find_all("reverb")
        .into_iter()
        .find(|n| n.role == Role::Button)
        .unwrap();
    h.right_click_at(rv.rect.center());
    h.click("Move up");
    let s = &h.project().mixer[1].slots;
    assert_eq!(s[0].as_ref().unwrap().plugin_id, "las-reverb");
    assert_eq!(s[0].as_ref().unwrap().mix, m);
    // right click: an automation clip of the mix
    let k = mix_knobs(&h)[0].clone();
    h.right_click_at(k.rect.center());
    assert!(h.has("Link to MIDI controller..."));
    h.click("Create automation clip");
    h.run(2);
    let c = h.project().automation.last().unwrap();
    assert_eq!(c.target["kind"], "effect");
    assert_eq!(c.target["param"], "mix");
    assert_eq!(
        c.target["slotId"].as_str(),
        Some(h.project().mixer[1].slots[0].as_ref().unwrap().id.as_str())
    );
}

#[test]
fn rename_colour_solo_reset_route_and_move_an_insert_by_hand() {
    let mut h = mixer();
    // double click the name and type a new one
    let p = pills(&h)[2].clone();
    h.double_click_at(p.rect.center());
    assert!(
        h.app.project.mixer_state.renaming.is_some(),
        "a double click starts typing"
    );
    h.key_with(Key::A, Modifiers::COMMAND);
    h.type_text("Lead bus");
    h.key(Key::Enter);
    h.run(2);
    assert_eq!(h.project().mixer[2].name, "Lead bus");
    // right click: a colour
    let p = pills(&h)[2].clone();
    h.right_click_at(p.rect.center());
    h.click(crate::mixer::COLOURS[6]);
    assert_eq!(h.project().mixer[2].color, crate::mixer::COLOURS[6]);
    // Alt+click on mute solos
    let mutes: Vec<_> = {
        let mut v = h.find_all("M");
        let strip = pills(&h)[2].rect;
        v.retain(|n| {
            n.role != Role::Label
                && n.rect.left() >= strip.left() - 4.0
                && n.rect.right() <= strip.right() + 4.0
        });
        v.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
        v
    };
    h.set_modifiers(Modifiers::ALT);
    h.click_with(
        mutes[0].rect.center(),
        PointerButton::Primary,
        Modifiers::ALT,
    );
    h.set_modifiers(Modifiers::NONE);
    assert!(
        h.project().mixer[2].solo && !h.project().mixer[2].mute,
        "{} {}",
        h.project().mixer[2].solo,
        h.project().mixer[2].mute
    );
    h.click_at(mutes[0].rect.center());
    assert!(h.project().mixer[2].mute);
    // Ctrl+click on a fader is 0 dB
    let mut f: Vec<_> = h
        .nodes
        .iter()
        .filter(|n| n.label.contains(" dB\nDrag, Shift for fine"))
        .cloned()
        .collect();
    f.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    h.drag(f[2].rect.center(), f[2].rect.center() + vec2(0.0, 50.0));
    assert!(h.project().mixer[2].volume < 0.9);
    h.set_modifiers(Modifiers::COMMAND);
    h.click_with(
        f[2].rect.center(),
        PointerButton::Primary,
        Modifiers::COMMAND,
    );
    h.set_modifiers(Modifiers::NONE);
    assert_eq!(h.project().mixer[2].volume, 1.0);
    // the wheel moves it a little
    h.wheel_at(f[2].rect.center(), -40.0);
    let v = h.project().mixer[2].volume;
    assert!(v < 1.0 && v > 0.8, "a small step down: {v}");
    // the routing button under insert 3 sends the selected insert (1) there
    pill(&mut h, 1);
    let before = h.project().mixer[1].sends.len();
    let route = h.get(&format!(
        "Click to send {} to {}.",
        h.project().mixer[1].name,
        h.project().mixer[3].name
    ));
    // the strips are taller than the window: scroll down to their feet
    let at = pills(&h)[3].rect.center();
    h.wheel_at(at, -400.0);
    let route = h.get(&route.label);
    h.click_at(route.rect.center());
    assert_eq!(h.project().mixer[1].sends.len(), before + 1, "{route:?}");
    assert_eq!(h.project().mixer[1].sends.last().unwrap().target, 3);
    // its send level knob turns
    let knob = h
        .find_prefix(&format!("Send level from {}", h.project().mixer[1].name))
        .into_iter()
        .find(|n| (n.rect.center().x - route.rect.center().x).abs() < 20.0)
        .expect("a send level knob under the button");
    h.drag(knob.rect.center(), knob.rect.center() + vec2(0.0, -30.0));
    assert!(
        h.project().mixer[1].sends.last().unwrap().amount > 0.5,
        "{knob:?} {route:?} {}",
        h.project().mixer[1].sends.last().unwrap().amount
    );
    // move insert 2 right: its name and the channels follow
    h.wheel_at(knob.rect.center() + vec2(30.0, 0.0), 400.0);
    let chans: Vec<usize> = (0..h.project().channels.len())
        .filter(|&c| h.project().channels[c].mixer_track == 2)
        .collect();
    let p = pills(&h)[2].clone();
    h.right_click_at(p.rect.center());
    h.click("Move right");
    assert_eq!(h.project().mixer[3].name, "Lead bus");
    for c in chans {
        assert_eq!(h.project().channels[c].mixer_track, 3);
    }
    // "Select all channels routed here" filters the rack
    let fr = pills(&h)[3].clone();
    let below = fr.rect.center() + vec2(0.0, 60.0);
    h.right_click_at(below);
    if h.has("Select all channels routed here") {
        let b = h.get("Select all channels routed here");
        if !b.disabled {
            h.click("Select all channels routed here");
            assert_eq!(
                h.app.shell.steps.filter,
                crate::steps::GroupFilter::Insert(3)
            );
        }
    }
}

#[test]
fn an_effect_dragged_from_the_browser_lands_in_the_slot_it_is_let_go_on() {
    let mut h = mixer();
    h.app.shell.browser = true;
    h.app.shell.tab = crate::shell::BrowserTab::Plugins;
    h.run(3);
    empty_insert(&mut h, 1);
    let name = las_dsp::fx::registry::descriptor("las-chorus")
        .unwrap()
        .name
        .clone();
    let src = h
        .find_all(&name)
        .into_iter()
        .min_by(|a, b| a.rect.left().total_cmp(&b.rect.left()))
        .expect("the effect in the browser");
    let slot = h
        .find_all("+  Slot 3")
        .into_iter()
        .next()
        .or_else(|| {
            h.find_prefix("+")
                .into_iter()
                .find(|n| n.label.contains("Slot 3"))
        })
        .expect("slot 3");
    h.press_at(src.rect.center());
    h.move_to(slot.rect.center(), 12);
    assert!(
        eframe::egui::DragAndDrop::has_any_payload(&h.ctx),
        "the effect is being dragged"
    );
    h.release();
    h.run(2);
    let s = &h.project().mixer[1].slots;
    assert_eq!(
        s[2].as_ref().map(|s| s.plugin_id.as_str()),
        Some("las-chorus"),
        "the chorus is in slot 3"
    );
}

#[test]
fn the_mixer_is_kept_by_a_save_and_reopen() {
    let mut h = mixer();
    empty_insert(&mut h, 1);
    h.app.project.edit_mixer(&[
        crate::mixer::MixEdit::AddEffect {
            insert: 1,
            slot: 0,
            plugin: "las-delay".into(),
        },
        crate::mixer::MixEdit::SlotMix {
            insert: 1,
            slot: 0,
            mix: 0.35,
        },
        crate::mixer::MixEdit::SlotOn {
            insert: 1,
            slot: 0,
            on: false,
        },
        crate::mixer::MixEdit::Color {
            insert: 1,
            color: "#22d3ee".into(),
        },
        crate::mixer::MixEdit::Rename {
            insert: 1,
            name: "Echo".into(),
        },
        crate::mixer::MixEdit::AddSend {
            insert: 1,
            target: 4,
        },
        crate::mixer::MixEdit::Solo {
            insert: 2,
            on: true,
        },
        crate::mixer::MixEdit::Mute {
            insert: 3,
            on: true,
        },
        crate::mixer::MixEdit::Pan {
            insert: 1,
            value: -0.4,
        },
    ]);
    h.run(2);
    let dir = std::env::temp_dir().join(format!("las-uitest-mix-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("mix.las").to_string_lossy().into_owned();
    h.app.project.save_to(&path).unwrap();
    let saved = serde_json::to_value(&h.project().mixer).unwrap();
    h.menu(&["File", "New Ctrl+N"]);
    h.app.open_project(&path);
    h.run(3);
    let back = serde_json::to_value(&h.project().mixer).unwrap();
    assert_eq!(saved, back, "the mixer comes back as it was saved");
    let s = h.project().mixer[1].slots[0].as_ref().unwrap();
    assert!(!s.enabled && (s.mix - 0.35).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(&dir);
}
