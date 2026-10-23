// SPDX-License-Identifier: GPL-3.0-or-later
//! Second bug hunt: flows an FL Studio user tries, each driven as a user does.

use super::Harness;

#[test]
fn the_toolbar_fits_a_small_screen_without_overlap() {
    for (w, hgt) in [
        (1280.0, 720.0),
        (1280.0, 800.0),
        (1366.0, 768.0),
        (2560.0, 1440.0),
    ] {
        let mut h = Harness::demo_sized(w, hgt);
        let w = h.ctx.content_rect().right();
        let master = h.find_prefix("Master: ")[0].clone();
        // what does not fit scrolls sideways; scrolled to its end, the last button
        // is clear of the knob
        let at = h.get("Play (Space)").rect.center();
        h.set_modifiers(eframe::egui::Modifiers::SHIFT);
        h.wheel_at(at, -2000.0);
        h.set_modifiers(eframe::egui::Modifiers::NONE);
        h.run(3);
        let n = h.get("Export");
        assert!(
            n.rect.right() <= master.rect.left(),
            "{w}x{hgt}: Export at {:?} is under the master knob at {:?}",
            n.rect,
            master.rect
        );
        assert!(master.rect.right() <= w + 0.5, "{w}: master off screen");
        // the toolbar stays a toolbar: it does not take the height of the window
        let play = h.get("Play (Space)").rect;
        assert!(
            play.top() < 120.0,
            "{w}x{hgt}: the toolbar runs down to {play:?}"
        );
    }
}

/// Render from the export window and wait for it to finish; the message it shows.
fn render(h: &mut Harness) -> Result<String, String> {
    let b = h
        .find_all("Render")
        .into_iter()
        .find(|n| n.role == eframe::egui::accesskit::Role::Button)
        .expect("a Render button");
    h.click_at(b.rect.center());
    for _ in 0..3000 {
        h.run(1);
        if let Some(m) = h.app.shell.export.message.clone() {
            return m;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the render never finished");
}

fn sound_of(path: &std::path::Path) -> las_engine::factory::Audio {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    las_engine::library::decode_audio(&bytes)
        .unwrap_or_else(|| panic!("{} does not decode", path.display()))
}

#[test]
fn export_every_format_and_stems_from_the_export_window() {
    // the new project's beat (a demo song takes minutes in a debug build)
    let mut h = Harness::new();
    let dir = std::env::temp_dir().join(format!("las-hunt2-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    h.app.shell.open.export = true;
    h.run(3);
    // a short render: the chosen pattern
    h.click("Chosen pattern");
    for (i, (name, kind, _)) in crate::shell::FORMATS.iter().enumerate() {
        h.app.shell.export.format = i;
        let path = dir.join(format!("mix{i}.{kind}"));
        h.app.shell.export.path = path.to_string_lossy().into_owned();
        h.run(2);
        let m = render(&mut h);
        assert!(m.is_ok(), "{name}: {m:?}");
        let a = sound_of(&path);
        let peak = a.channels[0].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(
            a.channels[0].len() > 44_100 && peak > 0.01,
            "{name}: silent or empty"
        );
    }
    h.click("Each mixer track as its own file");
    h.app.shell.export.format = 0;
    h.app.shell.export.path = dir
        .join("stems")
        .join("x.wav")
        .to_string_lossy()
        .into_owned();
    h.run(2);
    let m = render(&mut h);
    assert!(m.is_ok(), "stems: {m:?}");
    let files: Vec<_> = std::fs::read_dir(dir.join("stems"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(files.len() > 2, "stems: {files:?}");
    for f in &files {
        sound_of(f);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The knobs of the open synth window: widgets whose label is "Name: value\n...".
fn knobs(h: &Harness) -> Vec<super::Node> {
    h.nodes
        .iter()
        .filter(|n| n.label.contains(": ") && n.label.contains("double-click to reset"))
        .filter(|n| !n.label.starts_with("Master:"))
        .filter(|n| h.ctx.content_rect().contains_rect(n.rect))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .fold(Vec::new(), |mut v, n| {
            v.push(n);
            v.sort_by(|a: &super::Node, b| a.rect.top().total_cmp(&b.rect.top()));
            v
        })
}

#[test]
fn a_synth_panel_knob_is_heard_in_the_playing_song_while_it_is_dragged() {
    for id in [
        "polaris", "vortex", "monolith", "zenith", "triosc", "facet", "mist",
    ] {
        let mut h = Harness::demo();
        // only the synth window: nothing in front of its knobs
        h.app.shell.open.rack = false;
        h.app.shell.open.playlist = false;
        h.app.add_channel(id);
        let ch = h.project().channels.len() - 1;
        h.app.open_channel(ch);
        h.run(30);
        h.app.project.has_player = true;
        h.app.project.stale = false;
        // a tall face puts the first knobs below the fold of its scroll area
        let w = h
            .nodes
            .iter()
            .find(|n| n.role == eframe::egui::accesskit::Role::Window)
            .unwrap()
            .rect;
        h.wheel_at(w.center(), -120.0);
        let all = knobs(&h);
        let mut heard = 0;
        for k in &all {
            if heard >= 2 {
                break;
            }
            let before = h.project().channels[ch].state.clone();
            h.app.live_patches.clear();
            h.press_at(k.rect.center());
            h.move_to(k.rect.center() - eframe::egui::vec2(0.0, 60.0), 6);
            let after = &h.project().channels[ch].state;
            // a rack effect's knob builds the player again (another kind of edit)
            if *after == before || after.get("fx") != before.get("fx") {
                h.release();
                continue;
            }
            assert!(
                h.app.live_patches.contains(&ch),
                "{id}: dragging {:?} sent nothing to the player",
                k.label
            );
            h.release();
            assert!(
                !h.app.project.stale,
                "{id}: the player is built again after {:?}",
                k.label
            );
            heard += 1;
        }
        assert!(heard > 0, "{id}: no knob changed the patch");
    }
}

fn playing(h: &Harness) -> bool {
    h.app.audio.as_ref().is_some_and(|a| a.meters.playing())
}

/// Frames until the output is heard (or a few seconds of them pass).
fn hears(h: &mut Harness) -> bool {
    for _ in 0..150 {
        h.run(1);
        if h.heard > 1e-3 {
            return true;
        }
    }
    false
}

#[test]
fn space_plays_and_stops_the_pattern_and_the_song() {
    use eframe::egui::Key;
    let mut h = Harness::new().with_audio();
    h.settle();
    // pattern mode: the pattern loops
    h.click("PAT");
    h.key(Key::Space);
    h.settle();
    assert!(playing(&h), "Space did not start the pattern");
    assert!(h.app.project.looping, "PAT plays the pattern");
    assert!(hears(&mut h), "the pattern is not heard");
    h.key(Key::Space);
    h.run(3);
    assert!(!playing(&h), "Space did not stop");
    // song mode
    h.click("SONG");
    h.key(Key::Space);
    h.settle();
    assert!(playing(&h) && !h.app.project.looping, "SONG plays the song");
    h.click("Stop");
    h.run(3);
    assert!(!playing(&h), "Stop did not stop");
}

#[test]
fn a_click_on_swing_or_tempo_without_moving_is_not_an_edit() {
    let mut h = Harness::demo();
    h.app.project.dirty = false;
    let undo = h.app.project.undo.len();
    let swing = h.get("Swing: 0%").rect.center();
    h.click_at(swing);
    let bpm = h.find_prefix("Drag up and down to change the tempo")[0]
        .rect
        .center();
    h.click_at(bpm);
    assert_eq!(h.app.project.undo.len(), undo, "an empty undo step");
    assert!(!h.app.project.dirty, "the project counts as changed");
    // a real drag is one step
    h.drag(bpm, bpm - eframe::egui::vec2(0.0, 40.0));
    assert_eq!(h.app.project.undo.len(), undo + 1);
    assert!(h.project().bpm > 128.0);
}

#[test]
fn instrument_window_knobs_are_heard_live_in_a_playing_song() {
    // Zenith's face has knobs of its effects rack
    for id in ["nebula", "lumapad", "sampler", "zenith"] {
        let mut h = Harness::demo();
        h.app.shell.open.rack = false;
        h.app.shell.open.playlist = false;
        h.app.add_channel(id);
        let ch = h.project().channels.len() - 1;
        h.app.open_channel(ch);
        h.run(30);
        h.app.project.has_player = true;
        h.app.project.stale = false;
        let all = knobs(&h);
        let (mut live, mut rebuilt) = (Vec::new(), Vec::new());
        for k in all.iter().take(if id == "zenith" { 60 } else { 14 }) {
            let before = h.project().channels[ch].state.clone();
            h.app.live_params.clear();
            h.app.live_patches.clear();
            h.press_at(k.rect.center());
            h.move_to(k.rect.center() - eframe::egui::vec2(0.0, 30.0), 4);
            let changed = h.project().channels[ch].state != before;
            let sent = !h.app.live_params.is_empty() || !h.app.live_patches.is_empty();
            h.release();
            let name = k.label.lines().next().unwrap_or("").to_string();
            if changed && sent && !h.app.project.stale {
                live.push(name);
            } else if changed {
                rebuilt.push(name);
            }
            h.app.project.stale = false;
        }
        eprintln!("{id}: live {live:?}\n{id}: rebuilt {rebuilt:?}");
        assert!(
            !live.is_empty(),
            "{id}: no knob is heard live; rebuilt {rebuilt:?}"
        );
        if id == "zenith" {
            assert!(
                live.iter().any(|n| n.starts_with("Size")),
                "{id}: the rack's reverb size"
            );
        }
    }
}

#[test]
fn the_wheel_over_the_top_window_does_not_reach_the_one_under_it() {
    use eframe::egui::{accesskit::Role, Modifiers};
    let mut h = Harness::demo();
    h.app.shell.open.playlist = true;
    h.app.shell.open.roll = true;
    h.run(10);
    let win = |h: &Harness, name: &str| {
        h.nodes
            .iter()
            .find(|n| n.role == Role::Window && n.label.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} window"))
            .rect
    };
    let (roll, pl) = (win(&h, "Piano Roll"), win(&h, "Playlist"));
    let both = roll.intersect(pl);
    assert!(
        both.width() > 100.0 && both.height() > 100.0,
        "{roll:?} {pl:?}"
    );
    let at = both.center();
    let (rz, pz) = (h.app.project.roll.zoom, h.app.project.playlist.zoom);
    h.set_modifiers(Modifiers::COMMAND);
    h.wheel_at(at, 120.0);
    h.set_modifiers(Modifiers::NONE);
    assert_ne!(
        h.app.project.roll.zoom, rz,
        "Ctrl+wheel does not zoom the roll on top"
    );
    assert_eq!(
        h.app.project.playlist.zoom, pz,
        "the playlist under it zoomed too"
    );
    // Alt+wheel zooms the roll's keys, not the playlist's tracks
    let (rr, pr) = (h.app.project.roll.row, h.app.project.playlist.row);
    h.set_modifiers(Modifiers::ALT);
    h.wheel_at(at, 120.0);
    h.set_modifiers(Modifiers::NONE);
    assert_ne!(
        h.app.project.roll.row, rr,
        "Alt+wheel does not zoom the roll's keys"
    );
    assert_eq!(
        h.app.project.playlist.row, pr,
        "the playlist's tracks changed"
    );
}

#[test]
fn an_autosave_of_unsaved_work_is_offered_after_a_crash_and_opens() {
    let mut h = Harness::demo();
    h.app.settings.autosave_minutes = 1;
    h.app.project.modify(true, |p| p.bpm = 99.0);
    h.app.autosaved.last = std::time::Instant::now() - std::time::Duration::from_secs(120);
    h.run(2);
    let file = crate::settings::autosave_file("").unwrap();
    assert!(file.exists(), "no autosave at {}", file.display());
    // the app ends without quitting (a crash) and starts again
    let mut h = h.restart();
    h.run(3);
    assert!(h.has("Restore unsaved work"), "the autosave is not offered");
    h.click("Open");
    h.run(3);
    assert_eq!(h.project().bpm, 99.0);
    assert!(
        h.app.project.dirty && !h.app.project.has_file(),
        "it opens as unsaved work"
    );
    assert!(!h.has("Restore unsaved work"));
}

#[test]
fn a_clean_quit_leaves_nothing_to_restore() {
    let mut h = Harness::demo();
    h.app.settings.autosave_minutes = 1;
    h.app.project.modify(true, |p| p.bpm = 99.0);
    h.app.autosaved.last = std::time::Instant::now() - std::time::Duration::from_secs(120);
    h.run(2);
    let file = crate::settings::autosave_file("").unwrap();
    assert!(file.exists());
    // File > Quit asks about the changes; the user lets them go
    h.app.guard(crate::menus::Action::Quit);
    h.run(2);
    assert!(h.has("Unsaved changes"));
    h.app.project.dirty = false;
    h.app.guard(crate::menus::Action::Quit);
    h.run(1);
    assert!(
        !file.exists(),
        "the autosave is still there after a clean quit"
    );
}

#[test]
fn markers_on_the_ruler_set_a_loop_and_alt_arrows_jump_to_them() {
    use eframe::egui::{pos2, Key, Modifiers};
    let mut h = Harness::demo();
    let o = &mut h.app.shell.open;
    o.rack = false;
    o.mixer = false;
    o.roll = false;
    h.run(3);
    let a = h.app.project.playlist.area.expect("the playlist was drawn");
    let bar = 384.0_f32;
    let ruler = |t: f32| pos2(a.left + t * a.zoom, a.top - 8.0);
    let before = h.project().markers.iter().flatten().count();
    for t in [bar * 2.0, bar * 4.0] {
        h.right_click_at(ruler(t));
        h.click("Add marker here");
    }
    let marks: Vec<f64> = h
        .project()
        .markers
        .iter()
        .flatten()
        .map(|m| m.tick)
        .collect();
    assert_eq!(marks.len(), before + 2, "markers {marks:?}");
    h.right_click_at(ruler(bar * 3.0));
    h.click("Loop between these markers");
    assert_eq!(h.app.project.playlist.song_loop, Some((768.0, 1536.0)));
    assert!(h.app.project.playlist.loop_on);
    h.click("Loop");
    assert!(
        !h.app.project.playlist.loop_on,
        "the Loop button does not switch it off"
    );
    // Alt+Right jumps to the next marker
    h.app.project.start_tick = 0.0;
    h.hover(a_center(&h));
    h.key_with(Key::ArrowRight, Modifiers::ALT);
    let first = marks
        .iter()
        .copied()
        .filter(|&t| t > 0.0)
        .fold(f64::MAX, f64::min);
    assert_eq!(h.app.project.start_tick, first, "Alt+Right");
    // undo takes the markers back
    for _ in 0..2 {
        h.key_with(Key::Z, Modifiers::COMMAND);
    }
    assert_eq!(h.project().markers.iter().flatten().count(), before);
}

fn a_center(h: &Harness) -> eframe::egui::Pos2 {
    let a = h.app.project.playlist.area.expect("the playlist was drawn");
    eframe::egui::pos2(a.left + 200.0, a.top + 40.0)
}

#[test]
fn notes_typed_while_recording_go_into_the_pattern() {
    use eframe::egui::Key;
    let mut h = Harness::new().with_audio();
    h.settle();
    h.app.settings.record_audio = false;
    h.app.settings.record_notes = true;
    h.app.settings.count_in = false;
    let notes = |h: &Harness| -> usize {
        h.project().patterns[h.app.project.pattern]
            .notes
            .values()
            .map(Vec::len)
            .sum()
    };
    let before = notes(&h);
    h.click("PAT");
    h.app.shell.record = true;
    h.key(Key::Space);
    h.settle();
    assert!(playing(&h));
    h.run(10);
    h.key(Key::A);
    h.run(10);
    h.key(Key::S);
    h.run(10);
    h.key(Key::Space);
    h.run(3);
    assert_eq!(notes(&h), before + 2, "the typed notes were not recorded");
    // one Ctrl+Z takes the whole take back
    h.key_with(Key::Z, eframe::egui::Modifiers::COMMAND);
    assert_eq!(notes(&h), before);
}

#[test]
fn every_theme_applies_and_is_kept() {
    let mut h = Harness::demo();
    let mut seen = Vec::new();
    for t in crate::theme::themes() {
        h.app.choose_theme(&t.id);
        h.run(3);
        let p = crate::theme::pal();
        seen.push((t.id.clone(), p.accent, p.panel2));
    }
    let mut looks: Vec<_> = seen.iter().map(|s| (s.1, s.2)).collect();
    looks.sort_by_key(|c| (c.0.to_array(), c.1.to_array()));
    looks.dedup();
    assert!(looks.len() > 1, "every theme looks the same: {seen:?}");
    let last = seen.last().unwrap().0.clone();
    let h = h.restart();
    assert_eq!(h.app.shell.theme, last, "the theme is not kept");
}

#[test]
fn slicing_a_sound_makes_a_slicer_channel_and_a_pattern_that_plays_it() {
    let mut h = Harness::new();
    h.app
        .add_factory_channel("factory:kick-punch", "Kick Punch");
    let ch = h.project().channels.len() - 1;
    let id = h.project().channels[ch].id.clone();
    let (chans, pats) = (h.project().channels.len(), h.project().patterns.len());
    crate::sampleedit::request(crate::sampleedit::Request::Slice(id));
    h.run(6);
    assert_eq!(h.project().channels.len(), chans + 1, "no slicer channel");
    assert_eq!(
        h.project().patterns.len(),
        pats + 1,
        "no pattern for the slices"
    );
    let slicer = h.project().channels.last().unwrap().clone();
    assert_eq!(slicer.plugin_id, las_project::slicer::SLICER_ID);
    let pat = h.project().patterns.last().unwrap().clone();
    assert!(
        pat.notes.get(&slicer.id).is_some_and(|n| !n.is_empty()),
        "the pattern does not play the slices"
    );
    // its window opens, and the pattern is heard
    h.app.open_channel(chans);
    h.run(4);
    let l = h.app.project.loaded.as_ref().unwrap();
    let opts = las_engine::RenderOptions {
        sample_rate: 44_100.0,
        mode: las_engine::Mode::Pattern {
            id: Some(pat.id.clone()),
            loops: 1,
        },
        ..Default::default()
    };
    let r = las_engine::render_mix_with(&l.project, &l.samples, &opts).unwrap();
    let peak = r.left.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "the slices are silent");
}

#[test]
fn a_samplers_channel_settings_tabs_each_change_the_sound() {
    let mut h = Harness::new();
    h.app.shell.open.rack = false;
    h.app.shell.open.playlist = false;
    h.app
        .add_factory_channel("factory:kick-punch", "Kick Punch");
    let ch = h.project().channels.len() - 1;
    h.app.open_channel(ch);
    h.run(6);
    for tab in ["Sample", "Envelope", "Misc", "Arp"] {
        h.click(tab);
        h.run(2);
        let before = h.project().channels[ch].state.clone();
        let all = knobs(&h);
        let mut changed = false;
        for k in all.iter().take(4) {
            h.drag(
                k.rect.center(),
                k.rect.center() - eframe::egui::vec2(0.0, 40.0),
            );
            if h.project().channels[ch].state != before {
                changed = true;
                break;
            }
        }
        if tab == "Arp" && !changed {
            // the Arp tab has buttons rather than knobs
            h.click("Up");
            changed = h.project().channels[ch].state != before;
        }
        assert!(
            changed,
            "{tab}: nothing on it changes the channel ({} knobs)",
            all.len()
        );
    }
}

#[test]
fn a_browser_sample_dragged_to_the_rack_or_playlist_is_added_there() {
    let mut h = Harness::demo();
    h.app.shell.open.roll = false;
    h.app.shell.open.mixer = false;
    h.run(3);
    let chans = h.project().channels.len();
    let item = h.get("Kick Hard").rect.center();
    let rack = h.get("Channel Rack").rect;
    h.drag(item, rack.center_bottom() - eframe::egui::vec2(0.0, 40.0));
    h.run(3);
    assert_eq!(
        h.project().channels.len(),
        chans + 1,
        "the rack did not take the sample"
    );
    // to the playlist: an audio clip
    h.app.shell.open.rack = false;
    h.run(3);
    let clips = h.project().playlist.clips.len();
    let a = h.app.project.playlist.area.expect("the playlist was drawn");
    let to = eframe::egui::pos2(a.left + 384.0 * 6.0 * a.zoom, a.top + a.row * 7.5);
    let item = h.get("Kick Hard").rect.center();
    h.drag(item, to);
    h.run(3);
    assert_eq!(
        h.project().playlist.clips.len(),
        clips + 1,
        "the playlist did not take the sample"
    );
}

#[test]
fn keyboard_shortcuts_do_what_the_menus_say() {
    use eframe::egui::{Key, Modifiers};
    let mut h = Harness::demo();
    let pats = h.project().patterns.len();
    h.key(Key::F4);
    assert_eq!(h.project().patterns.len(), pats + 1, "F4 makes a pattern");
    h.key_with(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.project().patterns.len(), pats, "Ctrl+Z");
    h.key_with(Key::Y, Modifiers::COMMAND);
    assert_eq!(h.project().patterns.len(), pats + 1, "Ctrl+Y");
    h.key_with(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    h.key_with(Key::Z, Modifiers::COMMAND);
    h.key_with(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.project().patterns.len(), pats + 1, "Ctrl+Shift+Z");
    // Ctrl+Z while a name is typed takes back the typing only
    h.app.project.rename = Some((crate::project::RenameTarget::Channel(0), "Kick".into()));
    h.run(3);
    h.type_text("xyz");
    h.key_with(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        h.project().patterns.len(),
        pats + 1,
        "Ctrl+Z in a text box undid the project"
    );
    h.key(Key::Escape);
    h.run(2);
    // Ctrl+N with changes asks first
    h.key_with(Key::N, Modifiers::COMMAND);
    assert!(h.has("Unsaved changes"), "Ctrl+N would drop the changes");
}
