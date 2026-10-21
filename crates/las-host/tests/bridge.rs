// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugins in their own process: the test plugins run through `lumen-plugin-host` sound
//! the same as in this process, keep their state, and a plugin whose process dies is
//! silent without taking the host with it.

mod common;

use common::{testplug, testvst3};
use las_host::bridge::Remote;
use las_host::{describe_any, NoteMsg, Plugin, PluginInfo};
use std::path::Path;

const SR: f64 = 48_000.0;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_lumen-plugin-host"))
}

fn infos() -> Vec<PluginInfo> {
    let mut v = describe_any(&testplug());
    v.extend(describe_any(&testvst3()));
    assert_eq!(v.len(), 2);
    v
}

fn signal() -> Vec<f32> {
    (0..1000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()
}

#[test]
fn a_plugin_in_its_own_process_sounds_the_same() {
    for info in infos() {
        let mut local = Plugin::load_local(&info, SR).unwrap();
        let mut remote = Remote::load_with(exe(), &info, SR).unwrap();
        assert_eq!(remote.params(), local.params(), "{}", info.id);
        assert_eq!(remote.takes_audio(), local.takes_audio());
        let id = local.params()[0].id;
        local.set_param(id, 0.25);
        remote.set_param(id, 0.25);
        let x = signal();
        let (mut a, mut b) = (vec![0.0; 1000], vec![0.0; 1000]);
        let (mut c, mut d) = (vec![0.0; 1000], vec![0.0; 1000]);
        local.process(Some((&x, &x)), &[], &mut a, &mut b);
        remote.process(Some((&x, &x)), &[], &mut c, &mut d);
        assert_eq!(a, c, "{}", info.id);
        assert_eq!(b, d);
        // notes reach it too
        let on = [NoteMsg::On {
            offset: 10,
            key: 69,
            vel: 1.0,
        }];
        local.process(None, &on, &mut a, &mut b);
        remote.process(None, &on, &mut c, &mut d);
        assert_eq!(a, c);
        assert!(c.iter().any(|&v| v != 0.0));
        assert_eq!(remote.param_value(id), local.param_value(id));
        assert!(!remote.crashed());
    }
}

#[test]
fn state_goes_through_the_process() {
    for info in infos() {
        let mut a = Remote::load_with(exe(), &info, SR).unwrap();
        let id = a.params()[0].id;
        a.set_param(id, 0.125);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        a.process(None, &[], &mut l, &mut r);
        let state = a.save_state().expect("a state");
        let mut b = Remote::load_with(exe(), &info, SR).unwrap();
        b.load_state(&state).unwrap();
        assert_eq!(b.save_state().unwrap(), state, "{}", info.id);
    }
}

#[cfg(unix)]
#[test]
fn a_plugin_whose_process_dies_is_silent() {
    let info = infos().remove(0);
    let mut p = Remote::load_with(exe(), &info, SR).unwrap();
    let ok = std::process::Command::new("kill")
        .args(["-9", &p.process_id().to_string()])
        .status()
        .unwrap();
    assert!(ok.success());
    let x = signal();
    let (mut l, mut r) = (vec![1.0; 1000], vec![1.0; 1000]);
    p.process(Some((&x, &x)), &[], &mut l, &mut r);
    assert!(l.iter().chain(&r).all(|&v| v == 0.0));
    assert!(p.crashed());
    // asking more of it is harmless
    assert_eq!(p.param_value(1), None);
    assert!(p.save_state().is_none());
    p.process(None, &[], &mut l, &mut r);
}

#[test]
fn a_missing_plugin_is_an_error_not_a_hang() {
    let mut info = infos().remove(0);
    info.id = "no.such.plugin".into();
    assert!(Remote::load_with(exe(), &info, SR).is_err());
}
