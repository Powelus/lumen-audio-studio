// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings of a sampler channel's Channel settings window (sample, envelope,
//! filter envelope, arpeggiator and chords) and a note's release each change what an
//! offline render of the channel sounds like.

use las_engine::{render_mix_with, Mode, RenderOptions};
use las_project::create::{add_channel, empty_project, step_note};
use las_project::{Project, SampleRef};
use serde_json::{json, Value};

const SR: f64 = 24000.0;

/// A sampler channel playing the long 808 with `state`, holding `keys` for a bar (piano
/// roll notes, gated by their length), with `release` as each note's release.
fn project(state: Value, keys: &[i32], release: Option<f64>) -> Project {
    let mut p = empty_project("x");
    let c = add_channel(&mut p, "sampler");
    let id = "factory:808-long".to_string();
    p.channels[c].sample_id = Some(id.clone());
    p.channels[c].state = state;
    p.samples.push(SampleRef {
        id,
        name: "808".into(),
        factory: Some(true),
        ..SampleRef::default()
    });
    let ch = p.channels[c].id.clone();
    let notes = keys
        .iter()
        .map(|&k| {
            let mut n = step_note(0, k, 0.9);
            n.len = 96.0 * 4.0;
            n.step = None;
            n.release = release;
            n
        })
        .collect();
    p.patterns[0].notes.insert(ch, notes);
    p
}

fn render(p: &Project) -> Vec<f32> {
    let opts = RenderOptions {
        sample_rate: SR,
        tail: 1.5,
        mode: Mode::Pattern { id: None, loops: 1 },
    };
    render_mix_with(p, &Default::default(), &opts).unwrap().left
}

fn play(state: Value) -> Vec<f32> {
    render(&project(state, &[48], None))
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
}

fn diff(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    a[..n]
        .iter()
        .zip(&b[..n])
        .map(|(x, y)| f64::from(x - y).abs())
        .sum()
}

fn differs(a: &[f32], b: &[f32], what: &str) {
    let d = diff(a, b);
    assert!(d > 1.0, "{what} changes the sound: {d}");
}

#[test]
fn the_sample_tab_changes_the_sound() {
    let plain = play(json!({}));
    assert!(energy(&plain) > 1.0, "the channel sounds");
    differs(&plain, &play(json!({ "tune": 7 })), "pitch in semitones");
    differs(&plain, &play(json!({ "fine": 50 })), "pitch in cents");
    differs(&plain, &play(json!({ "root": 55 })), "the root key");
    differs(&plain, &play(json!({ "reverse": true })), "reverse");
    let late = play(json!({ "start": 0.5 }));
    assert!(energy(&late) < energy(&plain) * 0.8, "a later start skips the attack");
    // a looped sample keeps sounding to the end of the note
    let s = (1.9 * SR) as usize;
    let e = (2.0 * SR) as usize;
    let short = play(json!({ "end": 0.1 }));
    let looped = play(json!({ "end": 0.1, "loop": true }));
    assert!(
        energy(&looped[s..e]) > energy(&short[s..e]) * 10.0 + 1e-3,
        "{} vs {}",
        energy(&looped[s..e]),
        energy(&short[s..e])
    );
}

#[test]
fn the_envelope_tab_changes_the_sound() {
    let plain = play(json!({}));
    let first = (0.1 * SR) as usize;
    let slow = play(json!({ "attack": 1.0 }));
    assert!(energy(&slow[..first]) < energy(&plain[..first]) * 0.2, "a slow attack");
    let low = play(json!({ "sustain": 0.1, "decay": 0.05 }));
    assert!(energy(&low) < energy(&plain) * 0.5, "a low sustain");
    // the filter envelope opens a closed filter
    let closed = play(json!({ "cutoff": 200.0 }));
    let swept = play(json!({ "cutoff": 200.0, "fltEnvAmt": 6.0, "fltDecay": 0.5 }));
    differs(&closed, &swept, "the filter envelope");
    let res = play(json!({ "cutoff": 400.0, "res": 12.0, "fltEnvAmt": 3.0 }));
    differs(&swept, &res, "resonance");
}

#[test]
fn a_notes_release_stretches_the_samplers_release() {
    let state = json!({ "release": 0.4 });
    let tail = |r: f64| {
        let out = render(&project(state.clone(), &[48], Some(r)));
        // the note ends at 2 s (120 bpm, one bar)
        energy(&out[(2.05 * SR) as usize..])
    };
    let (short, mid, long) = (tail(0.0), tail(0.5), tail(1.0));
    assert!(short < mid && mid < long, "{short} {mid} {long}");
    // 0.5, the default, is the same as no release at all
    let none = render(&project(state.clone(), &[48], None));
    let half = render(&project(state, &[48], Some(0.5)));
    assert!(diff(&none, &half) < 1e-6);
}

#[test]
fn the_arpeggiator_and_chords_change_the_sound() {
    let chord = [48, 52, 55];
    let held = render(&project(json!({}), &chord, None));
    let up = render(&project(
        json!({ "arp": { "on": true, "mode": "up", "div": 0.25 } }),
        &chord,
        None,
    ));
    differs(&held, &up, "the arpeggiator");
    let down = render(&project(
        json!({ "arp": { "on": true, "mode": "down", "div": 0.25 } }),
        &chord,
        None,
    ));
    differs(&up, &down, "the arp's direction");
    let slower = render(&project(
        json!({ "arp": { "on": true, "mode": "up", "div": 0.5 } }),
        &chord,
        None,
    ));
    differs(&up, &slower, "the arp's time");
    let repeated = render(&project(
        json!({ "arp": { "on": true, "mode": "up", "div": 0.25, "repeat": 2 } }),
        &chord,
        None,
    ));
    differs(&up, &repeated, "the arp's repeat");
    let range = render(&project(
        json!({ "arp": { "on": true, "mode": "up", "div": 0.25, "octaves": 2 } }),
        &chord,
        None,
    ));
    differs(&up, &range, "the arp's range");
    let gated = render(&project(
        json!({ "arp": { "on": true, "mode": "up", "div": 0.25, "gate": 0.1 } }),
        &chord,
        None,
    ));
    assert!(energy(&gated) < energy(&up), "a short gate");
    // a chord preset turns one note into three
    let one = play(json!({}));
    let major = play(json!({ "arp": { "chord": "major" } }));
    differs(&one, &major, "a chord preset");
    let as_chord = render(&project(json!({}), &chord, None));
    assert!(diff(&major, &as_chord) < 1e-3, "major is the root, a third and a fifth");
}
