// SPDX-License-Identifier: GPL-3.0-or-later
//! The mixer: inserts with effects, width, pan, fader, mute, solo, sends,
//! routing, sidechain and plugin delay compensation. Port of the web version's
//! `audio/mixer.ts`, which builds the same thing from Web Audio nodes.

use crate::external::{plugin_host, PLUGIN_ID};
use crate::fxauto::{effect_param_def, map_automation, SlotAuto};
use crate::instrument::stereo_pan_a;
use crate::param::Param;
use las_dsp::fx::registry::{descriptor, full_state, parse_params, EffectUnit, FxSlot};
use las_project::{MixerTrack, MIXER_SLOTS};
use serde_json::{json, Value};

/// A fractional delay with linear interpolation, like a Web Audio `DelayNode`.
struct LinearDelay {
    buf: Vec<f32>,
    pos: usize,
}

impl LinearDelay {
    fn new(frames: usize) -> Self {
        Self {
            buf: vec![0.0; frames.max(4) + 2],
            pos: 0,
        }
    }

    /// Delay `x` in place by `frames` (fractional).
    fn run(&mut self, x: &mut [f32], frames: f64) {
        let size = self.buf.len();
        let d = frames.clamp(0.0, (size - 2) as f64);
        let whole = d.floor() as usize;
        let frac = (d - d.floor()) as f32;
        for s in x.iter_mut() {
            self.buf[self.pos] = *s;
            let a = self.buf[(self.pos + size - whole) % size];
            let b = self.buf[(self.pos + size - whole - 1) % size];
            *s = a + (b - a) * frac;
            self.pos = (self.pos + 1) % size;
        }
    }
}

struct Delay2 {
    l: LinearDelay,
    r: LinearDelay,
}

impl Delay2 {
    fn new(sr: f64, seconds: f64) -> Self {
        let frames = (seconds.max(1.0) * sr).ceil() as usize + 2;
        Self {
            l: LinearDelay::new(frames),
            r: LinearDelay::new(frames),
        }
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32], frames: f64) {
        self.l.run(l, frames);
        self.r.run(r, frames);
    }
}

/// Where an insert sends its output.
struct Route {
    target: usize,
    gain: f32,
    /// Compensation delay for this path, seconds.
    delay: f64,
    line: Option<Delay2>,
}

/// An effect slot of an insert, and where it sits in the model.
struct Slot {
    id: String,
    fx: FxSlot,
    auto: SlotAuto,
    /// The insert that feeds its sidechain, if it takes one.
    sidechain: Option<usize>,
    takes_sidechain: bool,
}

struct Track {
    slots: Vec<Slot>,
    bypass: bool,
    width: EffectUnit,
    pan: Param,
    fader: Param,
    mute: f32,
    /// Instruments entering here, summed.
    input: [Vec<f32>; 2],
    /// Everything that arrives (instruments after compensation, routes).
    sum: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    tmp: [Vec<f32>; 2],
    input_delay: f64,
    input_line: Option<Delay2>,
    routes: Vec<Route>,
    latency: f64,
}

pub struct Mixer {
    tracks: Vec<Track>,
    order: Vec<usize>,
    master: Param,
    vals: [Vec<f32>; 2],
    /// Delay from the timeline to each insert's output, for timing automation.
    sr: f64,
    sc: [Vec<f32>; 2],
    /// Arrival time of each insert and its effects latency, seconds.
    arrival: Vec<f64>,
    length: usize,
    wet_gain: Vec<f32>,
    dry_gain: Vec<f32>,
}

/// Inserts muted because another insert is soloed (FL semantics).
pub fn solo_mutes(mixer: &[MixerTrack]) -> Vec<bool> {
    let soloed: Vec<usize> = mixer
        .iter()
        .enumerate()
        .filter(|(i, t)| t.solo && *i > 0)
        .map(|(i, _)| i)
        .collect();
    let mut muted = vec![false; mixer.len()];
    if soloed.is_empty() {
        return muted;
    }
    let edges = |i: usize| -> Vec<usize> {
        let t = &mixer[i];
        let mut out: Vec<usize> = t.output.into_iter().collect();
        out.extend(t.sends.iter().map(|s| s.target));
        out
    };
    let reach = |from: usize, test: &dyn Fn(usize) -> bool| -> bool {
        let mut seen = vec![false; mixer.len()];
        let mut stack = vec![from];
        while let Some(i) = stack.pop() {
            if i >= mixer.len() || seen[i] {
                continue;
            }
            seen[i] = true;
            if i != from && test(i) {
                return true;
            }
            stack.extend(edges(i));
        }
        false
    };
    for (i, m) in muted.iter_mut().enumerate().skip(1) {
        if soloed.contains(&i) {
            continue;
        }
        let downstream = soloed.iter().any(|&s| reach(s, &|x| x == i));
        let feeds = reach(i, &|x| soloed.contains(&x));
        if !downstream && !feeds {
            *m = true;
        }
    }
    muted
}

impl Mixer {
    pub fn new(mixer: &[MixerTrack], master_volume: f64, sr: f64, block: usize) -> Self {
        let n = mixer.len();
        let muted = solo_mutes(mixer);
        let mut tracks: Vec<Track> = Vec::with_capacity(n);
        for (i, t) in mixer.iter().enumerate() {
            let mut slots = Vec::new();
            for s in t.slots.iter().take(MIXER_SLOTS).flatten() {
                let unit = if s.plugin_id == PLUGIN_ID {
                    let hosted = plugin_host().and_then(|h| h.effect(&s.state, sr));
                    let Some(p) = hosted else {
                        continue;
                    };
                    EffectUnit::External(p)
                } else {
                    let Some(mut unit) = EffectUnit::new(&s.plugin_id, sr) else {
                        continue;
                    };
                    let st = full_state(&s.plugin_id, &s.state);
                    if let Ok(p) = parse_params(&s.plugin_id, &st, sr) {
                        unit.apply(p);
                    }
                    unit
                };
                let mut fx = FxSlot::new(unit, sr);
                fx.enabled = s.enabled;
                fx.mix = s.mix;
                let takes = descriptor(&s.plugin_id).is_some_and(|d| d.sidechain);
                // an explicit source, or (as in FL) an insert routed to us "sidechain only"
                let explicit = s
                    .state
                    .get("sidechain")
                    .and_then(Value::as_u64)
                    .map(|x| x as usize);
                let src = explicit.or_else(|| {
                    mixer.iter().enumerate().position(|(mi, m)| {
                        mi != i && m.sends.iter().any(|x| x.target == i && x.sidechain_only)
                    })
                });
                let sidechain = src.filter(|&x| x != i && x < n);
                slots.push(Slot {
                    id: s.id.clone(),
                    auto: SlotAuto::new(&s.plugin_id, &s.state, s.enabled, s.mix, sr),
                    fx,
                    sidechain: if takes { sidechain } else { None },
                    takes_sidechain: takes,
                });
            }
            let mut width = EffectUnit::new("las-stereo", sr).expect("the stereo tool exists");
            if let Ok(p) = parse_params(
                "las-stereo",
                &full_state("las-stereo", &json!({ "width": t.width })),
                sr,
            ) {
                width.apply(p);
            }
            let z = || vec![0f32; block];
            tracks.push(Track {
                slots,
                bypass: t.fx_bypass,
                width,
                pan: Param::new(t.pan),
                fader: Param::new(t.volume),
                mute: if t.mute || muted[i] { 0.0 } else { 1.0 },
                input: [z(), z()],
                sum: [z(), z()],
                out: [z(), z()],
                tmp: [z(), z()],
                input_delay: 0.0,
                input_line: None,
                routes: Vec::new(),
                latency: 0.0,
            });
        }
        for t in &mut tracks {
            t.latency = if t.bypass {
                0.0
            } else {
                t.slots.iter().map(|s| s.fx.latency()).sum()
            };
        }
        // plugin delay compensation: every path into an insert arrives aligned
        let mut arrival = vec![0f64; n];
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (i, t) in mixer.iter().enumerate().skip(1) {
            if let Some(o) = t.output.filter(|&o| o != i && o < n) {
                edges.push((i, o));
            }
            for s in &t.sends {
                if !s.sidechain_only && s.target != i && s.target < n {
                    edges.push((i, s.target));
                }
            }
        }
        for _ in 0..n {
            let mut changed = false;
            for &(a, b) in &edges {
                let t = arrival[a] + tracks[a].latency;
                if t > arrival[b] + 1e-9 {
                    arrival[b] = t.min(2.0);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let edge_delay = |tracks: &[Track], a: usize, b: usize| -> f64 {
            if b < n {
                (arrival[b] - arrival[a] - tracks[a].latency).max(0.0)
            } else {
                0.0
            }
        };
        for (i, t) in mixer.iter().enumerate() {
            let mut routes = Vec::new();
            let mut add = |target: usize, gain: f32, tracks: &[Track]| {
                let delay = edge_delay(tracks, i, target);
                routes.push(Route {
                    target,
                    gain,
                    delay,
                    line: (delay > 0.0).then(|| Delay2::new(sr, delay)),
                });
            };
            if i > 0 {
                if let Some(o) = t.output.filter(|&o| o != i && o < n) {
                    add(o, 1.0, &tracks);
                }
            }
            for s in &t.sends {
                if s.sidechain_only || s.target == i || s.target >= n {
                    continue;
                }
                add(s.target, s.amount as f32, &tracks);
            }
            tracks[i].routes = routes;
        }
        for (i, t) in tracks.iter_mut().enumerate() {
            t.input_delay = arrival[i];
            t.input_line = (arrival[i] > 0.0).then(|| Delay2::new(sr, arrival[i]));
        }
        let order = Self::topological(&tracks);
        Self {
            tracks,
            order,
            master: Param::new(master_volume),
            vals: [vec![0.0; block], vec![0.0; block]],
            sr,
            sc: [vec![0.0; block], vec![0.0; block]],
            arrival,
            length: usize::MAX,
            wet_gain: Vec::new(),
            dry_gain: Vec::new(),
        }
    }

    /// Order the inserts so each comes after everything that feeds it.
    fn topological(tracks: &[Track]) -> Vec<usize> {
        let n = tracks.len();
        let mut indegree = vec![0usize; n];
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (i, t) in tracks.iter().enumerate() {
            for r in &t.routes {
                edges.push((i, r.target));
            }
            for s in &t.slots {
                if let Some(src) = s.sidechain {
                    edges.push((src, i));
                }
            }
        }
        for &(_, b) in &edges {
            indegree[b] += 1;
        }
        let mut order = Vec::with_capacity(n);
        let mut done = vec![false; n];
        while order.len() < n {
            let next = (0..n).find(|&i| !done[i] && indegree[i] == 0).or_else(|| {
                // a loop (the repair removes them): fall back to the lowest unfinished
                (0..n).find(|&i| !done[i])
            });
            let Some(i) = next else { break };
            done[i] = true;
            order.push(i);
            for &(a, b) in &edges {
                if a == i && indegree[b] > 0 {
                    indegree[b] -= 1;
                }
            }
        }
        order
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Automate an insert's `volume` or `pan` through `points` (time, value).
    pub fn automate_track(
        &mut self,
        index: usize,
        param: &str,
        points: &[(f64, f64)],
        first: bool,
    ) {
        let Some(t) = self.tracks.get_mut(index) else {
            return;
        };
        match param {
            "volume" => t.fader.ramp(points, first),
            "pan" => t.pan.ramp(points, first),
            _ => {}
        }
    }

    pub fn automate_master(&mut self, points: &[(f64, f64)], first: bool) {
        self.master.ramp(points, first);
    }

    /// Delay from the timeline to the input of effect slot `slot_id` on insert `index`.
    pub fn path_latency(&self, index: usize, slot_id: &str) -> f64 {
        let arrival = self.arrival.get(index).copied().unwrap_or(0.0);
        let Some(t) = self.tracks.get(index) else {
            return arrival;
        };
        if t.bypass {
            return arrival;
        }
        let mut lat = arrival;
        for s in &t.slots {
            if s.id == slot_id {
                break;
            }
            lat += s.fx.latency();
        }
        lat
    }

    /// Whether insert `index` has an effect slot `slot_id`.
    pub fn has_slot(&self, index: usize, slot_id: &str) -> bool {
        self.tracks
            .get(index)
            .is_some_and(|t| t.slots.iter().any(|s| s.id == slot_id))
    }

    /// The plugin id of an effect slot.
    pub fn slot_plugin(&self, index: usize, slot_id: &str) -> Option<String> {
        let t = self.tracks.get(index)?;
        let s = t.slots.iter().find(|s| s.id == slot_id)?;
        Some(s.auto.plugin_id().to_string())
    }

    /// Automate an effect parameter of an insert's slot through `points`
    /// (normalised values in seconds).
    pub fn automate_effect(
        &mut self,
        index: usize,
        slot_id: &str,
        path: &str,
        points: &[(f64, f64)],
        first: bool,
    ) {
        let length = self.length;
        let Some(t) = self.tracks.get_mut(index) else {
            return;
        };
        if let Some(s) = t.slots.iter_mut().find(|s| s.id == slot_id) {
            // parameters the effect automates itself (Web Audio parameters)
            if path != "mix" {
                let def = effect_param_def(s.auto.plugin_id(), path);
                let mapped = map_automation(def.as_ref(), points);
                if s.fx.unit.automate(path, &mapped, first) {
                    return;
                }
            }
            s.auto.automate(path, points, first, length);
        }
    }

    /// An insert's output after its fader, pan and mute, for the block just processed.
    pub fn insert_output(&self, index: usize) -> (&[f32], &[f32]) {
        let t = &self.tracks[index];
        (&t.out[0], &t.out[1])
    }

    /// Forget the automation that has passed.
    pub fn compact(&mut self) {
        self.master.compact();
        for t in &mut self.tracks {
            t.fader.compact();
            t.pan.compact();
        }
    }

    /// Frames the render lasts.
    pub fn length(&self) -> usize {
        self.length
    }

    /// Frames the render lasts, for the state updates that fall after it.
    pub fn set_length(&mut self, frames: usize) {
        self.length = frames;
    }

    /// Delay from the timeline to insert `index`'s output, seconds.
    pub fn output_latency(&self, index: usize) -> f64 {
        self.arrival.get(index).copied().unwrap_or(0.0)
            + self.tracks.get(index).map_or(0.0, |t| t.latency)
    }

    /// Beat position for the effects that follow the song.
    pub fn set_position(&mut self, beat: f64, bpm: f64) {
        for t in &mut self.tracks {
            for s in &mut t.slots {
                s.fx.unit
                    .set_position(las_dsp::fx::stepfx::Position { beat, bpm });
            }
        }
    }

    /// Start a block: clear what instruments will add to.
    pub fn begin(&mut self, n: usize) {
        for t in &mut self.tracks {
            for b in t.input.iter_mut().chain(t.sum.iter_mut()) {
                b.resize(n, 0.0);
                b.fill(0.0);
            }
        }
    }

    /// Add an instrument's output (after its fader and pan) to an insert.
    pub fn add_input(&mut self, track: usize, l: &[f32], r: &[f32]) {
        let last = self.tracks.len() - 1;
        let t = &mut self.tracks[track.min(last)];
        for i in 0..l.len() {
            t.input[0][i] += l[i];
            t.input[1][i] += r[i];
        }
    }

    /// Run every insert and write the master's output (after the master volume).
    pub fn process(&mut self, frame: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        for k in 0..self.order.len() {
            let i = self.order[k];
            // instruments arrive after compensation
            {
                let t = &mut self.tracks[i];
                let [il, ir] = &mut t.input;
                if let Some(line) = &mut t.input_line {
                    line.run(il, ir, t.input_delay * self.sr);
                }
                for j in 0..n {
                    t.sum[0][j] += t.input[0][j];
                    t.sum[1][j] += t.input[1][j];
                }
            }
            // effects
            let slot_count = self.tracks[i].slots.len();
            if !self.tracks[i].bypass {
                for s in 0..slot_count {
                    let src = self.tracks[i].slots[s].sidechain;
                    let takes = self.tracks[i].slots[s].takes_sidechain;
                    if takes {
                        match src {
                            Some(src) if src != i => {
                                self.sc[0].resize(n, 0.0);
                                self.sc[1].resize(n, 0.0);
                                self.sc[0].copy_from_slice(&self.tracks[src].out[0]);
                                self.sc[1].copy_from_slice(&self.tracks[src].out[1]);
                            }
                            _ => {
                                self.sc[0].resize(n, 0.0);
                                self.sc[1].resize(n, 0.0);
                                self.sc[0].fill(0.0);
                                self.sc[1].fill(0.0);
                            }
                        }
                    }
                    let t = &mut self.tracks[i];
                    let [sl, sr] = &mut t.sum;
                    let sc = takes.then_some((&self.sc[0][..], &self.sc[1][..]));
                    let slot = &mut t.slots[s];
                    slot.auto.begin_block(frame, &mut slot.fx.unit);
                    self.wet_gain.resize(n, 0.0);
                    self.dry_gain.resize(n, 0.0);
                    slot.auto.wet.fill(frame, &mut self.wet_gain, self.sr);
                    slot.auto.dry.fill(frame, &mut self.dry_gain, self.sr);
                    slot.fx
                        .process_gains(sl, sr, sc, &self.wet_gain, &self.dry_gain);
                }
            }
            // width, pan, fader, mute
            let t = &mut self.tracks[i];
            for b in &mut t.tmp {
                b.resize(n, 0.0);
            }
            for b in &mut t.out {
                b.resize(n, 0.0);
            }
            {
                let [wl, wr] = &mut t.tmp;
                let [ol, or] = &mut t.out;
                t.width.process(&t.sum[0], &t.sum[1], wl, wr, None);
                ol.copy_from_slice(wl);
                or.copy_from_slice(wr);
            }
            for v in &mut self.vals {
                v.resize(n, 0.0);
            }
            let [pan_v, fade_v] = &mut self.vals;
            t.pan.fill(frame, pan_v, self.sr);
            t.fader.fill(frame, fade_v, self.sr);
            let [ol, or] = &mut t.out;
            if pan_v.iter().any(|&p| p != 0.0) {
                stereo_pan_a(ol, or, pan_v);
            }
            for j in 0..n {
                let g = fade_v[j] * t.mute;
                ol[j] *= g;
                or[j] *= g;
            }
            // routes
            for ri in 0..self.tracks[i].routes.len() {
                let (target, gain, delay) = {
                    let r = &self.tracks[i].routes[ri];
                    (r.target, r.gain, r.delay)
                };
                let t = &mut self.tracks[i];
                let [tl, tr] = &mut t.tmp;
                for j in 0..n {
                    tl[j] = t.out[0][j] * gain;
                    tr[j] = t.out[1][j] * gain;
                }
                if let Some(line) = &mut t.routes[ri].line {
                    line.run(tl, tr, delay * self.sr);
                }
                let (cl, cr) = (t.tmp[0].clone(), t.tmp[1].clone());
                let dest = &mut self.tracks[target];
                for j in 0..n {
                    dest.sum[0][j] += cl[j];
                    dest.sum[1][j] += cr[j];
                }
            }
        }
        let m = &self.tracks[0];
        let gains = &mut self.vals[0];
        gains.resize(n, 0.0);
        self.master.fill(frame, gains, self.sr);
        for j in 0..n {
            out_l[j] = m.out[0][j] * gains[j];
            out_r[j] = m.out[1][j] * gains[j];
        }
    }
}
