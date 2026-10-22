// SPDX-License-Identifier: GPL-3.0-or-later
//! Times the engine on the demo song with a long audio clip added: how long a player
//! takes to build (what pressing play costs) and how long 30 seconds take to render.
//! `cargo run --release -p las-engine --example perf`

use las_engine::{PlayMode, Player};
use las_project::create::add_channel;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../las-project/tests/fixtures/demo.las"
    );
    let mut b = las_project::decode(&std::fs::read(path).unwrap()).unwrap();
    // a 20 second stereo 44.1 kHz audio clip, as a dropped file would be
    let frames = 44_100 * 20;
    let a = las_engine::factory::Audio {
        channels: (0..2)
            .map(|c| {
                (0..frames)
                    .map(|i| (i as f32 * 0.01 * (c + 1) as f32).sin() * 0.3)
                    .collect()
            })
            .collect(),
        sample_rate: 44_100.0,
    };
    let p = &mut b.project;
    p.samples.push(las_project::SampleRef {
        id: "smp_bench".into(),
        name: "bench".into(),
        duration: Some(20.0),
        ..Default::default()
    });
    b.samples.insert(
        "smp_bench".into(),
        las_project::Sample {
            name: "bench".into(),
            mime: "audio/wav".into(),
            data: las_engine::library::encode_wav(&a),
        },
    );
    let ch = add_channel(p, "audioclip");
    p.channels[ch].sample_id = Some("smp_bench".into());
    p.playlist.clips.push(las_project::Clip {
        id: "clip_bench".into(),
        kind: las_project::ClipKind::Audio,
        ref_id: p.channels[ch].id.clone(),
        track: 5,
        start: 0.0,
        length: 20.0 * p.bpm / 60.0 * f64::from(las_project::PPQ),
        ..Default::default()
    });
    let project = Arc::new(b.project);
    let sr = 48_000.0;
    let mut builds = Vec::new();
    let mut player = None;
    for _ in 0..5 {
        let t = Instant::now();
        player = Some(Player::new(project.clone(), &b.samples, sr, PlayMode::Song).unwrap());
        builds.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let mut player = player.unwrap();
    player.play(0.0);
    let (mut l, mut r) = (vec![0f32; 512], vec![0f32; 512]);
    let secs: f64 = std::env::var("LAS_PERF_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30.0);
    let blocks = (secs * sr / 512.0) as usize;
    let t = Instant::now();
    for _ in 0..blocks {
        player.process(&mut l, &mut r);
    }
    let render = t.elapsed().as_secs_f64();
    // stopped: the player keeps running for tails; nothing sounds after a few seconds
    let mut idle = Player::new(project.clone(), &b.samples, sr, PlayMode::Song).unwrap();
    idle.play(0.0);
    for _ in 0..20 {
        idle.process(&mut l, &mut r);
    }
    idle.stop();
    let t = Instant::now();
    for _ in 0..blocks {
        idle.process(&mut l, &mut r);
    }
    let stopped = t.elapsed().as_secs_f64();
    let t = Instant::now();
    for _ in 0..blocks {
        idle.process(&mut l, &mut r);
    }
    let asleep = t.elapsed().as_secs_f64();
    println!(
        "stopped, the next 30 s (all asleep): {:.1} ms ({:.3}% of one core)",
        asleep * 1000.0,
        asleep / secs * 100.0
    );
    println!("awake when stopped (channels, inserts): {:?}", idle.awake());
    println!(
        "stopped 30 s: {:.1} ms ({:.2}% of one core)",
        stopped * 1000.0,
        stopped / secs * 100.0
    );
    println!(
        "build (play) ms: first {:.1}, then {:?}",
        builds[0],
        builds[1..]
            .iter()
            .map(|x| (x * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );
    println!(
        "render {secs} s: {:.1} ms ({:.1}% of one core, realtime x{:.0})",
        render * 1000.0,
        render / secs * 100.0,
        secs / render
    );
    println!("decodes: {}", las_engine::library::decode_count());
}
