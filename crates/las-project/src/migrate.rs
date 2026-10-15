// SPDX-License-Identifier: GPL-3.0-or-later
//! Project migrations and repair.
//!
//! Every project that enters the app goes through [`migrate_project`]:
//!
//! 1. versioned steps bring older data to [`PROJECT_VERSION`],
//! 2. [`normalize`] fills in anything missing,
//! 3. [`sanitize`] repairs values the engine cannot handle and reports each
//!    repair as a warning.
//!
//! This is a port of the web version's `core/migrate.ts` and works on the JSON
//! (as the original does), so damaged data can be repaired rather than refused.
//! Valid projects pass through unchanged and unknown fields are kept.

use crate::create::{self, instrument_known, uid};
use crate::model::*;
use las_dsp::fx::registry::descriptor;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

type Json = Map<String, Value>;

#[derive(Debug)]
pub struct ProjectError(pub String);

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProjectError {}

pub struct Migrated {
    pub project: Project,
    /// What was repaired, in the order it was found.
    pub warnings: Vec<String>,
    /// The version the data was saved as, when it needed migrating.
    pub migrated_from: Option<u32>,
}

// ---------------------------------------------------------------------------
// Versioned steps. `migrate_step(n)` upgrades data saved as version n to
// n + 1. Never edit a released step; add one and bump PROJECT_VERSION.
// ---------------------------------------------------------------------------
const V1_PLUGIN_IDS: &[(&str, &str)] = &[
    ("powpad", "lumapad"),
    ("pow-eq", "las-eq"),
    ("pow-comp", "las-comp"),
    ("pow-ducker", "las-ducker"),
    ("pow-shaper", "las-shaper"),
    ("pow-gate", "las-gate"),
    ("pow-limiter", "las-limiter"),
    ("pow-maximizer", "las-maximizer"),
    ("pow-reverb", "las-reverb"),
    ("pow-delay", "las-delay"),
    ("pow-dist", "las-dist"),
    ("pow-crusher", "las-crusher"),
    ("pow-chorus", "las-chorus"),
    ("pow-phaser", "las-phaser"),
    ("pow-filter", "las-filter"),
    ("pow-stereo", "las-stereo"),
    ("pow-scope", "las-scope"),
];

/// Rename plugin ids wherever a plugin is referenced (`pluginId`, rack `type`).
fn rename_plugin_ids(node: &mut Value, map: &[(&str, &str)]) {
    match node {
        Value::Array(a) => a.iter_mut().for_each(|x| rename_plugin_ids(x, map)),
        Value::Object(o) => {
            for (k, v) in o.iter_mut() {
                if k == "pluginId" || k == "type" {
                    if let Some(to) = v
                        .as_str()
                        .and_then(|s| map.iter().find(|m| m.0 == s).map(|m| m.1))
                    {
                        *v = json!(to);
                        continue;
                    }
                }
                rename_plugin_ids(v, map);
            }
        }
        _ => {}
    }
}

fn migrate_step(version: u32, raw: &mut Value) {
    if version == 1 {
        // plugins were renamed when the app became Lumen Audio Studio
        rename_plugin_ids(raw, V1_PLUGIN_IDS);
    }
}

// ---------------------------------------------------------------------------
// Small helpers with the semantics of the original's JavaScript
// ---------------------------------------------------------------------------
fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn integer(v: Option<&Value>) -> Option<i64> {
    let x = v.and_then(Value::as_f64)?;
    (x.is_finite() && x.fract() == 0.0).then_some(x as i64)
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(v))
}

fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

/// `Math.round`.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn is_obj(v: Option<&Value>) -> bool {
    v.is_some_and(Value::is_object)
}

fn str_of(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

/// Repairs are reported once per message, with a count for repeats.
#[derive(Default)]
struct Report(Vec<(String, usize)>);

impl Report {
    fn add(&mut self, message: impl Into<String>) {
        let message = message.into();
        match self.0.iter_mut().find(|(m, _)| *m == message) {
            Some((_, n)) => *n += 1,
            None => self.0.push((message, 1)),
        }
    }

    fn finish(self) -> Vec<String> {
        self.0
            .into_iter()
            .map(|(m, n)| if n > 1 { format!("{m} ({n} times)") } else { m })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Normalising
// ---------------------------------------------------------------------------
fn merged(base: Value, over: Option<&Value>) -> Json {
    let mut out = base.as_object().cloned().unwrap_or_default();
    if let Some(Value::Object(o)) = over {
        for (k, v) in o {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn non_empty_array(v: Option<&Value>) -> Option<&Vec<Value>> {
    v.and_then(Value::as_array).filter(|a| !a.is_empty())
}

/// Give a saved project every field the app expects.
pub fn normalize(raw: &Json) -> Json {
    let name = match raw.get("name") {
        Some(Value::Null) | None => "Untitled".to_string(),
        Some(v) => v.as_str().unwrap_or("Untitled").to_string(),
    };
    let base = create::to_value(&create::empty_project(&name));
    let base_meta = base["meta"].clone();
    let mut p = merged(base.clone(), Some(&Value::Object(raw.clone())));
    p.insert(
        "meta".into(),
        Value::Object(merged(base_meta, raw.get("meta"))),
    );

    let playlist = raw.get("playlist");
    let tracks = match non_empty_array(playlist.and_then(|p| p.get("tracks"))) {
        Some(t) => Value::Array(t.clone()),
        None => base["playlist"]["tracks"].clone(),
    };
    let clips = playlist
        .and_then(|p| p.get("clips"))
        .filter(|c| !c.is_null())
        .cloned()
        .unwrap_or_else(|| json!([]));
    p.insert(
        "playlist".into(),
        json!({ "tracks": tracks, "clips": clips }),
    );

    let mixer: Vec<Value> = match non_empty_array(raw.get("mixer")) {
        Some(m) => m.clone(),
        None => base["mixer"].as_array().cloned().unwrap_or_default(),
    };
    let mixer = mixer
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut track = merged(create::to_value(&create::mixer_track(i, None)), Some(t));
            let slots: Vec<Value> = (0..MIXER_SLOTS)
                .map(|s| {
                    t.get("slots")
                        .and_then(|a| a.get(s))
                        .filter(|v| !v.is_null())
                        .cloned()
                        .unwrap_or(Value::Null)
                })
                .collect();
            track.insert("slots".into(), Value::Array(slots));
            let sends = t
                .get("sends")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(|| json!([]));
            track.insert("sends".into(), sends);
            Value::Object(track)
        })
        .collect();
    p.insert("mixer".into(), Value::Array(mixer));

    let channels: Vec<Value> = raw
        .get("channels")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|c| {
                    let plugin = str_of(c.get("pluginId")).unwrap_or("triosc");
                    Value::Object(merged(create::to_value(&create::channel(plugin)), Some(c)))
                })
                .collect()
        })
        .unwrap_or_default();
    p.insert("channels".into(), Value::Array(channels));

    let patterns = match non_empty_array(raw.get("patterns")) {
        Some(list) => list
            .iter()
            .map(|x| {
                let mut o = x.as_object().cloned().unwrap_or_default();
                if o.get("notes").is_none_or(Value::is_null) {
                    o.insert("notes".into(), json!({}));
                }
                if o.get("length").is_none_or(Value::is_null) {
                    o.insert("length".into(), json!(0));
                }
                Value::Object(o)
            })
            .collect(),
        None => base["patterns"].as_array().cloned().unwrap_or_default(),
    };
    p.insert("patterns".into(), Value::Array(patterns));
    for key in ["automation", "samples"] {
        let list = raw
            .get(key)
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| json!([]));
        p.insert(key.into(), list);
    }
    p
}

// ---------------------------------------------------------------------------
// Sanitising
// ---------------------------------------------------------------------------
fn get_arr<'a>(o: &'a mut Json, key: &str) -> &'a mut Vec<Value> {
    if !o.get(key).is_some_and(Value::is_array) {
        o.insert(key.into(), json!([]));
    }
    o.get_mut(key)
        .and_then(Value::as_array_mut)
        .expect("just made an array")
}

fn set_num(o: &mut Json, key: &str, v: f64) {
    o.insert(key.into(), num(v));
}

/// A number clamped, or `default` when it is not a finite number.
fn clamped_or(o: &mut Json, key: &str, lo: f64, hi: f64, default: f64) {
    let v = finite(o.get(key)).map_or(default, |x| clamp(x, lo, hi));
    set_num(o, key, v);
}

/// An id that is unique among those seen, or a new one.
fn unique_id(o: &mut Json, seen: &mut HashSet<String>, prefix: &str) {
    let current = str_of(o.get("id")).map(str::to_string);
    let id = match current {
        Some(id) if !seen.contains(&id) => id,
        _ => uid(prefix),
    };
    seen.insert(id.clone());
    o.insert("id".into(), json!(id));
}

fn sanitize(p: &mut Json, r: &mut Report) {
    // global values
    match finite(p.get("bpm")) {
        Some(b) if (20.0..=999.0).contains(&b) => {}
        other => {
            let shown = match other {
                Some(b) => num(b).to_string(),
                None => p.get("bpm").map_or("undefined".into(), |v| match v {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                }),
            };
            r.add(format!("Tempo {shown} is out of range; using 128 BPM"));
            p.insert("bpm".into(), json!(128));
        }
    }
    let sig_ok = p.get("timeSig").and_then(Value::as_array).is_some_and(|s| {
        let a = finite(s.first());
        let b = finite(s.get(1));
        matches!((a, b), (Some(a), Some(b))
                if (1.0..=32.0).contains(&a) && [1.0, 2.0, 4.0, 8.0, 16.0, 32.0].contains(&b))
    });
    if !sig_ok {
        r.add("Time signature was invalid; using 4/4");
        p.insert("timeSig".into(), json!([4, 4]));
    }
    clamped_or(p, "swing", 0.0, 1.0, 0.0);
    clamped_or(p, "masterVolume", 0.0, 1.25, 0.9);
    if finite(p.get("masterPitch")).is_none() {
        p.insert("masterPitch".into(), json!(0));
    }
    if !p.get("name").is_some_and(Value::is_string) {
        p.insert("name".into(), json!("Untitled"));
    }

    // mixer
    let tracks = get_arr(p, "mixer").len();
    let mut slot_ids = HashSet::new();
    let effect_known = |id: &str| descriptor(id).is_some();
    for (i, t) in get_arr(p, "mixer").iter_mut().enumerate() {
        let Some(t) = t.as_object_mut() else {
            continue;
        };
        clamped_or(t, "volume", 0.0, 1.25, 1.0);
        clamped_or(t, "pan", -1.0, 1.0, 0.0);
        clamped_or(t, "width", 0.0, 2.0, 1.0);
        if i == 0 {
            t.insert("output".into(), Value::Null);
        } else if !t.get("output").is_some_and(Value::is_null) {
            let ok = integer(t.get("output"))
                .is_some_and(|o| o >= 0 && (o as usize) < tracks && o as usize != i);
            if !ok {
                r.add(format!(
                    "Insert {i} had an invalid output; routed to master"
                ));
                t.insert("output".into(), json!(0));
            }
        }
        let sends = std::mem::take(get_arr(t, "sends"));
        let sends: Vec<Value> = sends
            .into_iter()
            .filter(|s| {
                let ok = is_obj(Some(s))
                    && integer(s.get("target"))
                        .is_some_and(|x| x >= 0 && (x as usize) < tracks && x as usize != i)
                    && finite(s.get("amount")).is_some();
                if !ok {
                    r.add("Removed a send with an invalid destination");
                }
                ok
            })
            .collect();
        t.insert("sends".into(), Value::Array(sends));
        let slots = std::mem::take(get_arr(t, "slots"));
        let slots: Vec<Value> = slots
            .into_iter()
            .take(MIXER_SLOTS)
            .map(|s| {
                if s.is_null() {
                    return Value::Null;
                }
                let plugin = str_of(s.get("pluginId")).map(str::to_string);
                let (Some(plugin), Some(mut s)) = (plugin, s.as_object().cloned()) else {
                    r.add("Removed an unreadable effect slot");
                    return Value::Null;
                };
                unique_id(&mut s, &mut slot_ids, "fx_");
                if !is_obj(s.get("state")) {
                    s.insert("state".into(), json!({}));
                }
                clamped_or(&mut s, "mix", 0.0, 1.0, 1.0);
                let enabled = s.get("enabled") != Some(&Value::Bool(false));
                s.insert("enabled".into(), json!(enabled));
                if !effect_known(&plugin) {
                    r.add(format!(
                        "Effect \"{plugin}\" is not available in this version and will pass audio through"
                    ));
                }
                Value::Object(s)
            })
            .collect();
        t.insert("slots".into(), Value::Array(slots));
    }
    break_routing_cycles(p, r);

    // channels
    let mut channel_ids = HashSet::new();
    for c in get_arr(p, "channels").iter_mut() {
        let Some(c) = c.as_object_mut() else {
            continue;
        };
        unique_id(c, &mut channel_ids, "ch_");
        if !is_obj(c.get("state")) {
            c.insert("state".into(), json!({}));
        }
        clamped_or(c, "volume", 0.0, 1.25, 1.0);
        clamped_or(c, "pan", -1.0, 1.0, 0.0);
        clamped_or(c, "pitch", -48.0, 48.0, 0.0);
        let track =
            integer(c.get("mixerTrack")).map_or(0.0, |t| clamp(t as f64, 0.0, tracks as f64 - 1.0));
        set_num(c, "mixerTrack", track);
        if let Some(snaps) = c.get("snapshots").cloned() {
            let list = snaps.as_array().cloned().unwrap_or_default();
            let good: Vec<Value> = list
                .iter()
                .filter(|x| str_of(x.get("name")).is_some() && is_obj(x.get("state")))
                .cloned()
                .collect();
            if !snaps.is_array() || good.len() != list.len() {
                r.add("Removed unreadable pinned sounds");
            }
            let fixed: Vec<Value> = good
                .into_iter()
                .map(|x| {
                    json!({
                        "id": str_of(x.get("id")).map_or_else(|| uid("pin_"), str::to_string),
                        "name": x["name"],
                        "time": finite(x.get("time")).unwrap_or(0.0),
                        "state": x["state"],
                    })
                })
                .collect();
            c.insert("snapshots".into(), Value::Array(fixed));
        }
        let plugin = str_of(c.get("pluginId")).unwrap_or("undefined");
        if !instrument_known(plugin) {
            r.add(format!(
                "Instrument \"{plugin}\" is not available in this version; its channels will play with TriOsc"
            ));
        }
    }

    // reference-track analysis (optional)
    if let Some(Value::Object(meta)) = p.get_mut("meta") {
        if let Some(reference) = meta.get("reference") {
            if !reference_ok(reference) {
                meta.remove("reference");
                r.add("Removed an unreadable reference-track analysis");
            }
        }
    }

    // patterns and notes
    let mut pattern_ids = HashSet::new();
    for pat in get_arr(p, "patterns").iter_mut() {
        let Some(pat) = pat.as_object_mut() else {
            continue;
        };
        unique_id(pat, &mut pattern_ids, "pat_");
        if !is_obj(pat.get("notes")) {
            pat.insert("notes".into(), json!({}));
        }
        match finite(pat.get("length")) {
            Some(l) if l >= 0.0 => {}
            _ => {
                pat.insert("length".into(), json!(0));
            }
        }
        if let Some(v) = pat.get("variation") {
            if !variation_ok(v) {
                pat.remove("variation");
                r.add("Removed unreadable pattern variation settings");
            }
        }
        let mut note_ids = HashSet::new();
        let Some(Value::Object(notes)) = pat.get_mut("notes") else {
            continue;
        };
        let channels: Vec<String> = notes.keys().cloned().collect();
        for ch in channels {
            let Some(Value::Array(list)) = notes.get(&ch).cloned() else {
                notes.remove(&ch);
                r.add("Removed an unreadable note list");
                continue;
            };
            let mut kept = Vec::new();
            for n in list {
                let ok = is_obj(Some(&n))
                    && finite(n.get("key")).is_some()
                    && finite(n.get("pos")).is_some_and(|x| x >= 0.0)
                    && finite(n.get("len")).is_some_and(|x| x > 0.0);
                if !ok {
                    r.add("Removed notes with invalid timing or pitch");
                    continue;
                }
                let mut n = n.as_object().cloned().unwrap_or_default();
                unique_id(&mut n, &mut note_ids, "n");
                let key = clamp(js_round(finite(n.get("key")).unwrap_or(60.0)), 0.0, 127.0);
                set_num(&mut n, "key", key);
                clamped_or(&mut n, "vel", 0.0, 1.0, 0.78);
                clamped_or(&mut n, "pan", -1.0, 1.0, 0.0);
                kept.push(Value::Object(n));
            }
            notes.insert(ch, Value::Array(kept));
        }
    }

    // automation
    let mut auto_ids = HashSet::new();
    let mixer_slot_ids: Vec<Vec<String>> = get_arr(p, "mixer")
        .iter()
        .map(|t| {
            t.get("slots")
                .and_then(Value::as_array)
                .map(|s| {
                    s.iter()
                        .filter_map(|x| str_of(x.get("id")).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let mut automation = std::mem::take(get_arr(p, "automation"));
    for a in automation.iter_mut() {
        let Some(a) = a.as_object_mut() else {
            continue;
        };
        unique_id(a, &mut auto_ids, "auto_");
        let mut points: Vec<Value> = a
            .get("points")
            .and_then(Value::as_array)
            .map(|pts| {
                pts.iter()
                    .filter(|pt| {
                        is_obj(Some(pt))
                            && finite(pt.get("t")).is_some()
                            && finite(pt.get("v")).is_some()
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for pt in points.iter_mut() {
            if let Some(o) = pt.as_object_mut() {
                clamped_or(o, "v", 0.0, 1.0, 0.0);
            }
        }
        points.sort_by(|x, y| {
            finite(x.get("t"))
                .partial_cmp(&finite(y.get("t")))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let last_t = points
            .last()
            .and_then(|pt| finite(pt.get("t")))
            .unwrap_or(0.0);
        a.insert("points".into(), Value::Array(points));
        if finite(a.get("length")).is_none_or(|l| l <= 0.0) {
            set_num(a, "length", 384f64.max(last_t));
        }
        let target = a.get("target").cloned().unwrap_or(Value::Null);
        let dangling = match target.as_object() {
            None => true,
            Some(t) => {
                let channel_gone =
                    || !str_of(t.get("channelId")).is_some_and(|c| channel_ids.contains(c));
                let track = finite(t.get("track"));
                match t.get("kind").and_then(Value::as_str) {
                    Some("channel" | "plugin") => channel_gone(),
                    Some("mixer") => !track.is_some_and(|x| x >= 0.0 && x < tracks as f64),
                    Some("effect") => {
                        let slot = str_of(t.get("slotId"));
                        !track
                            .filter(|x| x.fract() == 0.0 && *x >= 0.0)
                            .and_then(|x| mixer_slot_ids.get(x as usize))
                            .is_some_and(|ids| slot.is_some_and(|s| ids.iter().any(|i| i == s)))
                    }
                    _ => false,
                }
            }
        };
        if dangling {
            let name = match a.get("name") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => "undefined".into(),
            };
            r.add(format!(
                "Automation \"{name}\" points at something that no longer exists and will do nothing"
            ));
        }
    }
    p.insert("automation".into(), Value::Array(automation));

    // playlist
    let track_count = p
        .get("playlist")
        .and_then(|pl| pl.get("tracks"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let pattern_set: HashSet<String> = get_arr(p, "patterns")
        .iter()
        .filter_map(|x| str_of(x.get("id")).map(str::to_string))
        .collect();
    if let Some(Value::Object(playlist)) = p.get_mut("playlist") {
        let clips = std::mem::take(get_arr(playlist, "clips"));
        let mut kept = Vec::new();
        let mut clip_ids = HashSet::new();
        for c in clips {
            let ok = is_obj(Some(&c))
                && matches!(
                    str_of(c.get("kind")),
                    Some("pattern" | "audio" | "automation")
                )
                && finite(c.get("start")).is_some_and(|x| x >= 0.0)
                && finite(c.get("length")).is_some_and(|x| x > 0.0);
            if !ok {
                r.add("Removed playlist clips with invalid timing");
                continue;
            }
            kept.push(c);
        }
        for c in kept.iter_mut() {
            let Some(c) = c.as_object_mut() else {
                continue;
            };
            unique_id(c, &mut clip_ids, "clip_");
            if finite(c.get("offset")).is_none_or(|o| o < 0.0) {
                c.insert("offset".into(), json!(0));
            }
            let track = integer(c.get("track"))
                .map_or(0.0, |t| clamp(t as f64, 0.0, track_count as f64 - 1.0));
            set_num(c, "track", track);
            if let Some(take) = c.get("take") {
                let ok = str_of(take.get("group")).is_some() && finite(take.get("index")).is_some();
                if !ok {
                    c.remove("take");
                    r.add("Removed unreadable take information from a clip");
                }
            }
            if let Some(audio) = c.get("audio").cloned() {
                let (edit, ok) = clean_audio_edit(&audio);
                if !ok {
                    r.add("Repaired unreadable audio clip settings");
                }
                match edit {
                    Some(e) => c.insert("audio".into(), e),
                    None => c.remove("audio"),
                };
            }
            let kind = str_of(c.get("kind")).unwrap_or("pattern").to_string();
            let target = str_of(c.get("refId"));
            let exists = target.is_some_and(|t| match kind.as_str() {
                "pattern" => pattern_set.contains(t),
                "audio" => channel_ids.contains(t),
                _ => auto_ids.contains(t),
            });
            if !exists {
                r.add(format!(
                    "A {kind} clip refers to something that no longer exists and will be silent"
                ));
            }
        }
        playlist.insert("clips".into(), Value::Array(kept));
    }

    // markers
    if p.contains_key("markers") {
        let value = p.remove("markers").unwrap_or(Value::Null);
        let list = value.as_array().cloned().unwrap_or_default();
        let mut good: Vec<Value> = list
            .iter()
            .filter(|m| is_obj(Some(m)) && finite(m.get("tick")).is_some_and(|t| t >= 0.0))
            .cloned()
            .collect();
        if good.len() != list.len() || !value.is_array() {
            r.add("Removed unreadable song markers");
        }
        let mut ids = HashSet::new();
        for m in good.iter_mut() {
            let Some(m) = m.as_object_mut() else {
                continue;
            };
            unique_id(m, &mut ids, "mk_");
            let name: String = match m.get("name") {
                Some(Value::String(s)) => s.chars().take(80).collect(),
                _ => "Marker".into(),
            };
            m.insert("name".into(), json!(name));
            if m.get("color").is_some_and(|c| !c.is_string()) {
                m.remove("color");
            }
        }
        good.sort_by(|a, b| {
            finite(a.get("tick"))
                .partial_cmp(&finite(b.get("tick")))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if !good.is_empty() {
            p.insert("markers".into(), Value::Array(good));
        }
    }

    // MIDI controller assignments
    if p.contains_key("midiMap") {
        let value = p.remove("midiMap").unwrap_or(Value::Null);
        let list = value.as_array().cloned().unwrap_or_default();
        let mut good: Vec<Value> = list
            .iter()
            .filter(|m| {
                let cc = integer(m.get("cc")).is_some_and(|c| (0..=127).contains(&c));
                let ch = integer(m.get("channel")).is_some_and(|c| (0..=15).contains(&c));
                let Some(t) = m.get("target").and_then(Value::as_object) else {
                    return false;
                };
                if !(cc && ch) {
                    return false;
                }
                let track = finite(t.get("track"));
                match str_of(t.get("kind")) {
                    Some("channel" | "plugin") => {
                        str_of(t.get("channelId")).is_some_and(|c| channel_ids.contains(c))
                    }
                    Some("mixer") => track.is_some_and(|x| x >= 0.0 && x < tracks as f64),
                    Some("effect") => track
                        .filter(|x| x.fract() == 0.0 && *x >= 0.0)
                        .and_then(|x| mixer_slot_ids.get(x as usize))
                        .is_some_and(|ids| {
                            str_of(t.get("slotId")).is_some_and(|s| ids.iter().any(|i| i == s))
                        }),
                    Some("master") => true,
                    _ => false,
                }
            })
            .cloned()
            .collect();
        if good.len() != list.len() {
            r.add("Removed MIDI controller assignments that point at missing controls");
        }
        for m in good.iter_mut() {
            if let Some(m) = m.as_object_mut() {
                if !m.get("id").is_some_and(Value::is_string) {
                    m.insert("id".into(), json!(uid("midi_")));
                }
            }
        }
        if !good.is_empty() {
            p.insert("midiMap".into(), Value::Array(good));
        }
    }

    // samples
    let mut seen = HashSet::new();
    let samples = std::mem::take(get_arr(p, "samples"));
    let samples: Vec<Value> = samples
        .into_iter()
        .filter(|s| match (is_obj(Some(s)), str_of(s.get("id"))) {
            (true, Some(id)) => seen.insert(id.to_string()),
            _ => false,
        })
        .collect();
    p.insert("samples".into(), Value::Array(samples));
}

fn reference_ok(v: &Value) -> bool {
    let bands_ok = v
        .get("bands")
        .and_then(Value::as_array)
        .is_some_and(|b| b.len() == 30 && b.iter().all(|x| finite(Some(x)).is_some()));
    let loudness_ok = v.get("loudness").is_some_and(|l| {
        [
            "integrated",
            "range",
            "maxMomentary",
            "maxShortTerm",
            "samplePeak",
            "truePeak",
        ]
        .iter()
        .all(|k| finite(l.get(k)).is_some())
    });
    v.is_object()
        && v.get("name").is_some_and(Value::is_string)
        && bands_ok
        && finite(v.get("widthDb")).is_some()
        && finite(v.get("duration")).is_some()
        && loudness_ok
}

fn variation_ok(v: &Value) -> bool {
    let Some(o) = v.get("options") else {
        return false;
    };
    let numbers = [
        "seed", "timing", "velocity", "density", "pitch", "swing", "rotate",
    ]
    .iter()
    .all(|k| finite(o.get(k)).is_some());
    v.is_object()
        && v.get("of").is_some_and(Value::is_string)
        && o.is_object()
        && numbers
        && o.get("channels").is_some_and(Value::is_array)
}

/// Break feedback loops in the insert routing (they would silence the output).
fn break_routing_cycles(p: &mut Json, r: &mut Report) {
    fn edges(mixer: &[Value], i: usize) -> Vec<usize> {
        let t = &mixer[i];
        let mut out = Vec::new();
        if let Some(o) = integer(t.get("output")) {
            out.push(o as usize);
        }
        if let Some(sends) = t.get("sends").and_then(Value::as_array) {
            for s in sends {
                if s.get("sidechainOnly") != Some(&Value::Bool(true)) {
                    if let Some(x) = integer(s.get("target")) {
                        out.push(x as usize);
                    }
                }
            }
        }
        out
    }

    fn visit(mixer: &mut Vec<Value>, i: usize, state: &mut Vec<u8>, r: &mut Report) {
        state[i] = 1;
        for to in edges(mixer, i) {
            if to >= state.len() {
                continue;
            }
            if state[to] == 1 {
                // a back edge: cut it
                if let Some(t) = mixer[i].as_object_mut() {
                    if integer(t.get("output")) == Some(to as i64) {
                        t.insert("output".into(), json!(0));
                    }
                    if let Some(Value::Array(sends)) = t.get_mut("sends") {
                        sends.retain(|s| {
                            s.get("sidechainOnly") == Some(&Value::Bool(true))
                                || integer(s.get("target")) != Some(to as i64)
                        });
                    }
                }
                r.add("Removed a mixer routing loop");
            } else if state[to] == 0 {
                visit(mixer, to, state, r);
            }
        }
        state[i] = 2;
    }

    let Some(Value::Array(mixer)) = p.get_mut("mixer") else {
        return;
    };
    let mut state = vec![0u8; mixer.len()];
    for i in 0..mixer.len() {
        if state[i] == 0 {
            visit(mixer, i, &mut state, r);
        }
    }
}

/// Validate an audio clip's edits: drop unknown or out-of-range values.
fn clean_audio_edit(input: &Value) -> (Option<Value>, bool) {
    let Some(a) = input.as_object() else {
        return (None, false);
    };
    let mut out = Json::new();
    let mut ok = true;
    let mut number = |k: &str, lo: f64, hi: f64| {
        if let Some(v) = a.get(k) {
            match finite(Some(v)) {
                Some(x) => {
                    out.insert(k.into(), num(clamp(x, lo, hi)));
                }
                None => ok = false,
            }
        }
    };
    number("gain", -60.0, 24.0);
    number("fadeIn", 0.0, 1e9);
    number("fadeOut", 0.0, 1e9);
    number("fadeInCurve", -1.0, 1.0);
    number("fadeOutCurve", -1.0, 1.0);
    number("pitch", -24.0, 24.0);
    if let Some(v) = a.get("reverse") {
        match v.as_bool() {
            Some(b) => {
                out.insert("reverse".into(), json!(b));
            }
            None => ok = false,
        }
    }
    if let Some(st) = a.get("stretch") {
        let bpm = finite(st.get("bpm")).filter(|b| *b > 0.0);
        let mode = str_of(st.get("mode")).filter(|m| *m == "stretch" || *m == "resample");
        match (bpm, mode) {
            (Some(bpm), Some(mode)) => {
                out.insert(
                    "stretch".into(),
                    json!({ "bpm": num(clamp(bpm, 20.0, 400.0)), "mode": mode }),
                );
            }
            _ => ok = false,
        }
    }
    let edit = (!out.is_empty()).then_some(Value::Object(out));
    (edit, ok)
}

// ---------------------------------------------------------------------------

/// Bring saved project JSON to the current version and repair it.
pub fn migrate_project(input: &Value) -> Result<Migrated, ProjectError> {
    let Some(map) = input.as_object() else {
        return Err(ProjectError("The project data is not valid".into()));
    };
    let mut raw = Value::Object(map.clone());
    let stored = finite(raw.get("version"))
        .filter(|v| *v >= 1.0)
        .map_or(1, |v| v.floor() as u32);
    if stored > PROJECT_VERSION {
        return Err(ProjectError(format!(
            "This project was saved by a newer version of Lumen Audio Studio (project version {stored}, this app supports {PROJECT_VERSION}). Update the app to open it."
        )));
    }
    for v in stored..PROJECT_VERSION {
        migrate_step(v, &mut raw);
    }
    if let Some(o) = raw.as_object_mut() {
        o.insert("version".into(), json!(PROJECT_VERSION));
    }
    let mut project = normalize(raw.as_object().expect("an object"));
    let mut report = Report::default();
    sanitize(&mut project, &mut report);
    let project: Project = serde_json::from_value(Value::Object(project)).map_err(|e| {
        ProjectError(format!(
            "The project data is damaged and could not be read ({e})"
        ))
    })?;
    Ok(Migrated {
        project,
        warnings: report.finish(),
        migrated_from: (stored < PROJECT_VERSION).then_some(stored),
    })
}
