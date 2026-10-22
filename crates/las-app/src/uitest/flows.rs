// SPDX-License-Identifier: GPL-3.0-or-later
//! The flows an FL Studio user goes through, each driven with the mouse and keys.

use super::Harness;

#[test]
fn the_app_starts_and_draws_its_menus() {
    let h = Harness::demo();
    for m in ["File", "Edit", "Add", "Patterns", "View", "Options", "Help"] {
        assert!(h.has(m), "menu {m}");
    }
}

const CHANNEL: &str = "Double click: instrument. Right click: more.";

/// The channel buttons of the rack, top to bottom.
fn rows(h: &Harness) -> Vec<super::Node> {
    let mut v = h.find_all(CHANNEL);
    v.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
    v
}

/// The steps of the channel shown in row `i` of the rack.
fn step_row(h: &Harness, i: usize) -> eframe::egui::Rect {
    let r = &rows(h)[i];
    h.nodes
        .iter()
        .find(|n| {
            n.label.is_empty()
                && n.rect.left() > r.rect.right()
                && (n.rect.center().y - r.rect.center().y).abs() < 3.0
                && n.rect.width() > 100.0
        })
        .map(|n| n.rect)
        .expect("the steps of the row")
}

#[test]
fn every_instrument_of_the_add_menu_is_added_and_opens_on_double_click() {
    let mut h = Harness::demo();
    for (id, name, _) in las_project::create::INSTRUMENTS {
        let before = h.project().channels.len();
        h.menu(&["Add", name]);
        assert_eq!(h.project().channels.len(), before + 1, "Add > {name}");
        let last = h.project().channels.last().unwrap();
        assert_eq!(last.plugin_id, *id);
        assert_eq!(h.app.project.channel, before, "the new channel is selected");
        // the rack scrolls; open it through the app as a double click does
        let r = rows(&h);
        if let Some(n) = r.get(before) {
            let limit = h.get("+ Add channel").rect.top();
            if h.app.shell.open.rack && n.rect.bottom() < limit {
                h.double_click_at(n.rect.center());
            } else {
                h.app.open_channel(before);
            }
        } else {
            h.app.open_channel(before);
        }
        h.run(4);
        if *id == "audioclip" {
            continue;
        }
        assert!(
            h.app.shell.open.synth,
            "{name}: the instrument window opens"
        );
        assert_eq!(
            h.app.bound.map(|b| b.channel()),
            Some(before),
            "{name} is bound"
        );
        // its window is titled after the channel
        let title = h.project().channels[before].name.clone();
        assert!(
            h.nodes.iter().any(
                |n| n.role == eframe::egui::accesskit::Role::Window && n.label.contains(&title)
            ),
            "{name}: a window named {title}"
        );
    }
}

#[test]
fn the_rack_add_channel_button_adds_each_instrument() {
    let mut h = Harness::demo();
    for (id, name, _) in las_project::create::INSTRUMENTS {
        let before = h.project().channels.len();
        let add = h.get("+ Add channel");
        let rack = h.get("Channel Rack").rect;
        assert!(
            rack.bottom() <= 800.0,
            "the rack stays on the screen: {rack:?}"
        );
        assert!(
            add.rect.bottom() < 800.0,
            "+ Add channel is on the screen: {:?}",
            add.rect
        );
        h.click("+ Add channel");
        h.click(name);
        h.run(2);
        assert_eq!(
            h.project().channels.len(),
            before + 1,
            "+ Add channel > {name}"
        );
        assert_eq!(h.project().channels.last().unwrap().plugin_id, *id);
    }
    // the rack scrolls its rows and keeps its size from frame to frame
    let rack = h.get("Channel Rack").rect;
    h.run(10);
    assert_eq!(h.get("Channel Rack").rect, rack, "the rack does not creep");
}

#[test]
fn view_menu_and_toolbar_open_and_close_every_window() {
    let mut h = Harness::demo();
    for (label, get) in [
        (
            "Playlist F5",
            (|h: &Harness| h.app.shell.open.playlist) as fn(&Harness) -> bool,
        ),
        ("Channel rack F6", |h| h.app.shell.open.rack),
        ("Piano roll F7", |h| h.app.shell.open.roll),
        ("Mixer F9", |h| h.app.shell.open.mixer),
        ("Synth", |h| h.app.shell.open.synth),
        ("Browser F8", |h| h.app.shell.browser),
    ] {
        let was = get(&h);
        h.menu(&["View", label]);
        assert_eq!(get(&h), !was, "View > {label} toggles");
        h.menu(&["View", label]);
        assert_eq!(get(&h), was, "View > {label} toggles back");
    }
    for (label, get) in [
        (
            "Mixer",
            (|h: &Harness| h.app.shell.open.mixer) as fn(&Harness) -> bool,
        ),
        ("Analyser", |h| h.app.shell.open.analyser),
        ("Browser", |h| h.app.shell.browser),
    ] {
        let was = get(&h);
        let n = h
            .find_all(label)
            .into_iter()
            .find(|n| {
                n.role == eframe::egui::accesskit::Role::Button
                    && n.rect.top() > 40.0
                    && n.rect.top() < 100.0
            })
            .expect(label);
        h.click_at(n.rect.center());
        h.run(2);
        assert_eq!(get(&h), !was, "toolbar {label}");
    }
    for (path, get) in [
        (
            &["Options", "Settings..."][..],
            (|h: &Harness| h.app.shell.open.settings) as fn(&Harness) -> bool,
        ),
        (&["Help", "Keyboard shortcuts F1"][..], |h| {
            h.app.shell.open.keys
        }),
        (&["Help", "About"][..], |h| h.app.shell.open.about),
        (&["Edit", "History..."][..], |h| h.app.shell.open.history),
        (&["File", "Export... Ctrl+R"][..], |h| {
            h.app.shell.open.export
        }),
    ] {
        h.menu(path);
        assert!(get(&h), "{path:?} opens");
    }
}

#[test]
fn function_keys_open_the_windows() {
    use eframe::egui::Key;
    let mut h = Harness::demo();
    h.app.shell.open.roll = false;
    h.app.shell.open.mixer = false;
    h.run(1);
    h.key(Key::F7);
    assert!(h.app.shell.open.roll, "F7");
    h.key(Key::F9);
    assert!(h.app.shell.open.mixer, "F9");
    h.key(Key::F1);
    assert!(h.app.shell.open.keys, "F1");
}

#[test]
fn a_step_click_toggles_a_step_and_undo_takes_it_back() {
    let mut h = Harness::demo();
    let row = step_row(&h, 0);
    let pat = h.app.project.pattern;
    let count =
        |h: &Harness| -> usize { h.project().patterns[pat].notes.values().map(Vec::len).sum() };
    let before = count(&h);
    // a step near the end of the first row
    let at = eframe::egui::pos2(row.right() - 8.0, row.center().y);
    h.click_at(at);
    let after = count(&h);
    assert_ne!(after, before, "a click on a step toggles it");
    h.key_with(eframe::egui::Key::Z, eframe::egui::Modifiers::COMMAND);
    assert_eq!(count(&h), before, "Ctrl+Z takes the step back");
    h.key_with(eframe::egui::Key::Y, eframe::egui::Modifiers::COMMAND);
    assert_eq!(count(&h), after, "Ctrl+Y puts it back");
}

#[test]
fn patterns_menu_new_clone_delete() {
    let mut h = Harness::demo();
    let n = h.project().patterns.len();
    h.menu(&["Patterns", "New pattern F4"]);
    assert_eq!(h.project().patterns.len(), n + 1);
    assert_eq!(h.app.project.pattern, n);
    h.menu(&["Patterns", "Clone pattern"]);
    assert_eq!(h.project().patterns.len(), n + 2);
    assert_eq!(h.app.project.pattern, n + 1);
    h.menu(&["Patterns", "Delete pattern"]);
    assert_eq!(h.project().patterns.len(), n + 1);
    h.menu(&["Patterns", "Rename pattern..."]);
    assert!(h.app.project.rename.is_some());
}

#[test]
fn typing_a_name_does_not_play_notes_or_change_the_octave() {
    let mut h = Harness::demo();
    h.menu(&["Patterns", "Rename pattern..."]);
    let octave = h.app.octave;
    // the rename box has the focus: these are letters, not notes
    for k in [
        eframe::egui::Key::Z,
        eframe::egui::Key::Z,
        eframe::egui::Key::A,
    ] {
        h.key(k);
    }
    h.type_text("zza");
    assert_eq!(
        h.app.octave, octave,
        "Z and X typed into a name leave the octave"
    );
    assert!(h.app.typed.is_empty(), "no notes held");
    let name = h
        .app
        .project
        .rename
        .as_ref()
        .map(|r| r.1.clone())
        .unwrap_or_default();
    assert!(
        name.ends_with("zza"),
        "the letters went into the name: {name}"
    );
}

/// The preset menu button of the synth window ("Name  v").
fn preset_title(h: &Harness) -> String {
    let n = h
        .nodes
        .iter()
        .find(|n| n.label.ends_with("  v") && n.role == eframe::egui::accesskit::Role::Button)
        .expect("the preset menu button");
    n.label.trim_end_matches("  v").to_string()
}

#[test]
fn synth_presets_step_with_the_arrows_and_stay_with_their_channel() {
    for id in ["polaris", "monolith", "zenith", "triosc", "facet", "mist"] {
        let mut h = Harness::demo();
        h.app.shell.open.mixer = false;
        h.app.add_channel(id);
        let a = h.project().channels.len() - 1;
        h.app.add_channel(id);
        let b = a + 1;
        h.app.open_channel(a);
        h.run(3);
        let first = preset_title(&h);
        h.click(">");
        h.click(">");
        let picked = preset_title(&h);
        assert_ne!(first, picked, "{id}: > moves to another preset");
        let k = crate::app::index(h.app.inst);
        let want = h.app.entries[k]
            .iter()
            .find(|e| e.name == picked)
            .unwrap()
            .patch
            .clone();
        let saved = h.project().channels[a].state["patch"].clone();
        assert_eq!(
            saved,
            las_dsp::patch::merge_patch(&saved, &want),
            "{id}: the channel has the preset's sound"
        );
        // another channel of the same synth shows its own preset, not the last one picked
        h.app.open_channel(b);
        h.run(3);
        assert_eq!(
            preset_title(&h),
            first,
            "{id}: the second channel is still on its own preset"
        );
        h.app.open_channel(a);
        h.run(3);
        assert_eq!(
            preset_title(&h),
            picked,
            "{id}: the first channel shows the preset it has"
        );
        h.click("<");
        assert_ne!(preset_title(&h), picked, "{id}: < steps back");
    }
}

/// The visible part of the piano roll's grid (right of its keyboard).
fn roll_grid(h: &Harness) -> eframe::egui::Rect {
    let win = h.get("Piano Roll").rect;
    let grid = h
        .nodes
        .iter()
        .filter(|n| n.label.is_empty() && n.rect.height() > 1000.0 && win.intersects(n.rect))
        .max_by(|a, b| a.rect.height().total_cmp(&b.rect.height()))
        .expect("the roll's grid")
        .rect;
    // the roll's vertical scroll bar spans the part of the grid in view
    let bar = h
        .nodes
        .iter()
        .filter(|n| n.role == eframe::egui::accesskit::Role::ScrollBar && win.contains_rect(n.rect))
        .max_by(|a, b| a.rect.height().total_cmp(&b.rect.height()))
        .expect("the roll's scroll bar")
        .rect;
    eframe::egui::Rect::from_min_max(
        eframe::egui::pos2(grid.left() + 90.0, bar.top() + 10.0),
        eframe::egui::pos2(bar.left() - 10.0, bar.bottom() - 10.0),
    )
}

fn roll_notes(h: &Harness) -> Vec<las_project::Note> {
    let p = h.project();
    let id = &p.channels[h.app.project.channel].id;
    p.patterns[h.app.project.pattern]
        .notes
        .get(id)
        .cloned()
        .unwrap_or_default()
}

fn roll_harness() -> Harness {
    let mut h = Harness::demo();
    let o = &mut h.app.shell.open;
    o.playlist = false;
    o.rack = false;
    o.mixer = false;
    h.app.add_channel("polaris");
    h.run(2);
    h.key(eframe::egui::Key::F7);
    h.run(3);
    assert!(h.app.shell.open.roll);
    h
}

#[test]
fn piano_roll_draw_select_copy_paste_delete_undo() {
    use eframe::egui::{Key, Modifiers};
    let mut h = roll_harness();
    let g = roll_grid(&h);
    let a = g.left_top() + eframe::egui::vec2(40.0, 40.0);
    let b = g.left_top() + eframe::egui::vec2(200.0, 80.0);
    h.click_at(a);
    assert_eq!(roll_notes(&h).len(), 1, "a click draws a note");
    h.click_at(b);
    assert_eq!(roll_notes(&h).len(), 2, "another click draws another");
    // a click on a note does not add one
    h.click_at(a + eframe::egui::vec2(3.0, 0.0));
    assert_eq!(roll_notes(&h).len(), 2, "a click on a note leaves it");
    h.key_with(Key::A, Modifiers::COMMAND);
    assert_eq!(h.app.project.roll.selected.len(), 2, "Ctrl+A selects all");
    h.key_with(Key::C, Modifiers::COMMAND);
    h.key_with(Key::V, Modifiers::COMMAND);
    assert_eq!(roll_notes(&h).len(), 4, "Ctrl+V pastes the copy");
    h.key(Key::Delete);
    assert_eq!(
        roll_notes(&h).len(),
        2,
        "Delete removes the pasted (selected) notes"
    );
    // Ctrl+B with nothing selected duplicates all, as in FL
    h.key_with(Key::B, Modifiers::COMMAND);
    assert_eq!(roll_notes(&h).len(), 4, "Ctrl+B duplicates");
    h.key_with(Key::Z, Modifiers::COMMAND);
    assert_eq!(roll_notes(&h).len(), 2, "Ctrl+Z undoes the duplicate");
    h.key_with(Key::D, Modifiers::COMMAND);
    assert!(h.app.project.roll.selected.is_empty(), "Ctrl+D deselects");
    // a right click deletes the note under it
    h.right_click_at(b + eframe::egui::vec2(3.0, 0.0));
    assert_eq!(roll_notes(&h).len(), 1, "a right click deletes");
    // drag a note to the right moves it
    let before = roll_notes(&h)[0].pos;
    h.drag(
        a + eframe::egui::vec2(3.0, 0.0),
        a + eframe::egui::vec2(120.0, 0.0),
    );
    let n = roll_notes(&h);
    assert_eq!(n.len(), 1);
    assert!(
        n[0].pos > before,
        "a drag moves the note later: {} -> {}",
        before,
        n[0].pos
    );
}

#[test]
fn piano_roll_paint_and_delete_tools() {
    let mut h = roll_harness();
    let g = roll_grid(&h);
    h.click("Paint");
    let a = g.left_top() + eframe::egui::vec2(20.0, 60.0);
    h.drag(a, a + eframe::egui::vec2(300.0, 0.0));
    let painted = roll_notes(&h).len();
    assert!(painted >= 3, "painting across draws notes: {painted}");
    h.click("Delete");
    h.drag(a, a + eframe::egui::vec2(300.0, 0.0));
    assert_eq!(
        roll_notes(&h).len(),
        0,
        "the delete tool erases what it passes"
    );
    h.key_with(eframe::egui::Key::Z, eframe::egui::Modifiers::COMMAND);
    assert_eq!(roll_notes(&h).len(), painted, "undo brings them back");
}

#[test]
fn piano_roll_right_drag_erases_every_note_it_passes() {
    let mut h = roll_harness();
    let g = roll_grid(&h);
    h.click("Paint");
    let a = g.left_top() + eframe::egui::vec2(20.0, 60.0);
    h.drag(a, a + eframe::egui::vec2(300.0, 0.0));
    assert!(roll_notes(&h).len() >= 3);
    h.drag_with(
        a,
        a + eframe::egui::vec2(300.0, 0.0),
        eframe::egui::PointerButton::Secondary,
        eframe::egui::Modifiers::NONE,
    );
    assert_eq!(
        roll_notes(&h).len(),
        0,
        "a right drag erases what it passes"
    );
}
