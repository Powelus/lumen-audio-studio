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
        state: json!({}),
        volume: 1.0,
        pan: 0.0,
        pitch: 0.0,
        mixer_track: 0,
        mute: false,
        sample_id: None,
        root_key: 60,
        group: None,
        snapshots: None,
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

/// The same defaults as JSON, for filling in the gaps of a saved project.
pub(crate) fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}
