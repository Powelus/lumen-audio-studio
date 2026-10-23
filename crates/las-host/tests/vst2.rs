// SPDX-License-Identifier: GPL-3.0-or-later
//! The host against the test VST2 plugin (`las-testvst2`), a real VST2 library: found
//! by a scan, run in this process and in the plugin host program, its parameters, its
//! state chunk, and its window.

mod common;

use common::testvst2;
use las_host::vst2::Vst2;
use las_host::{describe_any, scan, Format, NoteMsg, Plugin, PluginInfo};

const SR: f64 = 48_000.0;
// what `las-testvst2` has (it is loaded as a library, not linked)
const GAIN: u32 = 0;

fn info() -> PluginInfo {
    describe_any(&testvst2()).pop().expect("the test plugin")
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_scan_finds_the_plugin_in_its_vendor_folder() {
    let root = testvst2()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let found = scan(&[root]);
    let t = found
        .iter()
        .find(|i| i.format == Format::Vst2)
        .expect("found");
    assert_eq!(t.name, "Lumen Test VST2");
    assert_eq!(t.vendor, "Lumen Audio Studio");
    assert!(t.is_instrument() && t.is_effect());
    assert_eq!(t.id, "vst2:4c547632:las-test2");
    assert_eq!(t.format.tag(), "VST2");
}

#[test]
fn as_an_effect_it_scales_the_input_by_its_gain() {
    let mut p = Vst2::load(&info(), SR).unwrap();
    assert!(p.takes_audio());
    let l: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let (mut ol, mut or) = (vec![0.0; 1000], vec![0.0; 1000]);
    // the default (0.5) is unity gain; more frames than one block
    p.process(Some((&l, &l)), &[], &mut ol, &mut or);
    for (a, b) in l.iter().zip(&ol) {
        assert!((a - b).abs() < 1e-6, "{a} {b}");
    }
    p.set_param(GAIN, 0.25);
    p.process(Some((&l, &l)), &[], &mut ol, &mut or);
    for (a, b) in l.iter().zip(&or) {
        assert!((a * 0.5 - b).abs() < 1e-6);
    }
    assert_eq!(p.param_value(GAIN), Some(0.25));
    assert_eq!(p.param_display(GAIN), "0.50 x");
}

#[test]
fn as_an_instrument_it_plays_the_midi_notes_it_is_sent() {
    let mut p = Vst2::load(&info(), SR).unwrap();
    let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
    p.process(None, &[], &mut l, &mut r);
    assert_eq!(rms(&l), 0.0);
    let on = [NoteMsg::On {
        offset: 100,
        key: 69,
        vel: 1.0,
    }];
    p.process(None, &on, &mut l, &mut r);
    assert_eq!(rms(&l[..100]), 0.0, "the note starts at its offset");
    let level = rms(&l[100..]);
    assert!((0.3..0.4).contains(&level), "{level}");
    let crossings = l.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert!((17..=20).contains(&crossings), "{crossings}");
    // a note in a later block of the same call
    let late = [NoteMsg::Off {
        offset: 1500,
        key: 69,
    }];
    p.process(None, &late, &mut l, &mut r);
    assert!(rms(&l[..1500]) > 0.3, "it keeps sounding");
    assert_eq!(rms(&l[1500..]), 0.0);
}

#[test]
fn its_parameters_are_listed_and_its_chunk_round_trips() {
    let mut p = Vst2::load(&info(), SR).unwrap();
    let ps = p.params();
    assert_eq!(ps.len(), 1);
    assert_eq!((ps[0].id, ps[0].name.as_str()), (GAIN, "Gain"));
    assert_eq!((ps[0].min, ps[0].max, ps[0].default), (0.0, 1.0, 0.5));
    p.set_param(GAIN, 0.75);
    let state = p.save_state().expect("state");
    assert!(state.starts_with(b"LVS2\x01"), "a chunk: {state:?}");

    let mut q = Vst2::load(&info(), SR).unwrap();
    assert_eq!(q.param_value(GAIN), Some(0.5));
    q.load_state(&state).unwrap();
    assert_eq!(q.param_value(GAIN), Some(0.75));
    assert!(q.load_state(&[1, 2]).is_err(), "a short state is refused");
    assert!(q.load_state(b"LVS2\x00\0\0\0\0\x05\0\0\0").is_err());
}

#[test]
fn in_the_plugin_host_program_it_plays_and_keeps_its_state() {
    let exe = las_host::bridge::host_exe().expect("lumen-plugin-host is built with the tests");
    let mut p = las_host::bridge::Remote::load_with(&exe, &info(), SR).unwrap();
    assert!(p.takes_audio());
    assert_eq!(p.params().len(), 1);
    let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
    let on = [NoteMsg::On {
        offset: 0,
        key: 69,
        vel: 1.0,
    }];
    p.process(None, &on, &mut l, &mut r);
    assert!(rms(&l) > 0.3);
    p.set_param(GAIN, 0.1);
    p.process(None, &[], &mut l, &mut r);
    let s = p.save_state().unwrap();
    let mut q = Plugin::Remote(las_host::bridge::Remote::load_with(&exe, &info(), SR).unwrap());
    q.load_state(&s).unwrap();
    assert!((q.param_value(GAIN).unwrap() - 0.1).abs() < 1e-6);
}

#[test]
fn a_scan_in_the_plugin_host_program_says_what_the_file_is() {
    let exe = las_host::bridge::host_exe().expect("lumen-plugin-host");
    let got =
        las_host::scan::describe_with(&exe, &testvst2(), las_host::scan::DESCRIBE_TIMEOUT).unwrap();
    assert_eq!(got, vec![info()]);
    // a library that is no plugin is reported as such
    let me = common::library_path("las-testvst2");
    let not = me.with_file_name(format!("not-a-plugin{}", std::env::consts::DLL_SUFFIX));
    // any binary without VSTPluginMain: this test program
    let any = std::env::current_exe().unwrap();
    let _ = std::fs::copy(&any, &not);
    let e =
        las_host::scan::describe_with(&exe, &not, las_host::scan::DESCRIBE_TIMEOUT).unwrap_err();
    assert!(!e.is_empty());
    let _ = std::fs::remove_file(not);
}

#[test]
fn the_state_a_project_keeps_survives_base64() {
    let mut p = Vst2::load(&info(), SR).unwrap();
    p.set_param(GAIN, 0.3);
    let bytes = p.save_state().unwrap();
    let mut state = las_host::plugin_state(&info().id, &[]);
    las_host::set_saved_state(&mut state, &bytes);
    assert_eq!(las_host::saved_state(&state), Some(bytes));
}

#[cfg(target_os = "linux")]
#[test]
fn the_plugin_window_sends_what_the_user_turns() {
    use las_host::Editor;
    use std::time::{Duration, Instant};
    let _xvfb = common::display();
    let i = info();
    let mut ed =
        Editor::open_with_state(&i, None, &[(GAIN, 0.5)], "Test").expect("the window opens");
    // the edit reported while it was opened is the plugin's own setup, not the user's
    let until = Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    while got.is_empty() && Instant::now() < until {
        got = ed.idle();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(got, vec![(GAIN, 0.75)], "the edit of its third idle call");
    let (chunk, values) = ed.state().expect("a VST2 window has a state");
    assert_eq!(values, vec![(GAIN, 0.75)]);
    // the window opened with that state shows it
    let mut p = Vst2::load(&i, SR).unwrap();
    p.load_state(&chunk).unwrap();
    assert_eq!(p.param_value(GAIN), Some(0.75));
    ed.set_param(GAIN, 0.1);
    assert!(ed.idle().is_empty(), "a value set by the host is no edit");
    assert!(!ed.closed());
}
