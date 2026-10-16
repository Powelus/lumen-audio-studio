// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation of effect parameters, as the web version's engine does it.
//!
//! A parameter reaches an effect in one of three ways:
//!
//! 1. the slot's `mix`, which ramps the wet and dry gains;
//! 2. timed ramps merged into a per-parameter line, read at the start of every
//!    128-frame block (the web version's audio worklets);
//! 3. state updates on a fixed 30 ms grid, applied at the block boundary
//!    nearest each point (everything else).

use crate::param::{Param, Timeline};
use las_dsp::fx::registry::{descriptor, full_state, parse_params, EffectUnit, ParamDef, ParamKind};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// Frames in a Web Audio render quantum.
pub const QUANTUM: usize = 128;
/// Grid (seconds) used to follow non-linear parameter curves.
const CURVE_GRID: f64 = 0.02;
/// Grid (seconds) for parameters that can only change through state updates.
const STATE_GRID: f64 = 0.03;

/// Add points on a fixed time grid inside every segment, so linear ramps follow a
/// non-linear mapping. The grid is absolute.
pub fn densify(points: &[(f64, f64)], step: f64) -> Vec<(f64, f64)> {
    const MAX_POINTS: usize = 20000;
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let span = b.0 - a.0;
        if span > 0.0 {
            let mut k = (a.0 / step).floor() + 1.0;
            while k * step < b.0 && out.len() < MAX_POINTS {
                let t = k * step;
                k += 1.0;
                if t - a.0 < 1e-6 {
                    continue;
                }
                out.push((t, a.1 + ((b.1 - a.1) * (t - a.0)) / span));
            }
        }
        out.push(b);
    }
    out
}

/// Turn a point list into a staircase: values hold until the next point.
fn staircase(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for &p in points {
        if let Some(&prev) = out.last() {
            if p.1 != prev.1 && p.0 - prev.0 > 2e-4 {
                out.push((p.0 - 1e-4, prev.1));
            }
        }
        out.push(p);
    }
    out
}

/// Real values for normalised automation points: non-linear and stepped
/// parameters are sampled on a grid first.
pub fn map_automation(def: Option<&ParamDef>, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let Some(def) = def else {
        return points.to_vec();
    };
    let curved = matches!(
        def.curve,
        Some(las_dsp::fx::registry::Curve::Log | las_dsp::fx::registry::Curve::Pow)
    ) || def.step.is_some();
    let pts = if curved {
        densify(points, CURVE_GRID)
    } else {
        points.to_vec()
    };
    let mapped: Vec<(f64, f64)> = pts
        .iter()
        .map(|&(t, v)| (t, def.from_normalized(v)))
        .collect();
    if def.step.is_some() {
        staircase(&mapped)
    } else {
        mapped
    }
}

/// The state value for a normalised point: an option, a flag, or a number.
pub fn state_value(def: Option<&ParamDef>, n: f64) -> Value {
    if let Some(d) = def {
        if !d.options.is_empty() {
            let i = ((n * (d.options.len() - 1) as f64) + 0.5).floor().max(0.0) as usize;
            return d.options[i.min(d.options.len() - 1)].value.clone();
        }
        if d.kind == ParamKind::Toggle {
            return json!(n >= 0.5);
        }
        return json!(d.from_normalized(n));
    }
    json!(n)
}

/// Set `path` (dotted, array indices as numbers) in `v`, creating what is missing.
pub fn with_path(v: &mut Value, path: &str, value: Value) {
    let mut cur = v;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let index = part.parse::<usize>().ok();
        if let (Some(ix), true) = (index, cur.is_array() || cur.is_null()) {
            if cur.is_null() {
                *cur = json!([]);
            }
            let arr = cur.as_array_mut().expect("an array");
            while arr.len() <= ix {
                arr.push(Value::Null);
            }
            if last {
                arr[ix] = value;
                return;
            }
            cur = &mut arr[ix];
        } else {
            if !cur.is_object() {
                *cur = json!({});
            }
            let map = cur.as_object_mut().expect("an object");
            if last {
                map.insert((*part).to_string(), value);
                return;
            }
            cur = map.entry((*part).to_string()).or_insert(Value::Null);
        }
    }
}

/// The parameter definition of an effect target's path.
pub fn effect_param_def(plugin_id: &str, path: &str) -> Option<ParamDef> {
    if path == "mix" {
        return serde_json::from_value(json!({
            "id": "mix", "label": "Mix", "min": 0.0, "max": 1.0, "default": 1.0
        }))
        .ok();
    }
    descriptor(plugin_id)?
        .params
        .iter()
        .find(|p| p.id == path)
        .cloned()
}

/// Whether an effect's parameter is automated with sample-accurate ramps (the
/// web version's worklets) rather than state updates.
fn is_ramped(plugin_id: &str, path: &str) -> bool {
    let fixed: &[&str] = match plugin_id {
        "las-comp" | "las-ducker" => &[
            "threshold", "ratio", "attack", "release", "knee", "makeup", "mix", "scHpf",
        ],
        "las-limiter" => &["gain", "ceiling", "release", "lookahead"],
        "las-crusher" => &["bits", "downsample", "mix"],
        "las-shaper" => &["depth", "smooth", "offset"],
        "las-gate" => &["depth", "smooth"],
        "las-stepfx" => &["res", "glide", "mix"],
        "las-noisegate" | "las-transient" | "las-tape" | "las-freqshift" | "las-pitch"
        | "las-hall" | "las-meter" | "las-delay" => {
            return descriptor(plugin_id).is_some_and(|d| {
                d.params
                    .iter()
                    .any(|p| p.id == path && p.kind == ParamKind::Knob)
            });
        }
        _ => return false,
    };
    fixed.contains(&path)
}

struct Timed {
    frame: usize,
    path: String,
    value: Value,
}

/// The automation of one effect slot.
pub struct SlotAuto {
    plugin_id: String,
    sr: f64,
    /// The saved state with the timed updates so far.
    base: Value,
    /// The state the effect last got.
    applied: Value,
    lines: BTreeMap<String, Timeline>,
    timed: Vec<Timed>,
    next: usize,
    last_timed: HashMap<String, Value>,
    pub wet: Param,
    pub dry: Param,
    /// Whether the slot is switched on: off, `mix` is not automated.
    enabled: bool,
}

impl SlotAuto {
    pub fn new(plugin_id: &str, state: &Value, enabled: bool, mix: f64, sr: f64) -> Self {
        let base = full_state(plugin_id, state);
        let m = if enabled { mix.clamp(0.0, 1.0) } else { 0.0 };
        Self {
            plugin_id: plugin_id.to_string(),
            sr,
            applied: base.clone(),
            base,
            lines: BTreeMap::new(),
            timed: Vec::new(),
            next: 0,
            last_timed: HashMap::new(),
            wet: Param::new(m),
            dry: Param::new(1.0 - m),
            enabled,
        }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Automate `path` through `points` (normalised, in seconds). `ramped` points are
    /// the same, already mapped; `first` jumps to the first value.
    pub fn automate(&mut self, path: &str, points: &[(f64, f64)], first: bool, length: usize) {
        let def = effect_param_def(&self.plugin_id, path);
        if path == "mix" {
            if self.enabled {
                let clamp = |v: f64| v.clamp(0.0, 1.0);
                let wet: Vec<(f64, f64)> = points.iter().map(|&(t, v)| (t, clamp(v))).collect();
                let dry: Vec<(f64, f64)> =
                    points.iter().map(|&(t, v)| (t, 1.0 - clamp(v))).collect();
                self.wet.ramp(&wet, first);
                self.dry.ramp(&dry, first);
            }
            return;
        }
        if is_ramped(&self.plugin_id, path) {
            let mapped = map_automation(def.as_ref(), points);
            self.lines
                .entry(path.to_string())
                .or_default()
                .set(&mapped, first);
            return;
        }
        // state updates on a grid, whenever the value changes
        for &(t, n) in &densify(points, STATE_GRID) {
            let v = state_value(def.as_ref(), n);
            if self.last_timed.get(path) == Some(&v) {
                continue;
            }
            self.last_timed.insert(path.to_string(), v.clone());
            let frame = ((t * self.sr / QUANTUM as f64) + 0.5).floor() * QUANTUM as f64;
            if frame >= length as f64 {
                continue;
            }
            let frame = frame.max(0.0) as usize;
            let at = self.timed.partition_point(|e| e.frame <= frame);
            self.timed.insert(
                at,
                Timed {
                    frame,
                    path: path.to_string(),
                    value: v,
                },
            );
        }
    }

    /// Whether anything of the slot changes over time (beyond its mix).
    fn moving(&self) -> bool {
        !self.lines.is_empty() || self.next < self.timed.len()
    }

    /// Bring the effect's parameters to what they are at the block starting at `frame`.
    pub fn begin_block(&mut self, frame: usize, unit: &mut EffectUnit) {
        if !self.moving() {
            return;
        }
        while self.next < self.timed.len() && self.timed[self.next].frame <= frame {
            let e = &self.timed[self.next];
            with_path(&mut self.base, &e.path, e.value.clone());
            self.next += 1;
        }
        let t0 = frame as f64 / self.sr;
        let mut state = self.base.clone();
        for (path, line) in &self.lines {
            if let Some(v) = line.value_at(t0) {
                with_path(&mut state, path, json!(v));
            }
        }
        if state != self.applied {
            if let Ok(p) = parse_params(&self.plugin_id, &state, self.sr) {
                unit.apply(p);
            }
            self.applied = state;
        }
    }
}
