// SPDX-License-Identifier: GPL-3.0-or-later
//! Playing again does not decode the audio again, and what has nothing to play sleeps
//! (smart disable) and wakes when a note comes.

use las_engine::library::{decode_count, encode_wav};
use las_engine::{PlayMode, Player};
use las_project::create::{add_channel, default_project};
use std::collections::BTreeMap;
use std::sync::Arc;

fn project_with_clip() -> (las_project::Project, BTreeMap<String, las_project::Sample>) {
    let mut p = default_project();
    let a = las_engine::factory::Audio {
        channels: vec![(0..44_100).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()],
        sample_rate: 44_100.0,
    };
    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_t".to_string(),
        las_project::Sample {
            name: "t".into(),
            mime: "audio/wav".into(),
            data: encode_wav(&a),
        },
    );
    p.samples.push(las_project::SampleRef {
        id: "smp_t".into(),
        name: "t".into(),
        duration: Some(1.0),
        ..Default::default()
    });
    let ch = add_channel(&mut p, "audioclip");
    p.channels[ch].sample_id = Some("smp_t".into());
    p.playlist.clips.push(las_project::Clip {
        id: "clip_t".into(),
        kind: las_project::ClipKind::Audio,
        ref_id: p.channels[ch].id.clone(),
        track: 0,
        start: 0.0,
        length: p.bpm / 60.0 * f64::from(las_project::PPQ),
        ..Default::default()
    });
    (p, samples)
}

#[test]
fn audio_is_decoded_once_and_quiet_parts_sleep() {
    let (p, samples) = project_with_clip();
    let p = Arc::new(p);
    let before = decode_count();
    let mut players: Vec<Player> = (0..4)
        .map(|_| Player::new(p.clone(), &samples, 48_000.0, PlayMode::Song).unwrap())
        .collect();
    // a different rate is another decode; the same rate again is not
    let _other = Player::new(p.clone(), &samples, 44_100.0, PlayMode::Song).unwrap();
    let _again = Player::new(p.clone(), &samples, 44_100.0, PlayMode::Song).unwrap();
    assert_eq!(decode_count() - before, 2, "each rate is decoded once");

    // a stopped player falls asleep once its tails are over
    let player = &mut players[0];
    let (mut l, mut r) = (vec![0f32; 480], vec![0f32; 480]);
    assert!(player.play(0.0));
    let mut heard = 0f32;
    for _ in 0..50 {
        player.process(&mut l, &mut r);
        heard = heard.max(l.iter().fold(0f32, |m, s| m.max(s.abs())));
    }
    assert!(heard > 0.01, "the clip plays");
    assert!(!player.awake().0.is_empty(), "the clip's channel runs");
    player.stop();
    for _ in 0..(48_000 * 4 / 480) {
        player.process(&mut l, &mut r);
    }
    let (ch, ins) = player.awake();
    assert!(ch.is_empty() && ins.is_empty(), "asleep: {ch:?} {ins:?}");
    // and wakes for a note at once
    player.note_on(0, 60.0, 0.9);
    player.process(&mut l, &mut r);
    player.process(&mut l, &mut r);
    assert!(!player.awake().0.is_empty(), "a note wakes the channel");
}
