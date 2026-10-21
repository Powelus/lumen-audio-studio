// SPDX-License-Identifier: GPL-3.0-or-later
//! The edits of an audio clip (fades, gain, reverse, normalize, time stretch and pitch)
//! each change what the render plays, the way FL Studio's audio clip options do.

use las_engine::render::{Mode, RenderOptions};
use las_engine::render_mix_with;
use las_project::create::{add_channel, empty_project};
use las_project::{
    AudioClipEdit, Clip, ClipKind, Project, Sample, SampleRef, Stretch, StretchMode,
};
use std::collections::BTreeMap;

const SR: u32 = 44100;

/// A WAV of `seconds`: a 220 Hz sine whose level rises from `lo` to `hi`.
fn wav(seconds: f32, lo: f32, hi: f32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
    let n = (SR as f32 * seconds) as usize;
    for i in 0..n {
        let x = i as f32 / n as f32;
        let level = lo + (hi - lo) * x;
        let v = (i as f32 * 220.0 * std::f32::consts::TAU / SR as f32).sin() * level;
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    out.into_inner()
}

/// A project of one audio clip, one bar (2 s at 120 BPM) long, with `edit`.
fn project(edit: Option<AudioClipEdit>) -> (Project, BTreeMap<String, Sample>) {
    let mut p = empty_project("clip");
    p.bpm = 120.0;
    let ch = add_channel(&mut p, "audioclip");
    p.channels[ch].sample_id = Some("smp_a".into());
    p.samples.push(SampleRef {
        id: "smp_a".into(),
        name: "a".into(),
        ..Default::default()
    });
    p.playlist.clips.push(Clip {
        id: "clip_a".into(),
        kind: ClipKind::Audio,
        ref_id: p.channels[ch].id.clone(),
        track: 0,
        start: 0.0,
        length: 384.0,
        audio: edit,
        ..Default::default()
    });
    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_a".to_string(),
        Sample {
            name: "a".into(),
            mime: "audio/wav".into(),
            data: wav(2.0, 0.05, 0.25),
        },
    );
    (p, samples)
}

fn render(edit: Option<AudioClipEdit>) -> Vec<f32> {
    let (p, s) = project(edit);
    let opts = RenderOptions {
        sample_rate: f64::from(SR),
        tail: 0.0,
        mode: Mode::Range {
            from: 0.0,
            to: 384.0,
        },
    };
    render_mix_with(&p, &s, &opts).unwrap().left
}

/// The peak level of the render between two times, seconds.
fn peak(buf: &[f32], from: f64, to: f64) -> f32 {
    let a = (from * f64::from(SR)) as usize;
    let b = ((to * f64::from(SR)) as usize).min(buf.len());
    buf[a..b].iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn edit() -> AudioClipEdit {
    AudioClipEdit::default()
}

#[test]
fn a_plain_clip_plays_its_sample_rising() {
    let r = render(None);
    let (start, end) = (peak(&r, 0.1, 0.3), peak(&r, 1.6, 1.8));
    assert!(start > 0.01 && end > start * 2.5, "{start} {end}");
}

#[test]
fn a_fade_in_starts_silent_and_a_fade_out_ends_silent() {
    let plain = render(None);
    let faded = render(Some(AudioClipEdit {
        fade_in: Some(192.0),
        fade_out: Some(96.0),
        ..edit()
    }));
    assert!(peak(&faded, 0.0, 0.05) < peak(&plain, 0.0, 0.05) * 0.2);
    assert!(peak(&faded, 1.95, 1.99) < peak(&plain, 1.95, 1.99) * 0.2);
    // the middle is left alone
    let (a, b) = (peak(&faded, 1.2, 1.4), peak(&plain, 1.2, 1.4));
    assert!((a - b).abs() < b * 0.05, "{a} {b}");
}

#[test]
fn gain_scales_the_clip_in_decibels() {
    let plain = peak(&render(None), 0.5, 1.5);
    let down = peak(
        &render(Some(AudioClipEdit {
            gain: Some(-6.0),
            ..edit()
        })),
        0.5,
        1.5,
    );
    let ratio = down / plain;
    assert!((ratio - 0.501).abs() < 0.03, "{ratio}");
}

#[test]
fn reverse_plays_the_loud_end_first() {
    let r = render(Some(AudioClipEdit {
        reverse: Some(true),
        ..edit()
    }));
    assert!(peak(&r, 0.1, 0.3) > peak(&r, 1.6, 1.8) * 2.5);
}

#[test]
fn normalize_brings_the_peak_to_full_scale() {
    let plain = peak(&render(None), 0.0, 2.0);
    let r = render(Some(AudioClipEdit {
        normalize: Some(true),
        ..edit()
    }));
    let loud = peak(&r, 0.0, 2.0);
    // the source peaks at 0.25: normalizing is four times (12 dB) louder
    assert!((loud / plain - 4.0).abs() < 0.3, "{plain} {loud}");
}

/// The strongest frequency of a stretch of audio, by counting zero crossings.
fn frequency(buf: &[f32], from: f64, to: f64) -> f64 {
    let a = (from * f64::from(SR)) as usize;
    let b = (to * f64::from(SR)) as usize;
    let crossings = buf[a..b]
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    crossings as f64 / 2.0 / (to - from)
}

#[test]
fn pitch_shifts_by_semitones_and_keeps_the_timing() {
    let plain = render(None);
    let up = render(Some(AudioClipEdit {
        pitch: Some(12.0),
        ..edit()
    }));
    let (f0, f1) = (frequency(&plain, 0.5, 1.5), frequency(&up, 0.5, 1.5));
    assert!((f0 - 220.0).abs() < 5.0, "{f0}");
    assert!((f1 - 440.0).abs() < 15.0, "{f1}");
    // still sounding at the end: the length is the same
    assert!(peak(&up, 1.7, 1.9) > 0.02);
}

#[test]
fn a_time_stretch_to_the_tempo_keeps_the_pitch_and_changes_the_length() {
    // the sample was made at 60 BPM: at 120 BPM it plays twice as fast, same pitch
    let r = render(Some(AudioClipEdit {
        stretch: Some(Stretch {
            bpm: 60.0,
            mode: StretchMode::Stretch,
        }),
        ..edit()
    }));
    let f = frequency(&r, 0.2, 0.8);
    assert!((f - 220.0).abs() < 10.0, "{f}");
    // the whole sample is over after one second
    assert!(peak(&r, 0.7, 0.9) > 0.05);
    assert!(peak(&r, 1.3, 1.9) < 0.01);
}

#[test]
fn old_projects_without_the_new_fields_still_load() {
    let e: AudioClipEdit = serde_json::from_str(r#"{"gain": -3}"#).unwrap();
    assert_eq!(e.gain, Some(-3.0));
    assert_eq!(e.normalize, None);
    let json = serde_json::to_string(&AudioClipEdit {
        normalize: Some(true),
        ..edit()
    })
    .unwrap();
    assert_eq!(json, r#"{"normalize":true}"#);
}
