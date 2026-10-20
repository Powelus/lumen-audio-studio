// SPDX-License-Identifier: GPL-3.0-or-later
//! The range a normalised automation value maps onto for an instrument's
//! parameters (the web version's `pluginParamDef`). They match the knobs of the
//! plugin windows, so a clip covers what the knob does.

use las_dsp::fx::registry::ParamDef;
use serde_json::{json, Value};

fn def(id: &str, min: f64, max: f64, default: f64, extra: Value) -> ParamDef {
    let mut v = json!({ "id": id, "label": id, "min": min, "max": max, "default": default });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    serde_json::from_value(v).expect("a parameter definition")
}

fn log() -> Value {
    json!({ "curve": "log" })
}

fn secs(id: &str, max: f64, default: f64) -> ParamDef {
    def(id, 0.001, max, default, log())
}

/// A sampler, audio clip or Nebula control.
fn known(plugin_id: &str, path: &str) -> Option<ParamDef> {
    Some(match (plugin_id, path) {
        ("sampler", "tune") => def(path, -24.0, 24.0, 0.0, json!({ "step": 1 })),
        ("sampler", "fine") => def(path, -100.0, 100.0, 0.0, json!({ "step": 1 })),
        ("sampler", "attack") => secs(path, 5.0, 0.001),
        ("sampler", "decay") => secs(path, 5.0, 0.3),
        ("sampler", "sustain") => def(path, 0.0, 1.0, 1.0, json!({})),
        ("sampler", "release") => secs(path, 8.0, 0.08),
        ("sampler", "cutoff") => def(path, 20.0, 20000.0, 20000.0, log()),
        ("sampler", "res") => def(path, 0.1, 20.0, 0.7, log()),
        ("sampler", "start") => def(path, 0.0, 1.0, 0.0, json!({})),
        ("sampler", "end") => def(path, 0.0, 1.0, 1.0, json!({})),
        ("sampler", "velToVol") => def(path, 0.0, 1.0, 0.7, json!({})),
        ("sampler", "boost") => def(path, -24.0, 12.0, 0.0, json!({})),
        ("audioclip", "gain") => def(path, -24.0, 12.0, 0.0, json!({})),
        ("nebula", "macros.cutoff" | "macros.res" | "macros.attack" | "macros.release") => def(
            path,
            0.0,
            1.0,
            if path.ends_with("res") { 0.0 } else { 0.5 },
            json!({}),
        ),
        ("nebula", "master") => def(path, 0.0, 1.2, 0.8, json!({})),
        _ => return None,
    })
}

fn layer_param(path: &str) -> Option<(usize, &str)> {
    let rest = path.strip_prefix("layers.")?;
    let (i, what) = rest.split_once('.')?;
    Some((i.parse().ok()?, what))
}

/// Facet and Mist: `patch.*` paths.
fn engine_param(path: &str) -> Option<ParamDef> {
    let common = match path {
        "patch.algo" => Some(def(path, 0.0, 7.0, 0.0, json!({ "step": 1 }))),
        "patch.feedback" => Some(def(path, 0.0, 1.0, 0.0, json!({}))),
        "patch.level" => Some(def(path, 0.0, 1.0, 0.7, json!({}))),
        "patch.tune" => Some(def(path, -24.0, 24.0, 0.0, json!({ "step": 1 }))),
        "patch.filter.cutoff" => Some(def(path, 20.0, 20000.0, 18000.0, log())),
        "patch.filter.res" => Some(def(path, 0.0, 1.0, 0.1, json!({}))),
        "patch.filter.envAmt" => Some(def(path, -8.0, 8.0, 0.0, json!({}))),
        "patch.filter.keytrack" => Some(def(path, 0.0, 1.0, 0.0, json!({}))),
        "patch.filter.velAmt" => Some(def(path, -4.0, 4.0, 0.0, json!({}))),
        _ => None,
    };
    if common.is_some() {
        return common;
    }
    if let Some(rest) = path.strip_prefix("patch.ops.") {
        let (i, what) = rest.split_once('.')?;
        i.parse::<u8>().ok().filter(|i| *i < 10)?;
        return Some(match what {
            "ratio" => def(path, 0.125, 32.0, 1.0, log()),
            "freq" => def(path, 1.0, 20000.0, 440.0, log()),
            "detune" => def(path, -100.0, 100.0, 0.0, json!({})),
            "level" => def(path, 0.0, 1.0, 0.5, json!({})),
            "velSens" => def(path, 0.0, 1.0, 0.5, json!({})),
            _ => return None,
        });
    }
    let g = path.strip_prefix("patch.grain.")?;
    Some(match g {
        "position" => def(path, 0.0, 1.0, 0.3, json!({})),
        "scan" => def(path, -1.0, 1.0, 0.0, json!({})),
        "spray" => def(path, 0.0, 0.5, 0.05, json!({ "curve": "pow" })),
        "size" => def(path, 5.0, 500.0, 90.0, log()),
        "density" => def(path, 1.0, 100.0, 24.0, log()),
        "pitch" => def(path, -24.0, 24.0, 0.0, json!({ "step": 1 })),
        "pitchRand" => def(path, 0.0, 12.0, 0.0, json!({ "curve": "pow" })),
        "spread" => def(path, 0.0, 1.0, 0.5, json!({})),
        "shape" => def(path, 0.0, 1.0, 0.2, json!({})),
        "reverse" => def(path, 0.0, 1.0, 0.0, json!({})),
        "rootKey" => def(path, 0.0, 127.0, 60.0, json!({ "step": 1 })),
        _ => return None,
    })
}

/// The ranges of the synth panels' knobs, by the last parts of the path.
fn synth_param(path: &str) -> ParamDef {
    let parts: Vec<&str> = path.split('.').collect();
    let last = parts.last().copied().unwrap_or(path);
    let parent = if parts.len() >= 3 {
        parts[parts.len() - 3]
    } else {
        ""
    };
    if last.contains("cutoff") {
        return def(path, 20.0, 20000.0, 2000.0, log());
    }
    if path == "patch.tune" {
        return def(path, -24.0, 24.0, 0.0, json!({}));
    }
    match last {
        "fine" => def(path, -100.0, 100.0, 0.0, json!({})),
        "semi" => def(path, -12.0, 12.0, 0.0, json!({ "step": 1 })),
        "oct" => def(path, -4.0, 4.0, 0.0, json!({ "step": 1 })),
        "voices" => def(path, 1.0, 16.0, 1.0, json!({ "step": 1 })),
        "poly" => def(path, 1.0, 32.0, 8.0, json!({ "step": 1 })),
        "pbRange" => def(path, 0.0, 24.0, 2.0, json!({ "step": 1 })),
        "pan" | "amt" => def(path, -1.0, 1.0, 0.0, json!({})),
        "envAmt" => def(path, -8.0, 8.0, 0.0, json!({})),
        "velAmt" => def(path, -4.0, 4.0, 0.0, json!({})),
        "a" | "d" => def(path, 0.001, 10.0, 0.1, log()),
        "r" => def(path, 0.001, 15.0, 0.25, log()),
        "h" => def(path, 0.0, 4.0, 0.0, json!({ "curve": "pow" })),
        "glide" => def(path, 0.0, 2.0, 0.0, json!({ "curve": "pow" })),
        "delay" if parent == "lfo" => def(path, 0.0, 4.0, 0.0, json!({ "curve": "pow" })),
        "rate" => def(path, 0.01, 40.0, 2.0, log()),
        _ => def(path, 0.0, 1.0, 0.0, json!({})),
    }
}

/// The definition for an instrument's parameter path.
pub fn plugin_param_def(plugin_id: &str, path: &str) -> ParamDef {
    if let Some(d) = known(plugin_id, path) {
        return d;
    }
    if plugin_id == "nebula" {
        if let Some((_, what)) = layer_param(path) {
            match what {
                "level" => return def(path, 0.0, 1.2, 0.8, json!({})),
                "pan" => return def(path, -1.0, 1.0, 0.0, json!({})),
                "transpose" => return def(path, -24.0, 24.0, 0.0, json!({ "step": 1 })),
                _ => {}
            }
        }
    }
    if plugin_id == "facet" || plugin_id == "mist" {
        if let Some(d) = engine_param(path) {
            return d;
        }
    }
    if plugin_id == "lumapad" {
        if let Some(rest) = path.strip_prefix("pads.") {
            if let Some((_, what)) = rest.split_once('.') {
                match what {
                    "level" => return def(path, 0.0, 1.2, 0.85, json!({})),
                    "pan" => return def(path, -1.0, 1.0, 0.0, json!({})),
                    "tune" => return def(path, -24.0, 24.0, 0.0, json!({ "step": 1 })),
                    _ => {}
                }
            }
        }
    }
    synth_param(path)
}
