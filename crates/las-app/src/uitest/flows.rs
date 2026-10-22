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

/// The visible part of the playlist's clip area.
fn playlist_view(h: &Harness) -> eframe::egui::Rect {
    let win = h.get("Playlist - Arrangement").rect;
    let bars: Vec<_> = h
        .nodes
        .iter()
        .filter(|n| n.role == eframe::egui::accesskit::Role::ScrollBar && win.contains_rect(n.rect))
        .cloned()
        .collect();
    let v = bars
        .iter()
        .max_by(|a, b| a.rect.height().total_cmp(&b.rect.height()))
        .expect("a scroll bar")
        .rect;
    let hz = bars
        .iter()
        .max_by(|a, b| a.rect.width().total_cmp(&b.rect.width()))
        .expect("a scroll bar")
        .rect;
    eframe::egui::Rect::from_min_max(
        eframe::egui::pos2(hz.left(), v.top()),
        eframe::egui::pos2(v.left(), hz.top()),
    )
}

/// Where a playlist clip is drawn: its middle, and its right edge.
fn clip_at(h: &Harness, id: &str) -> (eframe::egui::Pos2, f32) {
    let a = h.app.project.playlist.area.expect("the playlist was drawn");
    let c = h
        .project()
        .playlist
        .clips
        .iter()
        .find(|c| c.id == id)
        .expect("the clip");
    let x = a.left + (c.start + c.length / 2.0) as f32 * a.zoom;
    let y = a.top + (c.track as f32 + 0.5) * a.row;
    (
        eframe::egui::pos2(x, y),
        a.left + (c.start + c.length) as f32 * a.zoom,
    )
}

fn playlist_harness() -> Harness {
    let mut h = Harness::demo();
    let o = &mut h.app.shell.open;
    o.rack = false;
    o.mixer = false;
    o.roll = false;
    h.run(3);
    h
}

#[test]
fn playlist_place_move_resize_duplicate_delete() {
    use eframe::egui::{vec2, Key, Modifiers};
    let mut h = playlist_harness();
    let v = playlist_view(&h);
    println!("view {v:?}");
    let n0 = h.project().playlist.clips.len();
    // the lowest track in view, after the song
    let mut at = None;
    for dx in [0.75f32, 0.85, 0.65, 0.95] {
        let p = eframe::egui::pos2(v.left() + v.width() * dx, v.bottom() - 60.0);
        h.click_at(p);
        if h.project().playlist.clips.len() == n0 + 1 {
            at = Some(p);
            break;
        }
    }
    at.expect("a click on an empty spot places a clip");
    let placed = h.project().playlist.clips.last().unwrap().clone();
    // drag it right
    let (mid, _) = clip_at(&h, &placed.id);
    h.drag(mid, mid + vec2(100.0, 0.0));
    let moved = h
        .project()
        .playlist
        .clips
        .iter()
        .find(|c| c.id == placed.id)
        .unwrap()
        .clone();
    assert!(
        moved.start > placed.start,
        "a drag moves the clip: {} -> {}",
        placed.start,
        moved.start
    );
    assert_eq!(
        h.project().playlist.clips.len(),
        n0 + 1,
        "a drag adds nothing"
    );
    // Ctrl+B duplicates it after itself
    let (mid, right) = clip_at(&h, &placed.id);
    h.hover(mid);
    h.key_with(Key::B, Modifiers::COMMAND);
    assert_eq!(
        h.project().playlist.clips.len(),
        n0 + 2,
        "Ctrl+B duplicates"
    );
    h.key_with(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.project().playlist.clips.len(), n0 + 1, "undo");
    // a right click deletes it
    h.right_click_at(mid);
    assert_eq!(
        h.project().playlist.clips.len(),
        n0,
        "a right click deletes the clip"
    );
    h.key_with(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        h.project().playlist.clips.len(),
        n0 + 1,
        "undo brings it back"
    );
    // drag its right edge further right: it gets longer
    let len = moved.length;
    h.drag(
        eframe::egui::pos2(right - 2.0, mid.y),
        eframe::egui::pos2(right + 120.0, mid.y),
    );
    let c = h
        .project()
        .playlist
        .clips
        .iter()
        .find(|c| c.id == placed.id)
        .unwrap()
        .clone();
    assert!(
        c.length > len,
        "dragging the edge resizes: {len} -> {}",
        c.length
    );
    assert_eq!(c.start, moved.start, "resizing keeps the start");
}

#[test]
fn a_new_or_opened_project_does_not_keep_the_old_ones_loop_or_selection() {
    use eframe::egui::{Key, Modifiers};
    let mut h = playlist_harness();
    let v = playlist_view(&h);
    // a loop dragged along the ruler (just above the clips)
    let a = h.app.project.playlist.area.unwrap();
    let y = a.top - 8.0;
    h.drag(
        eframe::egui::pos2(v.left() + 50.0, y),
        eframe::egui::pos2(v.left() + 300.0, y),
    );
    assert!(
        h.app.project.playlist.song_loop.is_some(),
        "a drag along the ruler sets a loop"
    );
    h.hover(v.center());
    h.key_with(Key::A, Modifiers::COMMAND);
    assert!(
        !h.app.project.playlist.selected.is_empty(),
        "Ctrl+A selects the clips"
    );
    h.app.project.dirty = false;
    h.menu(&["File", "New Ctrl+N"]);
    assert_eq!(h.project().name, "Untitled", "a new project");
    assert_eq!(
        h.app.project.playlist.song_loop, None,
        "the old song's loop is gone"
    );
    assert!(!h.app.project.playlist.loop_on);
    assert!(
        h.app.project.playlist.selected.is_empty(),
        "nothing selected"
    );
    assert!(h.app.project.roll.selected.is_empty());
}

#[test]
fn a_saved_project_reopens_as_it_was_with_its_loop() {
    use eframe::egui::{Key, Modifiers};
    let mut h = playlist_harness();
    let v = playlist_view(&h);
    let a = h.app.project.playlist.area.unwrap();
    let y = a.top - 8.0;
    h.drag(
        eframe::egui::pos2(v.left() + 50.0, y),
        eframe::egui::pos2(v.left() + 300.0, y),
    );
    let lp = h.app.project.playlist.song_loop.expect("a loop");
    // a preset picked on a synth channel, a step and a pattern
    h.app.add_channel("zenith");
    let ch = h.project().channels.len() - 1;
    h.app.open_channel(ch);
    h.run(3);
    h.click(">");
    h.menu(&["Patterns", "New pattern F4"]);
    let dir = std::env::temp_dir().join(format!("las-uitest-save-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("song.las").to_string_lossy().into_owned();
    h.app.project.save_to(&path).unwrap();
    // a change saved with Ctrl+S
    h.app.add_channel("vortex");
    h.run(2);
    h.key_with(Key::S, Modifiers::COMMAND);
    assert!(!h.app.project.dirty, "Ctrl+S saved");
    let saved = serde_json::to_value(h.project()).unwrap();
    h.menu(&["File", "New Ctrl+N"]);
    assert_eq!(h.app.project.playlist.song_loop, None);
    h.app.open_project(&path);
    h.run(3);
    let mut back = serde_json::to_value(h.project()).unwrap();
    let mut want = saved.clone();
    for x in [&mut back, &mut want] {
        x["meta"]["updatedAt"] = serde_json::json!(0);
        if let Some(o) = x.as_object_mut() {
            o.remove("songLoop");
        }
    }
    let d = json_diff(&want, &back);
    assert!(
        d.is_empty(),
        "the project comes back as it was saved: {d:#?}"
    );
    assert_eq!(h.app.project.playlist.song_loop, Some(lp), "and its loop");
    assert!(h.app.project.playlist.loop_on);
    h.app.open_channel(ch);
    h.run(3);
    let k = crate::app::index(h.app.inst);
    let name = h.app.entries[k][h.app.presets[k]].name.clone();
    assert_eq!(preset_title(&h), name);
    assert_ne!(name, "Init", "the preset picked is shown again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Where two JSON values differ, as paths with both sides.
fn json_diff(a: &serde_json::Value, b: &serde_json::Value) -> Vec<String> {
    fn go(a: &serde_json::Value, b: &serde_json::Value, at: String, out: &mut Vec<String>) {
        use serde_json::Value as V;
        match (a, b) {
            (V::Object(x), V::Object(y)) => {
                for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                    let n = V::Null;
                    go(
                        x.get(k).unwrap_or(&n),
                        y.get(k).unwrap_or(&n),
                        format!("{at}/{k}"),
                        out,
                    );
                }
            }
            (V::Array(x), V::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    go(p, q, format!("{at}/{i}"), out);
                }
            }
            (V::Number(x), V::Number(y))
                if (x.as_f64().unwrap_or(0.0) - y.as_f64().unwrap_or(0.0)).abs() < 1e-9 => {}
            _ if a == b => {}
            _ => out.push(format!("{at}: {} != {}", short(a), short(b))),
        }
    }
    fn short(v: &serde_json::Value) -> String {
        let s = v.to_string();
        if s.len() > 120 {
            format!("{}...", &s[..120])
        } else {
            s
        }
    }
    let mut out = Vec::new();
    go(a, b, String::new(), &mut out);
    out
}

#[test]
fn the_export_file_follows_the_open_project_until_the_user_picks_one() {
    let mut h = Harness::demo();
    h.menu(&["File", "Export... Ctrl+R"]);
    assert!(h.app.shell.open.export);
    assert!(
        h.app.shell.export.path.ends_with("Lumen Nights (demo).wav"),
        "named after the song: {}",
        h.app.shell.export.path
    );
    let dir = std::env::temp_dir().join(format!("las-uitest-export-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let a = dir.join("first.las").to_string_lossy().into_owned();
    let b = dir.join("second.las").to_string_lossy().into_owned();
    h.app.project.save_to(&a).unwrap();
    h.run(2);
    assert_eq!(
        h.app.shell.export.path,
        dir.join("first.wav").to_string_lossy()
    );
    h.app.project.save_to(&b).unwrap();
    h.app.open_project(&a);
    h.run(2);
    assert_eq!(
        h.app.shell.export.path,
        dir.join("first.wav").to_string_lossy()
    );
    h.app.open_project(&b);
    h.run(2);
    assert_eq!(
        h.app.shell.export.path,
        dir.join("second.wav").to_string_lossy(),
        "another project exports to its own file, not over the last one's"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_window_fits_on_small_and_large_screens() {
    for (w, hgt) in [(1366.0, 768.0), (1600.0, 1000.0), (1920.0, 1080.0)] {
        let mut h = Harness::demo_sized(w, hgt);
        let check = |h: &Harness, what: &str| {
            let screen = h.ctx.content_rect();
            for n in h
                .nodes
                .iter()
                .filter(|n| n.role == eframe::egui::accesskit::Role::Window)
            {
                assert!(
                    n.rect.bottom() <= screen.bottom() + 1.0
                        && n.rect.top() >= screen.top() - 1.0
                        && n.rect.right() <= screen.right() + 1.0
                        && n.rect.left() >= -1.0,
                    "{w}x{hgt} {what}: the window {:?} at {:?} is off the screen {screen:?}",
                    n.label,
                    n.rect
                );
            }
        };
        check(&h, "start");
        type Open = fn(&mut crate::shell::Windows);
        let all: [(&str, Open); 9] = [
            ("settings", |o| o.settings = true),
            ("keys", |o| o.keys = true),
            ("about", |o| o.about = true),
            ("history", |o| o.history = true),
            ("export", |o| o.export = true),
            ("analyser", |o| o.analyser = true),
            ("mixer", |o| o.mixer = true),
            ("roll", |o| o.roll = true),
            ("playlist", |o| o.playlist = true),
        ];
        for (what, open) in all {
            open(&mut h.app.shell.open);
            h.run(6);
            check(&h, what);
        }
        h.app.wf.palette.show("");
        h.run(4);
        check(&h, "quick search");
        for (id, _, _) in las_project::create::INSTRUMENTS {
            h.app.add_channel(id);
            let ch = h.project().channels.len() - 1;
            h.app.open_channel(ch);
            h.run(6);
            check(&h, id);
        }
    }
}

#[test]
fn windows_shrink_to_fit_when_the_interface_is_made_bigger() {
    let mut h = Harness::demo_sized(1920.0, 1080.0);
    h.app.shell.open.mixer = true;
    h.app.shell.open.roll = true;
    h.run(4);
    h.app.settings.ui_scale = 150;
    h.run(10);
    let screen = h.ctx.content_rect();
    for n in h
        .nodes
        .iter()
        .filter(|n| n.role == eframe::egui::accesskit::Role::Window)
    {
        assert!(
            screen.expand(1.0).contains_rect(n.rect),
            "the window {:?} at {:?} is off the screen {screen:?}",
            n.label,
            n.rect
        );
    }
}

/// Open the quick search with Ctrl+K, type `q` and press Enter.
fn quick(h: &mut Harness, q: &str) {
    use eframe::egui::{Key, Modifiers};
    h.key_with(Key::K, Modifiers::COMMAND);
    assert!(h.app.wf.palette.open, "Ctrl+K opens the quick search");
    h.type_text(q);
    h.key(Key::Enter);
    assert!(!h.app.wf.palette.open, "Enter runs and closes");
}

#[test]
fn quick_search_runs_each_kind_of_result() {
    let mut h = Harness::demo();
    h.app.shell.open.mixer = false;
    // an instrument: a new channel, opened
    let n = h.project().channels.len();
    quick(&mut h, "Add Vortex");
    assert_eq!(h.project().channels.len(), n + 1);
    assert_eq!(h.project().channels[n].plugin_id, "vortex");
    assert_eq!(h.app.bound.map(|b| b.channel()), Some(n));
    // an effect: on the selected insert, and the mixer opens
    let ins = h.app.project.mixer_state.selected;
    let before = h.project().mixer[ins].slots.iter().flatten().count();
    quick(&mut h, "Reverb");
    assert_eq!(
        h.project().mixer[ins].slots.iter().flatten().count(),
        before + 1
    );
    assert!(h.app.shell.open.mixer);
    // a window
    h.app.shell.open.roll = false;
    quick(&mut h, "Piano roll");
    assert!(h.app.shell.open.roll, "the piano roll opens");
    // a factory sample: a new channel
    let n = h.project().channels.len();
    quick(&mut h, "Kick Deep");
    assert_eq!(h.project().channels.len(), n + 1);
    // an action
    let p = h.project().patterns.len();
    quick(&mut h, "New pattern");
    assert_eq!(h.project().patterns.len(), p + 1);
}

#[test]
fn a_preset_from_quick_search_never_turns_a_channel_into_another_synth() {
    let mut h = Harness::demo();
    h.app.add_channel("polaris");
    let pol = h.project().channels.len() - 1;
    h.app.open_channel(pol);
    h.run(3);
    let before = h.project().channels[pol].state.clone();
    // a Mist preset, while a Polaris channel is open
    let k = crate::app::index(crate::audio::Instrument::Mist);
    let name = h.app.entries[k][1].name.clone();
    quick(&mut h, &name);
    h.run(3);
    let after = &h.project().channels[pol];
    assert_eq!(after.plugin_id, "polaris");
    assert_eq!(after.state, before, "the Polaris channel keeps its sound");
    // the preset went to a Mist channel, which is open
    let b = h.app.bound.expect("a channel is open").channel();
    assert_eq!(
        h.project().channels[b].plugin_id,
        "mist",
        "a Mist channel has the preset"
    );
    assert_eq!(preset_title(&h), name);
}

#[test]
fn mixer_add_an_effect_from_an_empty_slot_on_a_small_screen() {
    let mut h = Harness::demo_sized(1366.0, 768.0);
    let o = &mut h.app.shell.open;
    o.rack = false;
    o.playlist = false;
    h.key(eframe::egui::Key::F9);
    h.run(3);
    let ins = h.app.project.mixer_state.selected;
    let before = h.project().mixer[ins].slots.iter().flatten().count();
    let slot = h
        .find_prefix("+ Slot")
        .into_iter()
        .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .expect("an empty slot");
    h.click_at(slot.rect.center());
    let screen = h.ctx.content_rect();
    let names: Vec<String> = las_dsp::fx::registry::descriptors()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    // the menu stays on the screen and scrolls: an effect in view is picked
    let buttons: Vec<_> = names
        .iter()
        .filter_map(|n| {
            h.find_all(n)
                .into_iter()
                .find(|b| b.role == eframe::egui::accesskit::Role::Button)
        })
        .collect();
    let left = buttons
        .iter()
        .map(|b| b.rect.left())
        .fold(f32::MAX, f32::min);
    let right = buttons
        .iter()
        .map(|b| b.rect.right())
        .fold(f32::MIN, f32::max);
    let bar = h
        .nodes
        .iter()
        .filter(|n| n.role == eframe::egui::accesskit::Role::ScrollBar && n.rect.left() > left)
        .min_by(|a, b| {
            (a.rect.left() - right)
                .abs()
                .total_cmp(&(b.rect.left() - right).abs())
        })
        .expect("the effects menu scrolls")
        .rect;
    assert!(
        screen.contains_rect(bar),
        "the menu is on the screen: {bar:?}"
    );
    let pick = buttons
        .iter()
        .find(|b| b.rect.top() > bar.top() + 4.0 && b.rect.bottom() < bar.bottom() - 4.0)
        .expect("an effect in view");
    h.click_at(pick.rect.center());
    assert_eq!(
        h.project().mixer[ins].slots.iter().flatten().count(),
        before + 1,
        "the effect is added"
    );
}

/// Right click `node` and choose "Create automation clip"; the clip made.
fn automate(h: &mut Harness, node: &super::Node) -> las_project::AutomationClip {
    let n = h.project().automation.len();
    h.right_click_at(node.rect.center());
    h.click("Create automation clip");
    h.run(2);
    assert_eq!(
        h.project().automation.len(),
        n + 1,
        "a clip for {:?}",
        node.label
    );
    h.project().automation.last().unwrap().clone()
}

#[test]
fn right_click_create_automation_clip_on_tempo_channel_and_mixer_controls() {
    let mut h = Harness::demo();
    let tempo = h.get("Drag up and down to change the tempo. Right click: Create automation clip (the tempo then follows it in the song)");
    let c = automate(&mut h, &tempo);
    assert!(
        c.target.to_string().contains("tempo") || c.target.to_string().contains("bpm"),
        "{:?}",
        c.target
    );
    // the first channel's level knob in the rack
    let lvl = h
        .find_prefix("Level:")
        .into_iter()
        .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .unwrap();
    let c = automate(&mut h, &lvl);
    assert!(!c.points.is_empty());
    // a mixer fader
    assert!(
        h.app.project.open_auto.is_some(),
        "the new clip opens to be edited"
    );
    h.app.project.open_auto = None;
    h.app.shell.open.rack = false;
    h.app.shell.open.playlist = false;
    h.key(eframe::egui::Key::F9);
    h.run(3);
    let fader = h
        .nodes
        .iter()
        .filter(|n| {
            n.label
                .contains(" dB\nDrag, Shift for fine, double-click for 0 dB")
        })
        .min_by(|a, b| a.rect.left().total_cmp(&b.rect.left()))
        .cloned()
        .expect("a fader");
    automate(&mut h, &fader);
    // undo takes the last clip away
    let n = h.project().automation.len();
    h.key_with(eframe::egui::Key::Z, eframe::egui::Modifiers::COMMAND);
    assert_eq!(h.project().automation.len(), n - 1);
}

#[test]
fn mixer_faders_and_knobs_keep_their_clicks_inside_a_strip() {
    let mut h = Harness::demo();
    h.app.shell.open.rack = false;
    h.app.shell.open.playlist = false;
    h.key(eframe::egui::Key::F9);
    h.run(3);
    let mut f: Vec<_> = h
        .nodes
        .iter()
        .filter(|n| n.label.contains(" dB\nDrag, Shift for fine"))
        .cloned()
        .collect();
    f.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    // the second strip (insert 1), dragged down then double clicked back to 0 dB
    let fader = f[1].clone();
    let v0 = h.project().mixer[1].volume;
    h.drag(
        fader.rect.center(),
        fader.rect.center() + eframe::egui::vec2(0.0, 40.0),
    );
    let v1 = h.project().mixer[1].volume;
    assert!(v1 < v0, "a drag down lowers the fader: {v0} -> {v1}");
    h.double_click_at(fader.rect.center());
    assert!(
        (h.project().mixer[1].volume - 1.0).abs() < 1e-9,
        "a double click is 0 dB"
    );
    // an EQ knob turned up
    let eq = h
        .find_prefix("EQ Low (100 Hz)")
        .into_iter()
        .min_by(|a, b| a.rect.left().total_cmp(&b.rect.left()))
        .unwrap();
    let ins = h
        .nodes
        .iter()
        .filter(|n| n.label.starts_with("EQ Low"))
        .filter(|n| n.rect.left() < eq.rect.left())
        .count();
    h.drag(
        eq.rect.center(),
        eq.rect.center() - eframe::egui::vec2(0.0, 40.0),
    );
    assert!(
        h.project().mixer[ins].eq.low > 0.0,
        "the low EQ goes up: {:?}",
        h.project().mixer[ins].eq
    );
    // a click on the strip's background selects it, a right click there gives its menu
    let pill = h.find_all("Click to select, right-click for render, sidechain and tuner");
    let mut pill: Vec<_> = pill.into_iter().collect();
    pill.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    h.click_at(pill[2].rect.center());
    assert_eq!(h.app.project.mixer_state.selected, 2);
}

#[test]
fn sample_editor_reverse_trim_undo_and_the_edit_is_saved() {
    let mut h = Harness::demo();
    let o = &mut h.app.shell.open;
    o.rack = false;
    o.playlist = false;
    o.mixer = false;
    h.app
        .add_factory_channel("factory:kick-punch", "Kick Punch");
    let ch = h.project().channels.len() - 1;
    let id = h.project().channels[ch].id.clone();
    crate::sampleedit::request(crate::sampleedit::Request::Edit(id.clone()));
    h.run(4);
    assert!(h.app.sample_editor.is_some(), "the editor opens");
    let sample = |h: &Harness| h.project().channels[ch].sample_id.clone();
    let s0 = sample(&h);
    h.click("Reverse");
    let s1 = sample(&h);
    assert_ne!(s0, s1, "Reverse makes a new sound for the channel");
    h.click("Undo");
    assert_eq!(sample(&h), s0, "the editor's Undo puts the sound back");
    h.click("Redo");
    assert_eq!(sample(&h), s1);
    // select the first half of the waveform and trim to it
    let win = h
        .find_prefix("Sample editor:")
        .into_iter()
        .find(|n| n.role == eframe::egui::accesskit::Role::Window)
        .unwrap()
        .rect;
    let wave = h.find_prefix("Drag across the waveform to select")[0].rect;
    assert!(win.contains_rect(wave));
    // below the marker strip at its top
    let wave = eframe::egui::Rect::from_min_max(wave.min + eframe::egui::vec2(0.0, 40.0), wave.max);
    let frames = |h: &Harness| h.app.sample_editor.as_ref().unwrap().audio.channels[0].len();
    let f0 = frames(&h);
    h.drag(
        wave.left_center() + eframe::egui::vec2(10.0, 0.0),
        wave.center(),
    );
    assert!(
        h.app.sample_editor.as_ref().unwrap().sel.is_some(),
        "a drag selects"
    );
    h.click("Trim to selection");
    let f1 = frames(&h);
    assert!(f1 < f0 * 3 / 4, "trimmed: {f0} -> {f1}");
    // the edited sound is in the saved project
    let dir = std::env::temp_dir().join(format!("las-uitest-se-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("edited.las").to_string_lossy().into_owned();
    h.app.project.save_to(&path).unwrap();
    let want = sample(&h);
    h.app.open_project(&path);
    h.run(3);
    assert_eq!(sample(&h), want);
    let l = h.app.project.loaded.as_ref().unwrap();
    assert!(
        l.samples.contains_key(want.as_deref().unwrap()),
        "the edited sound was saved with the project"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_theme_and_windows_are_kept_after_a_restart() {
    let mut h = Harness::demo();
    h.menu(&["Options", "Settings..."]);
    h.run(3);
    assert!(!h.app.settings.count_in);
    let cb = h.get("Count in one bar before recording");
    h.click_at(cb.rect.center());
    assert!(h.app.settings.count_in, "the box is ticked");
    h.app.shell.open.settings = false;
    // a theme from the menu
    let themes = crate::theme::themes();
    let other = themes.iter().find(|t| t.id != h.app.shell.theme).unwrap();
    let (id, name) = (other.id.to_string(), other.name.to_string());
    h.menu(&["Options", "Theme", &name]);
    assert_eq!(h.app.shell.theme, id);
    // the windows
    h.key(eframe::egui::Key::F9);
    h.key(eframe::egui::Key::F6);
    h.run(2);
    let (mixer, rack) = (h.app.shell.open.mixer, h.app.shell.open.rack);
    let h = h.restart();
    assert!(h.app.settings.count_in, "the setting is kept");
    assert_eq!(h.app.shell.theme, id, "the theme is kept");
    assert_eq!(
        (h.app.shell.open.mixer, h.app.shell.open.rack),
        (mixer, rack),
        "the windows are kept"
    );
}

#[test]
fn undo_takes_every_kind_of_edit_back_and_redo_does_them_again() {
    use eframe::egui::{Key, Modifiers};
    let mut h = Harness::demo();
    let start = serde_json::to_value(h.project()).unwrap();
    let mut states = vec![start.clone()];
    let note = |h: &Harness, states: &mut Vec<serde_json::Value>, what: &str| {
        let now = serde_json::to_value(h.project()).unwrap();
        assert_ne!(Some(&now), states.last(), "{what} changed the project");
        states.push(now);
    };
    // a step
    let r = step_row(&h, 1);
    h.click_at(eframe::egui::pos2(r.left() + 8.0, r.center().y));
    note(&h, &mut states, "a step");
    // a channel, a pattern
    h.menu(&["Add", "Zenith"]);
    note(&h, &mut states, "Add > Zenith");
    h.key(Key::F4);
    note(&h, &mut states, "F4");
    // the tempo, by dragging it
    let tempo = h.get("Drag up and down to change the tempo. Right click: Create automation clip (the tempo then follows it in the song)");
    h.drag(
        tempo.rect.center(),
        tempo.rect.center() - eframe::egui::vec2(0.0, 30.0),
    );
    note(&h, &mut states, "the tempo");
    // a rename
    h.menu(&["Patterns", "Rename pattern..."]);
    h.key_with(Key::A, Modifiers::COMMAND);
    h.type_text("Verse");
    h.key(Key::Enter);
    note(&h, &mut states, "a rename");
    assert_eq!(h.project().patterns[h.app.project.pattern].name, "Verse");
    let n = states.len() - 1;
    for i in (0..n).rev() {
        h.key_with(Key::Z, Modifiers::COMMAND);
        let now = serde_json::to_value(h.project()).unwrap();
        let d = json_diff(&states[i], &now);
        assert!(d.is_empty(), "undo {} of {n}: {d:#?}", n - i);
    }
    for (i, want) in states.iter().enumerate().skip(1) {
        h.key_with(Key::Y, Modifiers::COMMAND);
        let now = serde_json::to_value(h.project()).unwrap();
        let d = json_diff(want, &now);
        assert!(d.is_empty(), "redo {i}: {d:#?}");
    }
}

#[test]
fn enter_confirms_the_save_as_template_box_and_the_template_is_offered() {
    let mut h = Harness::demo();
    quick(&mut h, "Save as template");
    assert!(h.app.wf.template_name.is_some(), "the name box opens");
    h.type_text("My Beat");
    h.key(eframe::egui::Key::Enter);
    assert!(h.app.wf.template_name.is_none(), "Enter saves and closes");
    let dir = crate::workflow::templates::dir("templates").unwrap();
    let saved = crate::workflow::templates::user_templates(&dir);
    assert!(
        saved.iter().any(|(n, _)| n == "My Beat"),
        "saved: {saved:?}"
    );
    // and a new project made from it has the demo's channels
    let n = h.project().channels.len();
    h.app.project.dirty = false;
    quick(&mut h, "New from template: My Beat");
    assert_eq!(h.project().channels.len(), n);
    assert!(
        !h.app.project.has_file(),
        "a template opens as a new, unsaved project"
    );
}

/// Buttons a masher leaves alone: those that open the system's file dialogs (which
/// would wait for a person) or leave the app.
fn opens_a_dialog(label: &str) -> bool {
    let l = label.to_lowercase();
    l.contains("...")
        || l.contains('\u{2026}')
        || [
            "load",
            "save",
            "choose",
            "browse",
            "import",
            "open",
            "export",
            "quit",
            "replace",
            "sample file",
            "folder",
            "close window",
            "hide",
        ]
        .iter()
        .any(|w| l.contains(w))
}

/// Click every button of the window titled `title` once, checking the app never
/// panics and the project stays valid.
fn mash(h: &mut Harness, title: &str) -> usize {
    let Some(win) = h
        .find_all(title)
        .into_iter()
        .find(|n| n.role == eframe::egui::accesskit::Role::Window)
    else {
        return 0;
    };
    let labels: Vec<String> = h
        .nodes
        .iter()
        .filter(|n| {
            n.role == eframe::egui::accesskit::Role::Button
                && !n.disabled
                && !n.label.is_empty()
                && win.rect.contains_rect(n.rect)
                && !opens_a_dialog(&n.label)
        })
        .map(|n| n.label.clone())
        .collect();
    let mut clicked = 0;
    for l in labels {
        let Some(win) = h
            .find_all(title)
            .into_iter()
            .find(|n| n.role == eframe::egui::accesskit::Role::Window)
        else {
            break;
        };
        let Some(b) = h
            .find_all(&l)
            .into_iter()
            .find(|n| win.rect.contains_rect(n.rect) && !n.disabled)
        else {
            continue;
        };
        h.click_at(b.rect.center());
        // close whatever popup the button opened
        h.key(eframe::egui::Key::Escape);
        clicked += 1;
        if h.app.project.loaded.is_some() {
            let bytes = las_project::encode(h.project(), &Default::default(), false);
            assert!(
                bytes.is_ok(),
                "after {l:?} in {title}: the project still saves"
            );
        }
    }
    clicked
}

#[test]
fn every_button_of_every_instrument_window_can_be_clicked() {
    for (id, name, _) in las_project::create::INSTRUMENTS {
        let mut h = Harness::demo();
        let o = &mut h.app.shell.open;
        o.rack = false;
        o.playlist = false;
        o.mixer = false;
        h.app.add_channel(id);
        let ch = h.project().channels.len() - 1;
        h.app.open_channel(ch);
        h.run(4);
        let title = h.project().channels[ch].name.clone();
        let n = mash(&mut h, &title);
        println!("{name}: {n} buttons");
    }
}

#[test]
fn every_button_of_the_main_windows_can_be_clicked() {
    let mut h = Harness::demo();
    for (title, open) in [
        (
            "Piano Roll",
            (|o: &mut crate::shell::Windows| o.roll = true) as fn(&mut crate::shell::Windows),
        ),
        ("Mixer", |o| o.mixer = true),
        ("Channel Rack", |o| o.rack = true),
        ("Playlist - Arrangement", |o| o.playlist = true),
        ("Keyboard shortcuts", |o| o.keys = true),
        ("History", |o| o.history = true),
        ("Analyser", |o| o.analyser = true),
    ] {
        let o = &mut h.app.shell.open;
        *o = crate::shell::Windows { ..*o };
        o.rack = false;
        o.playlist = false;
        o.mixer = false;
        o.roll = false;
        open(o);
        h.run(4);
        let n = mash(&mut h, title);
        println!("{title}: {n} buttons");
    }
}
