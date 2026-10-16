// SPDX-License-Identifier: GPL-3.0-or-later
//! A Web Audio `AudioParam` with its automation timeline, as Chromium runs it:
//! `setValueAtTime`, `linearRampToValueAtTime`, `setTargetAtTime` and
//! `cancelScheduledValues`, evaluated for every frame.
//!
//! Where Chromium departs from the specification, so does this: a target
//! curve starts from the value of the last frame before it begins (not from
//! the ramp's end value), and it is declared finished (the value snaps to the
//! target) at the start of the first render quantum that finds it within 1.5
//! sixteen-bit steps.

const QUANTUM: usize = 128;
const SET_TARGET_THRESHOLD: f64 = 1.5 / 32768.0;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Set,
    Ramp,
    Target { tc: f64 },
}

#[derive(Clone, Copy, Debug)]
struct Event {
    time: f64,
    value: f64,
    kind: Kind,
}

#[derive(Clone, Debug)]
pub struct Param {
    events: Vec<Event>,
    default: f64,
    running: f64,
    /// The latest event whose time has come.
    cur: Option<usize>,
    /// Where the current target curve started from.
    init: f64,
    finished: bool,
}

impl Param {
    pub fn new(default: f64) -> Self {
        Self {
            events: Vec::new(),
            default,
            running: default,
            cur: None,
            init: default,
            finished: false,
        }
    }

    fn insert(&mut self, e: Event) {
        // sorted by time; an event after others at the same time goes after them
        let at = self.events.partition_point(|x| x.time <= e.time);
        self.events.insert(at, e);
    }

    pub fn set_value_at_time(&mut self, value: f64, time: f64) {
        self.insert(Event {
            time,
            value,
            kind: Kind::Set,
        });
    }

    pub fn linear_ramp_to_value_at_time(&mut self, value: f64, time: f64) {
        self.insert(Event {
            time,
            value,
            kind: Kind::Ramp,
        });
    }

    pub fn set_target_at_time(&mut self, target: f64, start: f64, time_constant: f64) {
        self.insert(Event {
            time: start,
            value: target,
            kind: Kind::Target {
                tc: time_constant.max(0.0),
            },
        });
    }

    /// Remove every event at or after `time`.
    pub fn cancel_scheduled_values(&mut self, time: f64) {
        self.events.retain(|e| e.time < time);
    }

    /// Automate through `points` (time, value): a jump to the first when
    /// `first`, a ramp to it otherwise, then ramps. This is what the web
    /// version's `rampParam` does.
    pub fn ramp(&mut self, points: &[(f64, f64)], first: bool) {
        let Some(&(t0, v0)) = points.first() else {
            return;
        };
        if first {
            self.set_value_at_time(v0, t0);
        } else {
            self.linear_ramp_to_value_at_time(v0, t0);
        }
        for &(t, v) in &points[1..] {
            self.linear_ramp_to_value_at_time(v, t);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Time of the last event.
    pub fn last_time(&self) -> Option<f64> {
        self.events.last().map(|e| e.time)
    }

    /// The value event `c` gives at time `t` once nothing else has begun.
    fn hold(&self, c: usize, t: f64) -> f64 {
        let p = self.events[c];
        match p.kind {
            Kind::Set | Kind::Ramp => p.value,
            Kind::Target { tc } => {
                if self.finished || tc <= 0.0 {
                    p.value
                } else {
                    p.value + (self.init - p.value) * (-(t - p.time) / tc).exp()
                }
            }
        }
    }

    /// Fill `out` with the values of frames `start..start + out.len()`. Calls
    /// must follow each other without gaps (a param that is not asked for does
    /// not move).
    pub fn fill(&mut self, start: usize, out: &mut [f32], sample_rate: f64) {
        if self.events.is_empty() {
            out.fill(self.default as f32);
            return;
        }
        for (i, o) in out.iter_mut().enumerate() {
            let f = start + i;
            let t = f as f64 / sample_rate;
            if f % QUANTUM == 0 && !self.finished {
                if let Some(Event {
                    kind: Kind::Target { .. },
                    value,
                    ..
                }) = self.cur.map(|c| self.events[c])
                {
                    if (self.running - value).abs() < SET_TARGET_THRESHOLD {
                        self.finished = true;
                    }
                }
            }
            // enter the events whose time has come
            loop {
                let next = self.cur.map_or(0, |c| c + 1);
                match self.events.get(next) {
                    // a ramp belongs to the stretch before its time
                    Some(e) if e.time <= t && !matches!(e.kind, Kind::Ramp) => {
                        let prev = self.cur;
                        self.cur = Some(next);
                        if matches!(e.kind, Kind::Target { .. }) {
                            // the curve starts from the last frame at or before its time:
                            // the one before, unless its time is exactly this frame
                            self.init = if e.time == t {
                                prev.map_or(self.running, |c| self.hold(c, t))
                            } else {
                                self.running
                            };
                            self.finished = false;
                        } else {
                            // a target that starts at the same time begins from this
                            self.running = e.value;
                        }
                    }
                    Some(e) if e.time <= t => {
                        // a ramp whose end has passed: it holds its value
                        self.cur = Some(next);
                    }
                    _ => break,
                }
            }
            let next = self.cur.map_or(0, |c| c + 1);
            let v = match (self.events.get(next), self.cur.map(|c| self.events[c])) {
                (Some(r), prev) if matches!(r.kind, Kind::Ramp) => {
                    let (t0, v0) = match prev {
                        Some(p) => (
                            p.time,
                            match p.kind {
                                Kind::Target { .. } => self.init,
                                _ => p.value,
                            },
                        ),
                        None => (0.0, self.default),
                    };
                    if r.time <= t0 {
                        r.value
                    } else {
                        v0 + (r.value - v0) * (t - t0) / (r.time - t0)
                    }
                }
                (_, None) => self.default,
                (_, Some(p)) => match p.kind {
                    Kind::Set | Kind::Ramp => p.value,
                    Kind::Target { tc } => {
                        if self.finished || tc <= 0.0 {
                            p.value
                        } else {
                            p.value + (self.init - p.value) * (-(t - p.time) / tc).exp()
                        }
                    }
                },
            };
            self.running = v;
            *o = v as f32;
        }
    }
}

/// Automation read when a note starts (the web version's `ParamTimeline`):
/// piecewise linear points, merged as segments arrive.
#[derive(Clone, Debug, Default)]
pub struct Timeline {
    points: Vec<(f64, f64)>,
}

impl Timeline {
    pub fn set(&mut self, incoming: &[(f64, f64)], jump: bool) {
        let (Some(first), Some(last)) = (incoming.first(), incoming.last()) else {
            return;
        };
        let (t0, t1) = (first.0, last.0);
        let mut out: Vec<(f64, f64)> = self
            .points
            .iter()
            .copied()
            .filter(|p| p.0 < t0 || p.0 > t1)
            .collect();
        if jump {
            // hold the previous value right up to the start, then step
            if let Some(before) = self.value_at(t0 - 1e-6) {
                out.push((t0 - 1e-6, before));
            }
        }
        out.extend_from_slice(incoming);
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.points = out;
    }

    /// The value at `time`: none before the first point, the last one held after.
    pub fn value_at(&self, time: f64) -> Option<f64> {
        let first = self.points.first()?;
        if time < first.0 {
            return None;
        }
        if time == first.0 {
            return Some(first.1);
        }
        for w in self.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if time <= b.0 {
                let span = b.0 - a.0;
                return Some(if span <= 0.0 {
                    b.1
                } else {
                    a.1 + ((b.1 - a.1) * (time - a.0)) / span
                });
            }
        }
        self.points.last().map(|p| p.1)
    }

    /// The automated value at `time`, or `fallback` when there is none.
    pub fn get(&self, time: f64, fallback: f64) -> f64 {
        self.value_at(time).unwrap_or(fallback)
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48000.0;

    fn render(p: &mut Param, n: usize) -> Vec<f32> {
        let mut out = vec![0f32; n];
        for (k, chunk) in out.chunks_mut(128).enumerate() {
            p.fill(k * 128, chunk, SR);
        }
        out
    }

    // Reference values from Chromium (see `tools/project`).
    #[test]
    fn a_ramp_then_a_target_starts_from_the_last_frame_before_it() {
        let t = 100.4 / SR;
        let mut p = Param::new(0.0);
        p.set_value_at_time(0.0, t);
        p.linear_ramp_to_value_at_time(1.0, t + 0.001);
        p.set_target_at_time(0.5, t + 0.001, 0.1 / 3.0);
        let g = render(&mut p, 12000);
        for (n, want) in [
            (100, 0.0),
            (101, 0.012499999),
            (102, 0.033333335),
            (148, 0.99166667),
            (149, 0.9914823),
            (150, 0.99117523),
            (151, 0.9908683),
            (300, 0.94722003),
            (1000, 0.78874707),
            (5000, 0.5237014),
        ] {
            assert!(
                (f64::from(g[n]) - want).abs() < 1e-6,
                "{n}: {} vs {want}",
                g[n]
            );
        }
    }

    #[test]
    fn cancelling_and_a_new_target_fade_out_and_then_snap_to_zero() {
        let t = 100.4 / SR;
        let rel = 4000.3 / SR;
        let mut p = Param::new(0.0);
        p.set_value_at_time(0.0, t);
        p.linear_ramp_to_value_at_time(1.0, t + 0.001);
        p.set_target_at_time(0.5, t + 0.001, 0.1 / 3.0);
        p.cancel_scheduled_values(rel);
        p.set_target_at_time(0.0, rel, 0.05 / 4.0);
        let g = render(&mut p, 12000);
        for (n, want) in [
            (3999, 0.5443087),
            (4000, 0.544281),
            (4001, 0.5436464),
            (4002, 0.54274106),
            (4100, 0.4609544),
            (5000, 0.10285284),
            (8000, 0.00069301797),
        ] {
            assert!(
                (f64::from(g[n]) - want).abs() < 1e-6,
                "{n}: {} vs {want}",
                g[n]
            );
        }
        // the curve is declared finished at the start of the first quantum within 1.5/32768
        assert!(g[9727] > 0.0 && g[9728] == 0.0 && g[11999] == 0.0);
    }

    #[test]
    fn a_value_snaps_at_the_quantum_that_finds_it_close_to_the_target() {
        // (initial value, target, time constant in frames, snap frame) from Chromium
        for (init, target, tc, snap) in [
            (1.0, 0.0, 100.0, 2048usize),
            (0.1, 0.0, 100.0, 1792),
            (1.0, 0.5, 100.0, 2048),
            (1.0, 0.0, 400.0, 5120),
            (0.01, 0.0, 400.0, 3200),
            (0.5, 1.0, 200.0, 2944),
        ] {
            let mut p = Param::new(0.0);
            p.set_value_at_time(init, 1000.0 / SR);
            p.set_target_at_time(target, 1000.0 / SR, tc / SR);
            let g = render(&mut p, 24000);
            let at = (1000..g.len())
                .find(|&i| f64::from(g[i]) == target)
                .unwrap();
            assert_eq!(at, snap, "init {init} target {target} tc {tc}");
        }
    }
}
