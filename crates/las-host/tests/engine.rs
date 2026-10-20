// SPDX-License-Identifier: GPL-3.0-or-later
//! A project with hosted plugins, rendered by the engine.

mod common;

use common::testplug;
use las_engine::{render_mix, Mode, RenderOptions};
use las_host::{install, plugin_state, rescan, HOSTED};
use las_project::create::{add_channel, effect_slot, empty_project, step_note};
use las_project::Project;

const SR: f64 = 24_000.0;

fn setup() {
    let dir = testplug().parent().unwrap().to_path_buf();
    assert!(!rescan(&[dir]).is_empty());
    install();
}

fn opts() -> RenderOptions {
    RenderOptions {
        sample_rate: SR,
        tail: 0.2,
        mode: Mode::Pattern { id: None, loops: 1 },
    }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// A project with a hosted instrument that plays one note of the pattern.
fn song(plugin: &str, params: &[(u32, f64)]) -> Project {
    let mut p = empty_project("hosted");
    let c = add_channel(&mut p, HOSTED);
    p.channels[c].state = plugin_state(plugin, params);
    let id = p.channels[c].id.clone();
    let mut n = step_note(0, 69, 1.0);
    n.len = 96.0;
    p.patterns[0].notes.insert(id, vec![n]);
    p
}

#[test]
fn a_hosted_instrument_plays_the_notes_of_a_pattern() {
    setup();
    let out = render_mix(&song("las.test.gain", &[]), &opts()).unwrap();
    let level = rms(&out.left[..2400]);
    assert!(level > 0.05, "the plugin sounds: {level}");
    // an A: 440 Hz, 44 cycles in 0.1 s
    let part = &out.left[100..2500];
    let crossings = part
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    assert!((40..=46).contains(&crossings), "{crossings}");
}

#[test]
fn the_parameters_a_project_keeps_reach_the_plugin() {
    setup();
    let loud = render_mix(&song("las.test.gain", &[(1, 1.0)]), &opts()).unwrap();
    let quiet = render_mix(&song("las.test.gain", &[(1, 0.25)]), &opts()).unwrap();
    let (a, b) = (rms(&loud.left[..2400]), rms(&quiet.left[..2400]));
    assert!(a > 0.05 && (b / a - 0.25).abs() < 0.02, "{a} {b}");
}

#[test]
fn a_plugin_that_is_not_installed_is_silent() {
    setup();
    let out = render_mix(&song("no.such.plugin", &[]), &opts()).unwrap();
    assert_eq!(rms(&out.left), 0.0);
}

#[test]
fn a_hosted_effect_processes_an_insert() {
    setup();
    let mut p = empty_project("fx");
    // a synth to feed it
    let c = add_channel(&mut p, "polaris");
    p.channels[c].state = serde_json::json!({ "patch": {} });
    let id = p.channels[c].id.clone();
    let insert = p.channels[c].mixer_track;
    let mut n = step_note(0, 69, 1.0);
    n.len = 96.0;
    p.patterns[0].notes.insert(id, vec![n]);
    let dry = render_mix(&p, &opts()).unwrap();
    assert!(rms(&dry.left) > 0.01);

    let mut slot = effect_slot(HOSTED);
    slot.state = plugin_state("las.test.gain", &[(1, 0.5)]);
    p.mixer[insert].slots[0] = Some(slot);
    let wet = render_mix(&p, &opts()).unwrap();
    let ratio = rms(&wet.left) / rms(&dry.left);
    // the plugin halves the insert (and plays no note of its own)
    assert!((ratio - 0.5).abs() < 0.02, "{ratio}");

    let mut slot = effect_slot(HOSTED);
    slot.state = plugin_state("no.such.plugin", &[]);
    p.mixer[insert].slots[0] = Some(slot);
    let skipped = render_mix(&p, &opts()).unwrap();
    assert!(
        (rms(&skipped.left) / rms(&dry.left) - 1.0).abs() < 1e-6,
        "an effect that is not installed leaves the sound alone"
    );
}
