// SPDX-License-Identifier: GPL-3.0-or-later
//! The host against the test plugin (`las-testplug`), a real CLAP library.

mod common;

use common::testplug;
use las_host::{describe, scan, Clap, NoteMsg, PluginInfo};

const SR: f64 = 48_000.0;

fn info() -> PluginInfo {
    describe(&testplug()).pop().expect("the test plugin")
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_scan_finds_the_plugin_and_its_kind() {
    let p = testplug();
    let found = scan(&[p.parent().unwrap().to_path_buf()]);
    let t = found
        .iter()
        .find(|i| i.id == "las.test.gain")
        .expect("found");
    assert_eq!(t.name, "Lumen Test Gain");
    assert_eq!(t.vendor, "Lumen Audio Studio");
    assert!(t.is_instrument() && t.is_effect());
}

#[test]
fn a_library_that_is_not_a_plugin_is_skipped() {
    let dir = std::env::temp_dir().join(format!("las-host-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bad.clap"), b"not a library").unwrap();
    assert!(scan(std::slice::from_ref(&dir)).is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn as_an_effect_it_scales_the_input_by_its_gain() {
    let mut p = Clap::load(&info(), SR).unwrap();
    assert!(p.takes_audio());
    let l: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let (mut ol, mut or) = (vec![0.0; 256], vec![0.0; 256]);
    p.process(Some((&l, &l)), &[], &mut ol, &mut or);
    for (a, b) in l.iter().zip(&ol) {
        assert!((a - b).abs() < 1e-6);
    }
    p.set_param(1, 0.5);
    p.process(Some((&l, &l)), &[], &mut ol, &mut or);
    for (a, b) in l.iter().zip(&or) {
        assert!((a * 0.5 - b).abs() < 1e-6);
    }
    assert_eq!(p.param_value(1), Some(0.5));
}

#[test]
fn as_an_instrument_it_plays_the_notes_it_is_sent() {
    let mut p = Clap::load(&info(), SR).unwrap();
    let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
    p.process(None, &[], &mut l, &mut r);
    assert_eq!(rms(&l), 0.0, "silent without notes");
    let on = [NoteMsg::On {
        offset: 100,
        key: 69,
        vel: 1.0,
    }];
    p.process(None, &on, &mut l, &mut r);
    assert_eq!(rms(&l[..100]), 0.0, "the note starts at its offset");
    let level = rms(&l[100..]);
    assert!((0.3..0.4).contains(&level), "a sine of 0.5: {level}");
    // 440 Hz: about 20 cycles in 2048 frames at 48 kHz
    let crossings = l.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert!((17..=20).contains(&crossings), "{crossings}");
    // it keeps sounding in the next block, until the note ends
    p.process(None, &[], &mut l, &mut r);
    assert!(rms(&l) > 0.3);
    p.process(None, &[NoteMsg::Off { offset: 0, key: 69 }], &mut l, &mut r);
    assert_eq!(rms(&l), 0.0);
}

#[test]
fn blocks_longer_than_the_plugin_takes_are_split() {
    let mut p = Clap::load(&info(), SR).unwrap();
    let n = las_host::MAX_FRAMES * 3 + 17;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let on = [NoteMsg::On {
        offset: (las_host::MAX_FRAMES * 2 + 5) as u32,
        key: 60,
        vel: 1.0,
    }];
    p.process(None, &on, &mut l, &mut r);
    assert_eq!(rms(&l[..las_host::MAX_FRAMES * 2 + 5]), 0.0);
    assert!(rms(&l[las_host::MAX_FRAMES * 2 + 8..]) > 0.2);
}

#[test]
fn its_parameters_are_listed_and_its_state_round_trips() {
    let mut p = Clap::load(&info(), SR).unwrap();
    let ps = p.params();
    assert_eq!(ps.len(), 1);
    assert_eq!((ps[0].id, ps[0].name.as_str()), (1, "Gain"));
    assert_eq!((ps[0].min, ps[0].max, ps[0].default), (0.0, 4.0, 1.0));

    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    p.set_param(1, 2.5);
    p.process(None, &[], &mut l, &mut r);
    let state = p.save_state().expect("state");

    let mut q = Clap::load(&info(), SR).unwrap();
    assert_eq!(q.param_value(1), Some(1.0));
    q.load_state(&state).unwrap();
    assert_eq!(q.param_value(1), Some(2.5));
    assert!(q.load_state(&[1]).is_err(), "a short state is refused");
}
