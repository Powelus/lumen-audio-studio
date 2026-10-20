// SPDX-License-Identifier: GPL-3.0-or-later
//! A project with hosted plugins, rendered by the engine.

mod common;

use common::{testplug, testvst3};
use las_engine::{render_mix, Mode, RenderOptions};
use las_host::{install, plugin_state, rescan, HOSTED};
use las_project::create::{add_channel, effect_slot, empty_project, step_note};
use las_project::Project;

const SR: f64 = 24_000.0;

fn setup() {
    let dirs = [
        testplug().parent().unwrap().to_path_buf(),
        testvst3().parent().unwrap().to_path_buf(),
    ];
    assert!(rescan(&dirs).len() >= 2);
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

fn vst3_id() -> String {
    las_host::known()
        .into_iter()
        .find(|p| p.format == las_host::Format::Vst3)
        .expect("the test VST3 plugin was found")
        .id
}

#[test]
fn a_vst3_instrument_plays_the_notes_of_a_pattern() {
    setup();
    let id = vst3_id();
    let out = render_mix(&song(&id, &[]), &opts()).unwrap();
    let level = rms(&out.left[..2400]);
    assert!(level > 0.05, "the plugin sounds: {level}");
    let part = &out.left[100..2500];
    let crossings = part
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    assert!((40..=46).contains(&crossings), "{crossings}");
    // a quarter of the range is half the gain
    let quiet = render_mix(&song(&id, &[(1, 0.25)]), &opts()).unwrap();
    let ratio = rms(&quiet.left[..2400]) / level;
    assert!((ratio - 0.5).abs() < 0.02, "{ratio}");
}

#[test]
fn a_vst3_effect_processes_an_insert() {
    setup();
    let mut p = empty_project("fx");
    let c = add_channel(&mut p, "polaris");
    p.channels[c].state = serde_json::json!({ "patch": {} });
    let id = p.channels[c].id.clone();
    let insert = p.channels[c].mixer_track;
    let mut n = step_note(0, 69, 1.0);
    n.len = 96.0;
    p.patterns[0].notes.insert(id, vec![n]);
    let dry = render_mix(&p, &opts()).unwrap();
    let mut slot = effect_slot(HOSTED);
    slot.state = plugin_state(&vst3_id(), &[(1, 0.25)]);
    p.mixer[insert].slots[0] = Some(slot);
    let wet = render_mix(&p, &opts()).unwrap();
    let ratio = rms(&wet.left) / rms(&dry.left);
    assert!((ratio - 0.5).abs() < 0.02, "{ratio}");
}

/// An automation clip that moves `param` of a target from `from` to `to` (0 to 1) over
/// the four bars of the clip.
fn automate(p: &mut Project, target: serde_json::Value, from: f64, to: f64) {
    use las_project::{AutomationClip, AutomationPoint, Clip, ClipKind};
    let length = 96.0 * 16.0;
    p.automation.push(AutomationClip {
        id: "auto1".into(),
        name: "test".into(),
        target,
        points: vec![
            AutomationPoint {
                t: 0.0,
                v: from,
                ..Default::default()
            },
            AutomationPoint {
                t: length,
                v: to,
                ..Default::default()
            },
        ],
        length,
        ..Default::default()
    });
    p.playlist.clips.push(Clip {
        id: "clip_auto".into(),
        kind: ClipKind::Automation,
        ref_id: "auto1".into(),
        track: 1,
        start: 0.0,
        length,
        ..Default::default()
    });
}

/// A hosted instrument holding one long note for four bars, in the song.
fn long_song(plugin: &str) -> Project {
    let mut p = song(plugin, &[]);
    let ch = p.channels[0].id.clone();
    let n = &mut p.patterns[0].notes.get_mut(&ch).unwrap()[0];
    n.len = 96.0 * 16.0;
    let pat = p.patterns[0].id.clone();
    p.playlist.clips.push(las_project::Clip {
        id: "clip_pat".into(),
        kind: las_project::ClipKind::Pattern,
        ref_id: pat,
        track: 0,
        start: 0.0,
        length: 96.0 * 16.0,
        ..Default::default()
    });
    p
}

fn song_opts() -> RenderOptions {
    RenderOptions {
        sample_rate: SR,
        tail: 0.0,
        mode: Mode::Song,
    }
}

#[test]
fn automation_moves_a_hosted_instruments_parameter() {
    setup();
    for id in ["las.test.gain".to_string(), vst3_id()] {
        let mut p = long_song(&id);
        let ch = p.channels[0].id.clone();
        automate(
            &mut p,
            serde_json::json!({ "kind": "plugin", "channelId": ch, "param": "p.1" }),
            0.5,
            0.0,
        );
        let out = render_mix(&p, &song_opts()).unwrap();
        let n = out.left.len();
        let (early, late) = (
            rms(&out.left[..n / 8]),
            rms(&out.left[n * 6 / 8..n * 7 / 8]),
        );
        assert!(early > 0.05, "{id}: {early}");
        assert!(
            late < early * 0.4,
            "{id}: the gain falls: {early} -> {late}"
        );
    }
}

#[test]
fn automation_moves_a_hosted_effects_parameter() {
    setup();
    let mut p = long_song("las.test.gain");
    // the instrument plays into an insert with the plugin as an effect, automated
    let insert = p.channels[0].mixer_track;
    let mut slot = effect_slot(HOSTED);
    slot.state = plugin_state("las.test.gain", &[]);
    let slot_id = slot.id.clone();
    p.mixer[insert].slots[0] = Some(slot);
    automate(
        &mut p,
        serde_json::json!({ "kind": "effect", "track": insert, "slotId": slot_id, "param": "p.1" }),
        0.25,
        0.0,
    );
    let out = render_mix(&p, &song_opts()).unwrap();
    let n = out.left.len();
    let (early, late) = (
        rms(&out.left[..n / 8]),
        rms(&out.left[n * 6 / 8..n * 7 / 8]),
    );
    assert!(early > 0.05, "{early}");
    assert!(late < early * 0.4, "{early} -> {late}");
}
