// SPDX-License-Identifier: GPL-3.0-or-later
//! Weave's window driven as a user does: voices, controls turned while a note plays,
//! cables, presets, undo.

use super::Harness;
use crate::weave_ui::patch_of;
use eframe::egui::{vec2, Modifiers};
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
