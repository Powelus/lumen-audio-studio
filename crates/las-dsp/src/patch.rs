// SPDX-License-Identifier: GPL-3.0-or-later
//! Patch data shared by the engine-based instruments.
//!
//! Patches are JSON, as in the project format and the preset banks. A preset
//! lists only what differs from the instrument's defaults, and is merged over
//! them with [`merge_patch`] (the same rules as `mergePatch` in the web
//! version: objects merge by key, arrays merge by index).

use serde::Deserialize;
use serde_json::Value;

/// Deep merge of `over` onto `base`. Values in `over` win; objects merge by key
/// and arrays by index (elements past the end of `over` keep the base value).
pub fn merge_patch(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Array(b), Value::Array(o)) => {
            let len = b.len().max(o.len());
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                if i < o.len() {
                    out.push(match b.get(i) {
                        Some(bi) => merge_patch(bi, &o[i]),
                        None => o[i].clone(),
                    });
                } else {
                    out.push(b[i].clone());
                }
            }
            Value::Array(out)
        }
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, v) in o {
                let merged = match b.get(k) {
                    Some(bv) => merge_patch(bv, v),
                    None => v.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        _ => over.clone(),
    }
}

/// Envelope: attack, hold, decay (seconds), sustain (level), release (seconds).
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct EnvPatch {
    pub a: f64,
    pub h: f64,
    pub d: f64,
    pub s: f64,
    pub r: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,
    Ramp,
    Square,
    Sh,
    Smooth,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct LfoPatch {
    pub shape: LfoShape,
    pub rate: f64,
    pub sync: bool,
    pub div: f64,
    pub retrig: bool,
    pub phase: f64,
    pub delay: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModSlot {
    pub src: String,
    pub dest: String,
    pub amt: f64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct VoicePatch {
    pub poly: f64,
    pub mono: bool,
    pub legato: bool,
    pub glide: f64,
    #[serde(rename = "pbRange")]
    pub pb_range: f64,
    #[serde(rename = "velSens")]
    pub vel_sens: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterKind {
    Lp,
    Bp,
    Hp,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct EngineFilter {
    pub on: bool,
    #[serde(rename = "type")]
    pub kind: FilterKind,
    pub cutoff: f64,
    pub res: f64,
    /// Modulation envelope amount, octaves.
    #[serde(rename = "envAmt")]
    pub env_amt: f64,
    /// Keyboard tracking, 0 to 1.
    pub keytrack: f64,
    /// Velocity amount, octaves.
    #[serde(rename = "velAmt")]
    pub vel_amt: f64,
}

/// What every engine-based instrument has in common.
#[derive(Clone, Debug, Deserialize)]
pub struct EnginePatch {
    /// `env[0]` is the amp envelope, `env[1]` the modulation envelope ("env2").
    pub env: Vec<EnvPatch>,
    pub lfo: Vec<LfoPatch>,
    #[serde(rename = "mod", default)]
    pub mods: Vec<ModSlot>,
    pub macros: Vec<f64>,
    pub voice: VoicePatch,
    pub filter: EngineFilter,
    pub level: f64,
    pub tune: f64,
}

/// Defaults of the common part, as JSON (instruments extend and override them).
pub fn default_engine_json() -> Value {
    serde_json::json!({
        "env": [
            { "a": 0.003, "h": 0, "d": 0.3, "s": 1, "r": 0.3 },
            { "a": 0.01, "h": 0, "d": 0.6, "s": 0, "r": 0.3 },
        ],
        "lfo": [
            { "shape": "sine", "rate": 4, "sync": false, "div": 1, "retrig": false, "phase": 0, "delay": 0 },
            { "shape": "triangle", "rate": 0.25, "sync": false, "div": 4, "retrig": false, "phase": 0, "delay": 0 },
        ],
        "mod": [],
        "macros": [0, 0, 0, 0],
        "voice": { "poly": 8, "mono": false, "legato": false, "glide": 0, "pbRange": 2, "velSens": 0.6 },
        "filter": { "on": false, "type": "lp", "cutoff": 18000, "res": 0.1, "envAmt": 0, "keytrack": 0, "velAmt": 0 },
        "level": 0.7,
        "tune": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn objects_merge_by_key_and_arrays_by_index() {
        let base = json!({ "a": 1, "b": { "x": 1, "y": 2 }, "c": [1, 2, 3] });
        let over = json!({ "b": { "y": 9 }, "c": [7], "d": 4 });
        let out = merge_patch(&base, &over);
        assert_eq!(
            out,
            json!({ "a": 1, "b": { "x": 1, "y": 9 }, "c": [7, 2, 3], "d": 4 })
        );
    }

    #[test]
    fn array_elements_merge_as_objects() {
        let base = json!([{ "a": 1, "b": 2 }, { "a": 3, "b": 4 }]);
        let over = json!([{ "b": 9 }]);
        assert_eq!(
            merge_patch(&base, &over),
            json!([{ "a": 1, "b": 9 }, { "a": 3, "b": 4 }])
        );
    }

    #[test]
    fn the_defaults_parse() {
        let p: EnginePatch = serde_json::from_value(default_engine_json()).expect("defaults parse");
        assert_eq!(p.env.len(), 2);
        assert_eq!(p.lfo.len(), 2);
        assert_eq!(p.voice.poly, 8.0);
    }
}

/// Set `path` (dotted, array indices as numbers) in `v` if every step but the last
/// exists, as the web version's `setPath` does.
pub fn set_path(v: &mut Value, path: &str, value: Value) {
    fn child<'a>(v: &'a mut Value, key: &str) -> Option<&'a mut Value> {
        match v {
            Value::Array(a) => key.parse::<usize>().ok().and_then(move |i| a.get_mut(i)),
            Value::Object(o) => o.get_mut(key),
            _ => None,
        }
    }
    let parts: Vec<&str> = path.split('.').collect();
    let Some((last, init)) = parts.split_last() else {
        return;
    };
    let mut cur = v;
    for part in init {
        match child(cur, part) {
            Some(n) if !n.is_null() => cur = n,
            _ => return,
        }
    }
    match cur {
        Value::Object(o) => {
            o.insert((*last).to_string(), value);
        }
        Value::Array(_) => {
            if let Some(slot) = child(cur, last) {
                *slot = value;
            }
        }
        _ => {}
    }
}

/// Timed parameter ramps for a synth, one line per patch path, read at the start of
/// each control block (the web version's `ramp` messages).
#[derive(Default)]
pub struct Ramps {
    lines: std::collections::BTreeMap<String, crate::param::Timeline>,
    last: std::collections::HashMap<String, f64>,
}

impl Ramps {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Merge points (time, value) into a path's line; `jump` steps to the first.
    pub fn ramp(&mut self, path: &str, points: &[(f64, f64)], jump: bool) {
        self.lines
            .entry(path.to_string())
            .or_default()
            .set(points, jump);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.last.clear();
    }

    /// Re-apply the automated values over a new patch.
    pub fn forget(&mut self) {
        self.last.clear();
    }

    /// The paths whose value at `time` differs from the one last applied.
    pub fn due(&mut self, time: f64) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        for (path, line) in &self.lines {
            let Some(v) = line.value_at(time) else {
                continue;
            };
            if self.last.get(path) == Some(&v) {
                continue;
            }
            self.last.insert(path.clone(), v);
            out.push((path.clone(), v));
        }
        out
    }
}
