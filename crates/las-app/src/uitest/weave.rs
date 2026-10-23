// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave's window driven as a user does: voices, controls turned while a note plays,
//! cables, presets, undo.

use super::Harness;
use crate::weave_ui::patch_of;
use eframe::egui::{pos2, vec2, Modifiers};
use las_dsp::modular::{factory, Kind, Patch};
use las_engine::{PlayMode, Player};
use std::sync::Arc;

/// The demo with a Weave channel added and its window open, as if a player were
/// playing it (so edits are sent live). The channel's index.
fn weave() -> (Harness, usize) {
    let mut h = Harness::demo();
    h.menu(&["Add", "Weave"]);
    let ch = h.project().channels.len() - 1;
    h.app.open_channel(ch);
    h.run(4);
    // the harness has no sound device: pretend a player has the project, so that
    // edits take the live path (what is sent is kept in `sent`)
    h.app.project.has_player = true;
    (h, ch)
}

fn patch(h: &Harness, ch: usize) -> Patch {
    patch_of(&h.project().channels[ch].state)
}

/// A player for the project as it is, holding a note on the channel.
fn player(h: &Harness, ch: usize) -> Player {
    let p = Arc::new(h.project().clone());
    let mut pl = Player::new(p, &Default::default(), 48_000.0, PlayMode::Song).unwrap();
    pl.note_on(ch, 57.0, 1.0);
    pl.note_on(ch, 64.0, 1.0);
    pl
}

/// Whether the player is heard within a second or so.
fn plays(pl: &mut Player) -> bool {
    (0..12).any(|_| rms(pl) > 1e-5)
}

fn rms(pl: &mut Player) -> f32 {
    let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
    pl.process(&mut l, &mut r);
    (l.iter().map(|x| x * x).sum::<f32>() / 4096.0).sqrt()
}

/// What a filtered note sounds like: energy above the fundamental, from the
/// difference of neighbouring samples.
fn brightness(pl: &mut Player) -> f32 {
    let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
    pl.process(&mut l, &mut r);
    l.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>()
}

/// Hand the player every patch the editor sent live since `from`.
fn deliver(h: &Harness, pl: &mut Player, ch: usize, from: usize) {
    for (c, p) in &h.app.weave.sent[from..] {
        assert_eq!(*c, ch);
        drop(pl.weave_patch(*c, Box::new(p.clone())));
    }
}

#[test]
fn dragging_a_whole_number_control_does_not_hang_and_changes_it() {
    // the crash: the oscillator's unison `voices` (a whole number) read the drag
    // while it held the context's lock, so the window froze for good
    let (mut h, ch) = weave();
    let osc = 2;
    let at = h.app.weave.row_at(&patch(&h, ch), osc, 4 + 6);
    h.drag(at, at - vec2(0.0, 120.0));
    let v = patch(&h, ch).node(osc).unwrap().param("voices");
    assert!(v > 3.0, "dragged up: {v}");
    h.drag(at, at + vec2(0.0, 400.0));
    assert_eq!(patch(&h, ch).node(osc).unwrap().param("voices"), 1.0);
}

#[test]
fn voices_change_again_and_again_while_notes_play_without_a_panic() {
    let (mut h, ch) = weave();
    let mut pl = player(&h, ch);
    rms(&mut pl);
    let field = h
        .get("How many keys play at once (drag, type or use the wheel)")
        .rect
        .center();
    let osc_voices = h.app.weave.row_at(&patch(&h, ch), 2, 10);
    let mut seen = std::collections::BTreeSet::new();
    for round in 0..6 {
        let from = h.app.weave.sent.len();
        let dy = if round % 2 == 0 { 300.0 } else { -300.0 };
        // the patch's voices, dragged right across and back
        h.press_at(field);
        h.move_to(field + vec2(dy, 0.0), 6);
        deliver(&h, &mut pl, ch, from);
        assert!(rms(&mut pl).is_finite());
        h.move_to(field - vec2(dy, 0.0), 6);
        h.release();
        // and the oscillator's unison voices
        h.press_at(osc_voices);
        h.move_to(osc_voices - vec2(0.0, dy), 6);
        h.release();
        deliver(&h, &mut pl, ch, from);
        pl.note_on(ch, 60.0 + round as f64, 0.8);
        let level = rms(&mut pl);
        assert!(level.is_finite() && level > 0.0, "round {round}: {level}");
        seen.insert(patch(&h, ch).voices);
        // mono and back
        h.click("Mono");
        deliver(&h, &mut pl, ch, from);
        rms(&mut pl);
    }
    assert!(h.app.weave.sent.len() > 12, "the changes went out live");
    let v = patch(&h, ch).voices;
    assert!((1..=las_dsp::modular::MAX_VOICES).contains(&v));
    // a new player of the patch as it is now plays too
    let mut again = player(&h, ch);
    assert!(plays(&mut again));
}

#[test]
fn a_knob_drag_changes_the_sound_before_the_button_is_let_go() {
    let (mut h, ch) = weave();
    let mut pl = player(&h, ch);
    for _ in 0..3 {
        brightness(&mut pl);
    }
    let dull = brightness(&mut pl);
    let undo_before = h.app.project.history().0.len();
    let filter = 4;
    let cutoff = h.app.weave.row_at(&patch(&h, ch), filter, 2);
    let start = patch(&h, ch).node(filter).unwrap().param("cutoff");
    h.press_at(cutoff);
    h.move_to(cutoff - vec2(0.0, 60.0), 8);
    // still held: the knob is being turned and its value is already out
    assert_eq!(h.app.weave.turning(), Some((filter, 1)));
    let sent = h
        .app
        .weave
        .sent
        .last()
        .expect("sent live while dragging")
        .1
        .clone();
    let now = sent.node(filter).unwrap().param("cutoff");
    assert!(now > start * 2.0, "{start} -> {now}");
    deliver(&h, &mut pl, ch, 0);
    brightness(&mut pl);
    let bright = brightness(&mut pl);
    assert!(bright > dull * 1.1, "heard while held: {dull} -> {bright}");
    // and finer with shift
    h.set_modifiers(Modifiers::SHIFT);
    h.move_to(cutoff - vec2(0.0, 90.0), 4);
    let fine = patch(&h, ch).node(filter).unwrap().param("cutoff");
    h.set_modifiers(Modifiers::NONE);
    assert!(fine > now && fine < now * 1.3, "{now} -> {fine}");
    h.release();
    assert_eq!(h.app.weave.turning(), None);
    // the whole drag is one undo step, and undo takes it back
    assert_eq!(h.app.project.history().0.len(), undo_before + 1);
    h.app.project.undo();
    h.run(2);
    assert_eq!(patch(&h, ch).node(filter).unwrap().param("cutoff"), start);
    // a double click puts it back to its default
    h.press_at(cutoff);
    h.move_to(cutoff - vec2(0.0, 50.0), 4);
    h.release();
    h.double_click_at(cutoff);
    let ps = Kind::Filter.param("cutoff").unwrap();
    assert_eq!(
        patch(&h, ch).node(filter).unwrap().param("cutoff"),
        ps.default
    );
}

#[test]
fn the_wheel_turns_the_control_under_the_pointer() {
    let (mut h, ch) = weave();
    let res = h.app.weave.row_at(&patch(&h, ch), 4, 3);
    let before = patch(&h, ch).node(4).unwrap().param("res");
    h.wheel_at(res, 150.0);
    let after = patch(&h, ch).node(4).unwrap().param("res");
    assert!(after > before + 0.03, "{before} -> {after}");
    // a whole number moves a step a notch
    let wave = h.app.weave.row_at(&patch(&h, ch), 2, 4);
    let w0 = patch(&h, ch).node(2).unwrap().param("wave");
    h.wheel_at(wave, -50.0);
    assert_eq!(patch(&h, ch).node(2).unwrap().param("wave"), w0 - 1.0);
}

#[test]
fn a_cable_is_plugged_in_pulled_out_and_moved() {
    let (mut h, ch) = weave();
    let p0 = patch(&h, ch);
    let n = p0.cables.len();
    // the envelope's output into the resonance of the filter (a row, not just its dot)
    let from = h.app.weave.out_at(&p0, 3, 0);
    let to = h.app.weave.row_at(&p0, 4, 3);
    h.drag(from, to);
    let p1 = patch(&h, ch);
    assert_eq!(p1.cables.len(), n + 1, "plugged in");
    let c = p1.cables.last().unwrap();
    assert_eq!(
        (c.from, c.output.as_str(), c.to, c.input.as_str()),
        (3, "out", 4, "res")
    );
    // let go anywhere else: nothing
    let from = h.app.weave.out_at(&p1, 3, 0);
    h.drag(from, from + vec2(5.0, 300.0));
    assert_eq!(patch(&h, ch).cables.len(), n + 1);
    // drag from the plugged input to the filter's mode row: the cable moves
    let k = Kind::Filter.input_index("res").unwrap();
    let dot = h.app.weave.in_at(&p1, 4, k);
    let mode = h.app.weave.row_at(&p1, 4, 1);
    h.drag(dot, mode);
    let p2 = patch(&h, ch);
    assert_eq!(p2.cables.len(), n + 1);
    assert_eq!(p2.cables.last().unwrap().input, "mode");
    // a double click on its handle pulls it out
    let at = h.app.weave.cable_at(&p2, p2.cables.len() - 1);
    h.double_click_at(at);
    assert_eq!(patch(&h, ch).cables.len(), n, "pulled out");
    // a click selects one and Delete pulls it out
    let p3 = patch(&h, ch);
    let at = h.app.weave.cable_at(&p3, 0);
    h.click_at(at);
    assert!(h.has("amount"), "its amount is shown");
    h.key(eframe::egui::Key::Delete);
    assert_eq!(patch(&h, ch).cables.len(), n - 1);
    h.app.project.undo();
    h.run(2);
    assert_eq!(patch(&h, ch).cables.len(), n);
}

#[test]
fn every_preset_loads_and_plays_and_nothing_stale_is_left() {
    let (mut h, ch) = weave();
    // a cable picked, then each patch loaded
    let p0 = patch(&h, ch);
    let at = h.app.weave.cable_at(&p0, 0);
    h.click_at(at);
    for (name, fp) in factory() {
        // the entry in the menu (a channel may have the same name)
        let menu = h.get("Patches").rect;
        h.click_at(menu.center());
        let entry = h
            .find_all(name)
            .into_iter()
            .min_by(|a, b| {
                let d = |n: &super::Node| (n.rect.left() - menu.left()).abs();
                d(a).total_cmp(&d(b))
            })
            .unwrap_or_else(|| panic!("no {name} in the menu"));
        h.click_at(entry.rect.center());
        h.run(2);
        let now = patch(&h, ch);
        assert_eq!(now, fp, "{name}");
        assert!(
            !h.has("amount"),
            "{name}: no cable of the last patch is still picked"
        );
        let mut pl = player(&h, ch);
        assert!(plays(&mut pl), "{name} plays");
    }
    // a module still moves by its title after all that, and only its place changes
    let p = patch(&h, ch);
    let title = h.find_prefix("Keys (Sources)")[0].rect.center();
    h.drag(title, title + vec2(60.0, 40.0));
    let moved = patch(&h, ch);
    let (a, b) = (p.node(1).unwrap(), moved.node(1).unwrap());
    assert!(
        b.pos[0] > a.pos[0] + 20.0 && b.pos[1] > a.pos[1] + 10.0,
        "{:?} -> {:?}",
        a.pos,
        b.pos
    );
    assert_eq!(a.params, b.params);
    assert_eq!(p.cables, moved.cables);
}

#[test]
fn a_weave_knob_links_to_a_midi_controller_and_follows_it_live() {
    let (mut h, ch) = weave();
    let filter = 4;
    let ps = Kind::Filter.spec().params[1];
    let cutoff = h.app.weave.row_at(&patch(&h, ch), filter, 2);
    h.right_click_at(cutoff);
    h.run(2);
    h.click("Link to MIDI controller...");
    assert!(crate::midilearn::waiting().is_some(), "waits for a knob");
    // the knob moved on the keyboard: linked, and the control takes its value
    h.app.midi_control(0, 21, 0);
    h.run(1);
    assert!(crate::midilearn::waiting().is_none());
    let map = h.project().midi_map.clone().unwrap_or_default();
    assert_eq!(map.len(), 1);
    assert_eq!(map[0].cc, 21);
    let v = patch(&h, ch).node(filter).unwrap().param(ps.name);
    assert!((v - ps.min).abs() < 1e-6, "{v}");
    // turned again while the player has the project: heard at once, no new player
    let from = h.app.weave.sent.len();
    h.app.midi_control(0, 21, 127);
    h.run(1);
    let v = patch(&h, ch).node(filter).unwrap().param(ps.name);
    assert!((v - ps.max).abs() < 1e-3, "{v}");
    let sent = &h.app.weave.sent[from..];
    assert!(
        sent.iter().any(
            |(c, p)| *c == ch && (p.node(filter).unwrap().param(ps.name) - ps.max).abs() < 1e-3
        ),
        "sent live"
    );
    // another knob moves nothing
    h.app.midi_control(0, 22, 0);
    let after = patch(&h, ch).node(filter).unwrap().param(ps.name);
    assert!((after - ps.max).abs() < 1e-3);
}

/// The choice popup's entries shown now (each is a selectable label in the popup).
fn shown_choices(h: &Harness, names: &[&str]) -> usize {
    names.iter().filter(|n| h.has(n)).count()
}

#[test]
fn a_left_click_on_a_choice_opens_a_dropdown_and_a_pick_sets_it() {
    let (mut h, ch) = weave();
    let osc = 2;
    let waves = las_dsp::modular::OSC_WAVES;
    let wave = h.app.weave.row_at(&patch(&h, ch), osc, 4);
    assert_eq!(shown_choices(&h, &waves), 0);
    h.click_at(wave);
    assert_eq!(
        shown_choices(&h, &waves),
        waves.len(),
        "the dropdown is open"
    );
    h.click_at(h.get("triangle").rect.center());
    assert_eq!(
        patch(&h, osc as usize * 0 + ch)
            .node(osc)
            .unwrap()
            .param("wave"),
        1.0
    );
    assert_eq!(shown_choices(&h, &waves), 0, "closed on a pick");
    // a click that wobbles a little still opens it, and a click outside closes it
    h.press_at(wave);
    h.move_to(wave + vec2(2.0, 1.0), 2);
    h.release();
    h.run(2);
    assert_eq!(
        shown_choices(&h, &waves),
        waves.len(),
        "opened by a shaky click"
    );
    let away = h.app.weave.row_at(&patch(&h, ch), 4, 2) + vec2(0.0, 300.0);
    h.click_at(away);
    assert_eq!(shown_choices(&h, &waves), 0, "closed by a click outside");
    assert_eq!(patch(&h, ch).node(osc).unwrap().param("wave"), 1.0);
}

#[test]
fn a_right_click_on_a_choice_shows_only_its_own_menu() {
    let (mut h, ch) = weave();
    let waves = las_dsp::modular::OSC_WAVES;
    let wave = h.app.weave.row_at(&patch(&h, ch), 2, 4);
    h.right_click_at(wave);
    assert!(h.has("Create automation clip"));
    assert!(h.has("Reset to default"));
    assert_eq!(shown_choices(&h, &waves), 0, "no dropdown under the menu");
    assert!(!h.has("Paste Ctrl+V"), "no canvas menu as well");
    assert!(!h.has("Add module here"), "no canvas menu as well");
}

#[test]
fn a_choice_works_at_any_zoom_with_arrows_wheel_and_drag() {
    let (mut h, ch) = weave();
    let osc = 2;
    let waves = las_dsp::modular::OSC_WAVES;
    let wave = |h: &Harness| patch(h, ch).node(osc).unwrap().param("wave");
    for z in [0.5, 1.0, 1.8] {
        h.app.weave.set_zoom(z);
        h.app.weave.set_pan(vec2(20.0, 20.0));
        h.run(3);
        let at = h.app.weave.row_at(&patch(&h, ch), osc, 4);
        h.click_at(at);
        assert_eq!(shown_choices(&h, &waves), waves.len(), "open at zoom {z}");
        h.click_at(h.get("square").rect.center());
        assert_eq!(wave(&h), 3.0, "picked at zoom {z}");
        assert_eq!(shown_choices(&h, &waves), 0);
        // the arrows step (round from the last to the first) without opening the list
        let r = h.app.weave.row_rect_at(&patch(&h, ch), osc, 4);
        h.click_at(r.right_center() - vec2(3.0, 0.0));
        assert_eq!(wave(&h), 4.0, "zoom {z}");
        h.click_at(r.right_center() - vec2(3.0, 0.0));
        assert_eq!(wave(&h), 0.0, "zoom {z}");
        h.click_at(r.left_center() + vec2(3.0, 0.0));
        assert_eq!(wave(&h), 4.0, "zoom {z}");
        assert_eq!(shown_choices(&h, &waves), 0);
        // the wheel steps it
        h.wheel_at(at, -50.0);
        assert_eq!(wave(&h), 3.0, "zoom {z}");
    }
    // a drag steps through the choices
    h.app.weave.set_zoom(1.0);
    h.run(2);
    let at = h.app.weave.row_at(&patch(&h, ch), osc, 4);
    let w0 = wave(&h);
    h.drag(at, at + vec2(0.0, 90.0));
    assert!(wave(&h) < w0, "{w0} -> {}", wave(&h));
    assert_eq!(
        shown_choices(&h, &waves),
        0,
        "a drag does not open the list"
    );
}

#[test]
fn every_choice_of_every_module_opens_and_picks() {
    let (mut h, ch) = weave();
    for k in Kind::ALL {
        let Some((pi, ps)) = k
            .spec()
            .params
            .iter()
            .enumerate()
            .find(|(_, ps)| k.choices(ps.name).is_some())
        else {
            continue;
        };
        let choices = k.choices(ps.name).unwrap();
        h.menu(&["Add module"]);
        h.click(las_dsp::modular::CATEGORIES[k.category()]);
        h.click(crate::weave_ui::kind_name(k));
        h.run(2);
        let id = *h
            .app
            .weave
            .picked()
            .last()
            .expect("the new module is picked");
        assert_eq!(patch(&h, ch).node(id).unwrap().kind, k);
        let row = k.spec().inputs.len() + pi;
        let at = h.app.weave.row_at(&patch(&h, ch), id, row);
        h.click_at(at);
        let last = choices[choices.len() - 1];
        let entry = h
            .find_all(last)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{k:?} {}: no list", ps.name));
        h.click_at(entry.rect.center());
        assert_eq!(
            patch(&h, ch).node(id).unwrap().param(ps.name),
            (choices.len() - 1) as f64,
            "{k:?}"
        );
        h.key(eframe::egui::Key::Delete);
        assert!(patch(&h, ch).node(id).is_none(), "{k:?} deleted");
    }
}

#[test]
fn a_new_cable_into_a_single_input_replaces_the_old_one() {
    let (mut h, ch) = weave();
    let p0 = patch(&h, ch);
    // the envelope's output into the filter's audio input (the oscillator is there)
    let from = h.app.weave.out_at(&p0, 3, 0);
    let to = h.app.weave.in_at(&p0, 4, 0);
    h.drag(from, to);
    let into: Vec<u32> = patch(&h, ch)
        .cables
        .iter()
        .filter(|c| c.to == 4 && c.input == "in")
        .map(|c| c.from)
        .collect();
    assert_eq!(into, vec![3]);
    h.app.project.undo();
    h.run(2);
    assert_eq!(patch(&h, ch), p0);
}

#[test]
fn modules_are_picked_copied_pasted_duplicated_and_deleted() {
    let (mut h, ch) = weave();
    let p0 = patch(&h, ch);
    let n = p0.nodes.len();
    // a click picks one, a Ctrl click adds another
    h.click_at(h.app.weave.title_at(&p0, 2));
    assert_eq!(h.app.weave.picked(), vec![2]);
    h.click_with(
        h.app.weave.title_at(&p0, 4),
        eframe::egui::PointerButton::Primary,
        Modifiers::COMMAND,
    );
    assert_eq!(h.app.weave.picked(), vec![2, 4]);
    // copy, then paste where the pointer is
    h.key_with(eframe::egui::Key::C, Modifiers::COMMAND);
    let spot = h.app.weave.row_at(&p0, 6, 0) + vec2(0.0, 250.0);
    h.hover(spot);
    h.key_with(eframe::egui::Key::V, Modifiers::COMMAND);
    let p1 = patch(&h, ch);
    assert_eq!(p1.nodes.len(), n + 2);
    let new = h.app.weave.picked();
    assert_eq!(new.len(), 2);
    assert!(new.iter().all(|id| !p0.nodes.iter().any(|x| x.id == *id)));
    // the cable between the two came too
    assert!(p1.cables.iter().any(|c| c.from == new[0] && c.to == new[1]));
    let w = h.app.weave.world_at(spot);
    let corner = new
        .iter()
        .map(|id| p1.node(*id).unwrap().pos)
        .fold([f32::MAX; 2], |m, q| [m[0].min(q[0]), m[1].min(q[1])]);
    assert!((corner[0] - w.x).abs() < 2.0 && (corner[1] - w.y).abs() < 2.0);
    // Ctrl+D duplicates the pasted ones
    h.key_with(eframe::egui::Key::D, Modifiers::COMMAND);
    assert_eq!(patch(&h, ch).nodes.len(), n + 4);
    // Delete takes the picked ones out
    h.hover(spot);
    h.key(eframe::egui::Key::Delete);
    assert_eq!(patch(&h, ch).nodes.len(), n + 2);
    // undo brings them back, step by step
    h.app.project.undo();
    h.run(2);
    assert_eq!(patch(&h, ch).nodes.len(), n + 4);
    h.app.project.undo();
    h.app.project.undo();
    h.run(2);
    assert_eq!(patch(&h, ch), p0);
}

#[test]
fn a_box_drag_on_the_empty_canvas_picks_modules_and_the_menu_has_the_edits() {
    let (mut h, ch) = weave();
    h.app.weave.set_zoom(1.0);
    h.app.weave.set_pan(vec2(120.0, 80.0));
    h.run(3);
    let p0 = patch(&h, ch);
    // a box from just above and left of the oscillator over to the filter's title
    let a = h.app.weave.title_at(&p0, 2) - vec2(95.0, 20.0);
    let b = h.app.weave.title_at(&p0, 4) + vec2(10.0, 5.0);
    h.drag(a, b);
    let picked = h.app.weave.picked();
    assert!(picked.contains(&2) && picked.contains(&4), "{picked:?}");
    assert!(!picked.contains(&6));
    // nothing moved
    assert_eq!(patch(&h, ch), p0);
    // the right click on the empty canvas: its own menu
    // between the keys and the oscillator
    let empty = h.app.weave.title_at(&p0, 2) - vec2(108.0, 0.0);
    h.right_click_at(empty);
    assert!(h.has("Paste Ctrl+V") && h.has("Duplicate Ctrl+D") && h.has("Add module here"));
    h.click("Duplicate Ctrl+D");
    assert_eq!(patch(&h, ch).nodes.len(), p0.nodes.len() + picked.len());
    // a module added from it lands where the menu was opened
    h.right_click_at(empty);
    h.click("Add module here");
    h.type_text("reverb");
    h.click("Reverb");
    let p = patch(&h, ch);
    let r = p
        .nodes
        .iter()
        .find(|n| n.kind == Kind::Reverb)
        .expect("added");
    let w = h.app.weave.world_at(empty);
    assert!((r.pos[0] - w.x).abs() < 2.0 && (r.pos[1] - w.y).abs() < 2.0);
}

#[test]
fn a_module_dropped_on_a_cable_goes_into_it_and_alt_pulls_it_out() {
    let (mut h, ch) = weave();
    h.menu(&["Add module"]);
    h.click(las_dsp::modular::CATEGORIES[Kind::Shaper.category()]);
    h.click("Wavefolder");
    h.run(2);
    let fold = h.app.weave.picked()[0];
    let p0 = patch(&h, ch);
    let ci = p0
        .cables
        .iter()
        .position(|c| c.from == 4 && c.to == 5)
        .unwrap();
    let target = h.app.weave.cable_at(&p0, ci);
    // drag by the title so that the pointer ends on the cable's middle
    let grab = h.app.weave.title_at(&p0, fold);
    h.press_at(grab);
    h.move_to(target, 10);
    assert!(h.app.weave.insert_target().is_some(), "the cable is lit");
    h.release();
    let p1 = patch(&h, ch);
    assert!(p1.cables.iter().any(|c| c.from == 4 && c.to == fold));
    assert!(p1.cables.iter().any(|c| c.from == fold && c.to == 5));
    assert!(!p1.cables.iter().any(|c| c.from == 4 && c.to == 5));
    // Alt and a drag pulls it out and joins the filter to the amp again
    let grab = h.app.weave.title_at(&p1, fold);
    h.set_modifiers(Modifiers::ALT);
    h.press_at(grab);
    h.move_to(grab + vec2(0.0, 200.0), 6);
    h.release();
    h.set_modifiers(Modifiers::NONE);
    let p2 = patch(&h, ch);
    assert!(p2.cables.iter().all(|c| c.from != fold && c.to != fold));
    assert!(p2
        .cables
        .iter()
        .any(|c| c.from == 4 && c.to == 5 && c.input == "in"));
}

#[test]
fn the_math_module_shows_its_operation_in_its_title() {
    let (mut h, ch) = weave();
    h.menu(&["Add module"]);
    h.type_text("math");
    h.click("Math");
    h.run(2);
    let id = h.app.weave.picked()[0];
    assert!(!h.find_prefix("Math: Add").is_empty());
    let op = Kind::Math.spec().inputs.len() + Kind::Math.input_index("op").unwrap()
        - Kind::Math.spec().inputs.len();
    let at = h.app.weave.row_at(&patch(&h, ch), id, op);
    h.click_at(at);
    h.click_at(h.get("multiply").rect.center());
    h.run(2);
    assert_eq!(patch(&h, ch).node(id).unwrap().param("op"), 2.0);
    assert!(!h.find_prefix("Math: Multiply").is_empty());
}

#[test]
fn every_module_kind_is_added_turned_wired_and_removed_from_the_ui() {
    let (mut h, ch) = weave();
    h.app.weave.set_zoom(0.8);
    h.app.weave.set_pan(vec2(10.0, 10.0));
    h.run(2);
    // only the envelope (3) and the output (6) are kept
    let p0 = patch(&h, ch);
    h.click_at(h.app.weave.title_at(&p0, 1));
    for id in [2, 4, 5] {
        h.click_with(
            h.app.weave.title_at(&p0, id),
            eframe::egui::PointerButton::Primary,
            Modifiers::COMMAND,
        );
    }
    h.key(eframe::egui::Key::Delete);
    assert_eq!(patch(&h, ch).nodes.len(), 2);
    for k in Kind::ALL {
        let add = h.get("Add module").rect;
        h.menu(&["Add module"]);
        h.type_text(crate::weave_ui::kind_name(k));
        // the entry in the menu, under its button (a channel may have the same name)
        let entry = h
            .find_all(crate::weave_ui::kind_name(k))
            .into_iter()
            .filter(|n| n.rect.top() >= add.bottom())
            .filter(|n| n.role == eframe::egui::accesskit::Role::Button)
            .min_by(|a, b| {
                let d = |n: &super::Node| n.rect.left_top().distance(add.left_bottom());
                d(a).total_cmp(&d(b))
            })
            .unwrap_or_else(|| panic!("{k:?} is not in the menu"));
        assert!(
            entry.rect.top() - add.bottom() < 60.0,
            "{k:?}: found by the search"
        );
        h.click_at(entry.rect.center());
        h.run(2);
        let id = *h.app.weave.picked().last().expect("added and picked");
        assert_eq!(patch(&h, ch).node(id).unwrap().kind, k);
        // moved to the left of the canvas, clear of the others
        let c = h.app.weave.canvas();
        let grab = h.app.weave.title_at(&patch(&h, ch), id);
        let spot = pos2(c.left() + 300.0, c.top() + 30.0);
        h.drag(grab, spot);
        let s = k.spec();
        // number controls turn with a drag (the first two of each: every control's
        // reach is checked by the engine's own audit)
        let numbers = s
            .params
            .iter()
            .enumerate()
            .filter(|(_, ps)| k.choices(ps.name).is_none())
            .take(2);
        for (pi, ps) in numbers {
            let at = h.app.weave.row_at(&patch(&h, ch), id, s.inputs.len() + pi);
            assert!(c.contains(at), "{k:?}.{} is on the canvas", ps.name);
            let v0 = patch(&h, ch).node(id).unwrap().param(ps.name);
            let dy = if ps.normalize(v0) > 0.5 { 60.0 } else { -60.0 };
            h.drag(at, at + vec2(0.0, dy));
            let v1 = patch(&h, ch).node(id).unwrap().param(ps.name);
            assert_ne!(v0, v1, "{k:?}.{}", ps.name);
        }
        // the envelope into every plain input, and every output into the output
        let before = patch(&h, ch).cables.len();
        for i in 0..s.inputs.len() {
            let p = patch(&h, ch);
            h.drag(h.app.weave.out_at(&p, 3, 0), h.app.weave.in_at(&p, id, i));
        }
        for o in 0..s.outputs.len() {
            let p = patch(&h, ch);
            h.drag(h.app.weave.out_at(&p, id, o), h.app.weave.in_at(&p, 6, 1));
        }
        let p = patch(&h, ch);
        let ins = p
            .cables
            .iter()
            .filter(|c| c.to == id && c.from == 3)
            .count();
        let outs = p
            .cables
            .iter()
            .filter(|c| c.from == id && c.to == 6)
            .count();
        assert_eq!((ins, outs), (s.inputs.len(), s.outputs.len()), "{k:?}");
        // taken out again by their dots (a drag that ends on nothing)
        for i in 0..s.inputs.len() {
            let p = patch(&h, ch);
            let dot = h.app.weave.in_at(&p, id, i);
            h.drag(dot, pos2(c.left() + 15.0, dot.y));
        }
        let p = patch(&h, ch);
        assert!(!p.cables.iter().any(|c| c.to == id), "{k:?}: unplugged");
        // and the module goes with Delete, with its cables
        h.click_at(h.app.weave.title_at(&p, id));
        h.key(eframe::egui::Key::Delete);
        let p = patch(&h, ch);
        assert!(p.node(id).is_none(), "{k:?}");
        assert_eq!(p.cables.len(), before, "{k:?}");
    }
}
