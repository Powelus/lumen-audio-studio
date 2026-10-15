// SPDX-License-Identifier: GPL-3.0-or-later
//! Every factory preset loads into its engine and makes sound.

use las_dsp::engine::{Event, Playable};
use las_dsp::fm::fm_engine;
use las_dsp::mist::mist_engine;
use las_dsp::presets::bank;
use las_dsp::synth::synth_engine;

const SR: f64 = 48000.0;

fn play(e: &mut dyn Playable, key: f64) -> (Vec<f32>, Vec<f32>) {
    e.queue(&[
        Event::On {
            time: 0.0,
            key,
            vel: 0.9,
            fine: 0.0,
            pan: 0.0,
        },
        Event::Off { time: 1.0, key },
    ]);
    let n = (SR * 1.6) as usize;
    let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
    let mut i = 0;
    while i < n {
        let end = (i + 128).min(n);
        e.process(&mut l[i..end], &mut r[i..end], i as f64 / SR);
        i = end;
    }
    (l, r)
}

fn check(id: &str, make: &dyn Fn() -> Box<dyn Playable>) {
    let b = bank(id).unwrap();
    let mut silent = Vec::new();
    for (i, p) in b.presets.iter().enumerate() {
        let mut e = make();
        let patch = b.patch(i).unwrap();
        e.set_patch(&patch)
            .unwrap_or_else(|err| panic!("{id}/{}: {err}", p.name));
        let (l, r) = play(e.as_mut(), 60.0);
        assert!(
            l.iter().chain(&r).all(|x| x.is_finite()),
            "{id}/{}: not finite",
            p.name
        );
        let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak < 1.6, "{id}/{}: too loud ({peak})", p.name);
        if peak < 1e-3 {
            silent.push(p.name.clone());
        }
    }
    assert!(silent.is_empty(), "{id}: silent presets {silent:?}");
}

#[test]
fn every_synth_preset_plays() {
    for id in ["polaris", "vortex", "monolith", "zenith", "triosc"] {
        check(id, &|| Box::new(synth_engine(SR, 1)));
    }
}

#[test]
fn every_facet_preset_plays() {
    check("facet", &|| Box::new(fm_engine(SR, 1)));
}

#[test]
fn every_mist_preset_plays() {
    check("mist", &|| Box::new(mist_engine(SR, 1)));
}
