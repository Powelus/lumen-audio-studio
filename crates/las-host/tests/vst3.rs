// SPDX-License-Identifier: GPL-3.0-or-later
//! The host against the test VST3 plugin (`las-testvst3`), a real VST3 library.

mod common;

use common::testvst3;
use las_host::vst3::Vst3;
use las_host::{describe_any, scan, Format, NoteMsg, PluginInfo};

const SR: f64 = 48_000.0;

fn info() -> PluginInfo {
    describe_any(&testvst3()).pop().expect("the test plugin")
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_scan_finds_the_plugin_and_its_kind() {
    let found = scan(&[testvst3().parent().unwrap().to_path_buf()]);
    let t = found
        .iter()
        .find(|i| i.format == Format::Vst3)
        .expect("found");
    assert_eq!(t.name, "Lumen Test VST3");
    assert_eq!(t.vendor, "Lumen Audio Studio");
    assert!(t.is_instrument() && t.is_effect());
    assert_eq!(t.id.len(), 32);
}

#[test]
fn as_an_effect_it_scales_the_input_by_its_gain() {
    let mut p = Vst3::load(&info(), SR).unwrap();
    assert!(p.takes_audio());
    let l: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let (mut ol, mut or) = (vec![0.0; 256], vec![0.0; 256]);
    // the default (0.5 of the range) is unity gain
    p.process(Some((&l, &l)), &[], &mut ol, &mut or);
    for (a, b) in l.iter().zip(&ol) {
        assert!((a - b).abs() < 1e-6, "{a} {b}");
    }
    p.set_param(1, 0.25);
    p.process(Some((&l, &l)), &[], &mut ol, &mut or);
    for (a, b) in l.iter().zip(&or) {
        assert!((a * 0.5 - b).abs() < 1e-6);
    }
    assert_eq!(p.param_value(1), Some(0.25));
}

#[test]
fn as_an_instrument_it_plays_the_notes_it_is_sent() {
    let mut p = Vst3::load(&info(), SR).unwrap();
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
    p.process(None, &[], &mut l, &mut r);
    assert!(rms(&l) > 0.3, "it keeps sounding");
    p.process(None, &[NoteMsg::Off { offset: 0, key: 69 }], &mut l, &mut r);
    assert_eq!(rms(&l), 0.0);
}

#[test]
fn its_parameters_are_listed_and_its_state_round_trips() {
    let mut p = Vst3::load(&info(), SR).unwrap();
    let ps = p.params();
    assert_eq!(ps.len(), 1);
    assert_eq!((ps[0].id, ps[0].name.as_str()), (1, "Gain"));
    assert_eq!((ps[0].min, ps[0].max, ps[0].default), (0.0, 1.0, 0.5));

    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    p.set_param(1, 0.75);
    p.process(None, &[], &mut l, &mut r);
    let state = p.save_state().expect("state");
    assert_eq!(state.len(), 8);

    let mut q = Vst3::load(&info(), SR).unwrap();
    assert_eq!(q.param_value(1), Some(0.5));
    q.load_state(&state).unwrap();
    assert_eq!(
        q.param_value(1),
        Some(0.75),
        "the controller follows the component"
    );
    assert!(q.load_state(&[1]).is_err(), "a short state is refused");
}

#[test]
fn a_bundle_without_a_binary_is_skipped() {
    let dir = std::env::temp_dir().join(format!("las-vst3-bad-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("empty.vst3")).unwrap();
    assert!(scan(std::slice::from_ref(&dir)).is_empty());
    std::fs::remove_dir_all(dir).ok();
}
