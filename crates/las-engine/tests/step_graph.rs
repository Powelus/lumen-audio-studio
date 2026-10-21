// SPDX-License-Identifier: GPL-3.0-or-later
//! What the channel rack's graph editor sets on a step (velocity, pan, fine pitch)
//! changes what is heard.

use las_engine::render::{render_mix, Mode, RenderOptions};
use las_engine::timeline::STEP_F;
use las_project::create::{add_channel, default_project};
use las_project::{Note, Project};

/// A project whose only sound is one step of a synth channel, set up by `f`.
fn one_step(f: impl Fn(&mut Note)) -> Project {
    let mut p = default_project();
    p.channels.clear();
    for pat in &mut p.patterns {
        pat.notes.clear();
    }
    let at = add_channel(&mut p, "polaris");
    let id = p.channels[at].id.clone();
    let mut n = Note {
        id: "s0".into(),
        key: 69,
        pos: 0.0,
        len: STEP_F * 4.0,
        vel: 0.8,
        step: Some(true),
        ..Default::default()
    };
    f(&mut n);
    p.patterns[0].notes.insert(id, vec![n]);
    p.patterns[0].length = STEP_F * 16.0;
    p
}

fn render(p: &Project) -> (Vec<f32>, Vec<f32>) {
    let opts = RenderOptions {
        sample_rate: 24_000.0,
        tail: 0.2,
        mode: Mode::Pattern { id: None, loops: 1 },
    };
    let r = render_mix(p, &opts).expect("renders");
    (r.left, r.right)
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v).powi(2)).sum()
}

/// Zero crossings going up, for a rough pitch.
fn crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
}

#[test]
fn velocity_pan_and_fine_pitch_of_a_step_change_the_render() {
    let (l, r) = render(&one_step(|_| {}));
    let base = energy(&l) + energy(&r);
    assert!(base > 0.0, "the step sounds");
    // quieter
    let (ql, qr) = render(&one_step(|n| n.vel = 0.2));
    assert!(energy(&ql) + energy(&qr) < base * 0.7);
    // panned hard left: the right side all but goes
    let (pl, pr) = render(&one_step(|n| n.pan = -1.0));
    assert!(
        energy(&pr) < energy(&pl) * 0.2,
        "{} {}",
        energy(&pl),
        energy(&pr)
    );
    // a semitone up has more cycles
    let (fl, _) = render(&one_step(|n| n.fine = Some(100.0)));
    let (a, b) = (crossings(&l), crossings(&fl));
    assert!(b as f64 > a as f64 * 1.03, "{a} -> {b}");
}
