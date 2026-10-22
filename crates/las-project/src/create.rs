// SPDX-License-Identifier: GPL-3.0-or-later
//! Making projects and their parts: ids, empty projects, mixer inserts,
//! channels, patterns.

use crate::model::*;
use serde_json::{json, Value};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

/// The instruments the web version has: id, name, colour.
pub const INSTRUMENTS: &[(&str, &str, &str)] = &[
    ("sampler", "Sampler", "#22d3ee"),
    ("audioclip", "Audio Clip", "#3ddc97"),
    ("polaris", "Polaris", "#5b8cff"),
    ("vortex", "Vortex", "#22d3ee"),
    ("monolith", "Monolith", "#ff7a45"),
    ("zenith", "Zenith", "#9be15d"),
    ("nebula", "Nebula", "#c26bff"),
    ("triosc", "TriOsc", "#ffb547"),
    ("facet", "Facet", "#ff9f43"),
    ("mist", "Mist", "#7dd3fc"),
    ("lumapad", "LumaPad", "#ff5c8a"),
];

pub fn instrument_known(id: &str) -> bool {
    INSTRUMENTS.iter().any(|i| i.0 == id)
}

/// Milliseconds since 1970, as the web version stamps projects.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

/// A new id: the prefix, four characters of time and eight random ones, in base 36.
pub fn uid(prefix: &str) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut random = RandomState::new().build_hasher().finish();
    let mut time = now_ms() as u64;
    let mut out = String::from(prefix);
    let mut tail = [0u8; 4];
    for c in tail.iter_mut().rev() {
        *c = DIGITS[(time % 36) as usize];
        time /= 36;
    }
    out.extend(tail.iter().map(|&c| c as char));
    for _ in 0..8 {
        out.push(DIGITS[(random % 36) as usize] as char);
        random /= 36;
    }
    out
}

/// The palette of channels, patterns, tracks and inserts.
pub const PALETTE: [&str; 12] = [
    "#7c5cff", "#22d3ee", "#ff5c8a", "#3ddc97", "#ffb547", "#5b8cff", "#c26bff", "#ff7a45",
    "#2ee6b6", "#f25fd0", "#9be15d", "#56b4ff",
];

pub fn palette_color(index: usize) -> &'static str {
    PALETTE[index % PALETTE.len()]
}

pub fn mixer_track(index: usize, name: Option<&str>) -> MixerTrack {
    MixerTrack {
        id: uid("mx_"),
        name: name.map_or_else(
            || {
                if index == 0 {
                    "Master".into()
                } else {
                    format!("Insert {index}")
                }
            },
            str::to_string,
        ),
        color: if index == 0 { "#7c5cff" } else { "#3a4256" }.into(),
        volume: 1.0,
        pan: 0.0,
        width: 1.0,
        mute: false,
        solo: false,
        slots: (0..MIXER_SLOTS).map(|_| None).collect(),
        sends: Vec::new(),
        output: if index == 0 { None } else { Some(0) },
        fx_bypass: false,
        eq: Default::default(),
        invert: false,
        swap: false,
        extra: Extra::new(),
    }
}

/// An effect with its default settings (an empty state for an unknown effect).
pub fn effect_slot(plugin_id: &str) -> EffectSlot {
    let state = las_dsp::fx::registry::descriptor(plugin_id)
        .map_or_else(|| json!({}), |d| d.default_state.clone());
    EffectSlot {
        id: uid("fx_"),
        plugin_id: plugin_id.into(),
        enabled: true,
        mix: 1.0,
        state,
        extra: Extra::new(),
    }
}

/// The kit a new LumaPad starts with: sixteen pads from the factory samples, on the keys
/// from C2 up, as drum machines lay them out.
pub const DEFAULT_PADS: [(&str, &str); 16] = [
    ("factory:kick-punch", "Kick"),
    ("factory:snare-tight", "Snare"),
    ("factory:clap-classic", "Clap"),
    ("factory:hat-closed", "Hat"),
    ("factory:hat-open", "Open hat"),
    ("factory:kick-deep", "Kick 2"),
    ("factory:snare-trap", "Snare 2"),
    ("factory:snap", "Snap"),
    ("factory:tom-low", "Tom low"),
    ("factory:tom-mid", "Tom mid"),
    ("factory:tom-high", "Tom high"),
    ("factory:rim", "Rim"),
    ("factory:cowbell", "Cowbell"),
    ("factory:shaker", "Shaker"),
    ("factory:ride", "Ride"),
    ("factory:crash", "Crash"),
];

/// The state a new channel of an instrument starts with (most start empty and play
/// their default patch).
pub fn default_state(plugin_id: &str) -> Value {
    match plugin_id {
        "lumapad" => {
            let pads: Vec<Value> = DEFAULT_PADS
                .iter()
                .enumerate()
                .map(|(i, (id, name))| {
                    json!({
                        "key": 36 + i,
                        "name": name,
                        "sampleId": id,
                        "level": 0.85,
                        "pan": 0.0,
                        "tune": 0.0,
                        // the hats choke each other
                        "cut": if *id == "factory:hat-closed" || *id == "factory:hat-open" { 1 } else { 0 },
                    })
                })
                .collect();
            json!({ "pads": pads })
        }
        "nebula" => las_dsp::presets::bank("nebula")
            .and_then(|b| b.find("Init").map(|i| b.presets[i].state.clone()))
            .map(|mut s| {
                // the display shows the preset loaded
                s["preset"] = json!("Init");
                s
            })
            .unwrap_or_else(|| json!({})),
        _ => json!({}),
    }
}

pub fn channel(plugin_id: &str) -> Channel {
    let known = INSTRUMENTS.iter().find(|i| i.0 == plugin_id);
    Channel {
        id: uid("ch_"),
        name: known.map_or(plugin_id, |i| i.1).into(),
        color: known.map_or("#7c5cff", |i| i.2).into(),
        kind: match plugin_id {
            "sampler" => "sampler",
            "audioclip" => "audioclip",
            _ => "instrument",
        }
        .into(),
        plugin_id: plugin_id.into(),
        state: default_state(plugin_id),
        volume: 1.0,
        pan: 0.0,
        pitch: 0.0,
        mixer_track: 0,
        mute: false,
        sample_id: None,
        root_key: 60,
        group: None,
        snapshots: None,
        swing: None,
        extra: Extra::new(),
    }
}

pub fn pattern(index: usize, name: Option<&str>) -> Pattern {
    Pattern {
        id: uid("pat_"),
        name: name.map_or_else(|| format!("Pattern {}", index + 1), str::to_string),
        color: palette_color(index).into(),
        length: 0.0,
        ..Pattern::default()
    }
}

pub fn playlist_track(index: usize) -> PlaylistTrack {
    PlaylistTrack {
        id: uid("trk_"),
        name: format!("Track {}", index + 1),
        color: "#2a3145".into(),
        mute: false,
        solo: false,
        height: 56.0,
        extra: Extra::new(),
    }
}

pub fn empty_project(name: &str) -> Project {
    let now = now_ms();
    Project {
        version: PROJECT_VERSION,
        id: uid("prj_"),
        name: name.into(),
        bpm: 128.0,
        time_sig: (4, 4),
        swing: 0.0,
        master_volume: 0.9,
        master_pitch: 0.0,
        channels: Vec::new(),
        patterns: vec![pattern(0, None)],
        playlist: Playlist {
            tracks: (0..24).map(playlist_track).collect(),
            clips: Vec::new(),
        },
        automation: Vec::new(),
        mixer: (0..=MIXER_INSERTS).map(|i| mixer_track(i, None)).collect(),
        samples: Vec::new(),
        meta: ProjectMeta {
            created_at: now,
            updated_at: now,
            ..ProjectMeta::default()
        },
        markers: None,
        midi_map: None,
        extra: Extra::new(),
    }
}

/// A note of the step sequencer: one step long, at `step`.
pub fn step_note(step: usize, key: i32, vel: f64) -> Note {
    Note {
        id: uid("n"),
        key,
        pos: step as f64 * f64::from(TICKS_PER_STEP),
        len: f64::from(TICKS_PER_STEP),
        vel,
        pan: 0.0,
        step: Some(true),
        ..Note::default()
    }
}

/// The "new project" of the web version: four drum channels and a synth, a four on the
/// floor pattern, and the pattern in the playlist.
pub fn default_project() -> Project {
    let mut p = empty_project("Untitled");
    let drums = [
        ("Kick", "factory:kick-punch", "#ff5c8a"),
        ("Clap", "factory:clap-classic", "#ffb547"),
        ("Hat", "factory:hat-closed", "#22d3ee"),
        ("Snare", "factory:snare-tight", "#3ddc97"),
    ];
    for (i, (name, sample, color)) in drums.iter().enumerate() {
        let mut c = channel("sampler");
        c.name = (*name).into();
        c.color = (*color).into();
        c.sample_id = Some((*sample).into());
        c.mixer_track = i + 1;
        p.channels.push(c);
        p.mixer[i + 1].name = (*name).into();
        p.mixer[i + 1].color = (*color).into();
        p.samples.push(SampleRef {
            id: (*sample).into(),
            name: (*name).into(),
            factory: Some(true),
            ..SampleRef::default()
        });
    }
    let mut lead = channel("nebula");
    lead.name = "Nebula".into();
    lead.mixer_track = 5;
    p.channels.push(lead);
    p.mixer[5].name = "Nebula".into();
    p.mixer[5].color = "#c26bff".into();
    p.mixer[0].slots[0] = Some(effect_slot("las-limiter"));
    let ids: Vec<String> = p.channels.iter().map(|c| c.id.clone()).collect();
    let pat = &mut p.patterns[0];
    pat.notes.insert(
        ids[0].clone(),
        [0, 4, 8, 12]
            .iter()
            .map(|&s| step_note(s, 60, 0.78))
            .collect(),
    );
    pat.notes.insert(
        ids[1].clone(),
        [4, 12].iter().map(|&s| step_note(s, 60, 0.78)).collect(),
    );
    pat.notes.insert(
        ids[2].clone(),
        [2, 6, 10, 14]
            .iter()
            .map(|&s| step_note(s, 60, 0.6))
            .collect(),
    );
    let pat_id = pat.id.clone();
    p.playlist.clips.push(Clip {
        id: uid("clip_"),
        kind: ClipKind::Pattern,
        ref_id: pat_id,
        track: 0,
        start: 0.0,
        length: 384.0,
        offset: 0.0,
        ..Clip::default()
    });
    p
}

/// Add a channel with the given instrument to a project: it takes the first insert
/// that no channel uses (or the master's neighbour when all are used), named for it.
pub fn add_channel(p: &mut Project, plugin_id: &str) -> usize {
    let mut c = channel(plugin_id);
    let used: Vec<usize> = p.channels.iter().map(|c| c.mixer_track).collect();
    let insert = (1..p.mixer.len()).find(|i| !used.contains(i)).unwrap_or(1);
    c.mixer_track = insert;
    let same = p
        .channels
        .iter()
        .filter(|x| x.plugin_id == plugin_id)
        .count();
    if same > 0 {
        c.name = format!("{} {}", c.name, same + 1);
    }
    if let Some(t) = p.mixer.get_mut(insert) {
        t.name = c.name.clone();
        t.color = c.color.clone();
    }
    p.channels.push(c);
    p.channels.len() - 1
}

/// The same defaults as JSON, for filling in the gaps of a saved project.
pub(crate) fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

#[cfg(test)]
mod default_tests {
    use super::*;

    #[test]
    fn the_default_project_is_the_web_versions() {
        let p = default_project();
        let names: Vec<_> = p.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Kick", "Clap", "Hat", "Snare", "Nebula"]);
        assert_eq!(p.bpm, 128.0);
        assert_eq!(p.mixer.len(), MIXER_INSERTS + 1);
        assert_eq!(
            p.mixer[0].slots[0].as_ref().unwrap().plugin_id,
            "las-limiter"
        );
        let steps = |i: usize| p.patterns[0].notes[&p.channels[i].id].len();
        assert_eq!((steps(0), steps(1), steps(2)), (4, 2, 4));
        assert!(!p.patterns[0].notes.contains_key(&p.channels[3].id));
        assert_eq!(p.playlist.clips.len(), 1);
        assert_eq!(p.playlist.clips[0].ref_id, p.patterns[0].id);
        // it survives a save and a load
        let bytes = crate::encode(&p, &Default::default(), true).unwrap();
        let back = crate::decode(&bytes).unwrap().project;
        assert_eq!(back.channels.len(), 5);
        assert_eq!(back.samples.len(), 4);
    }

    #[test]
    fn channels_are_added_on_free_inserts_with_numbered_names() {
        let mut p = default_project();
        let i = add_channel(&mut p, "polaris");
        assert_eq!(p.channels[i].mixer_track, 6);
        assert_eq!(p.mixer[6].name, "Polaris");
        let j = add_channel(&mut p, "polaris");
        assert_eq!(p.channels[j].name, "Polaris 2");
        assert_eq!(p.channels[j].mixer_track, 7);
    }
}
