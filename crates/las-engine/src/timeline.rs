// SPDX-License-Identifier: GPL-3.0-or-later
//! Song time: ticks, tempo and where notes fall. Port of the web version's
//! `core/timeline.ts` and the offline half of its scheduler.

use las_project::{AutomationClip, Clip, ClipKind, Note, Pattern, Project, PPQ, TICKS_PER_STEP};

pub const PPQ_F: f64 = PPQ as f64;
pub const STEP_F: f64 = TICKS_PER_STEP as f64;
/// Tempo automation range: 0 to 1 maps onto this, in BPM.
pub const TEMPO_RANGE: (f64, f64) = (60.0, 200.0);
/// Tempo is read once per cell, so every render agrees.
pub const TEMPO_CELL: f64 = STEP_F;

/// `Math.round`.
pub fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

pub fn ticks_per_bar(sig: (u32, u32)) -> f64 {
    PPQ_F * 4.0 * f64::from(sig.0) / f64::from(sig.1)
}

pub fn pattern_length(p: Option<&Pattern>, sig: (u32, u32)) -> f64 {
    let bar = ticks_per_bar(sig);
    let Some(p) = p else { return bar };
    if p.length > 0.0 {
        return p.length;
    }
    let mut end = 0f64;
    for notes in p.notes.values() {
        for n in notes {
            // step notes count by their cell, not their one-shot length
            let e = if n.step == Some(true) {
                n.pos + STEP_F
            } else {
                n.pos + n.len
            };
            end = end.max(e);
        }
    }
    bar.max((end / bar).ceil() * bar)
}

pub fn song_length(p: &Project) -> f64 {
    let bar = ticks_per_bar(p.time_sig);
    let end = p
        .playlist
        .clips
        .iter()
        .fold(0f64, |e, c| e.max(c.start + c.length));
    bar.max((end / bar).ceil() * bar)
}

pub fn ticks_to_seconds(ticks: f64, bpm: f64) -> f64 {
    (ticks / PPQ_F) * (60.0 / bpm)
}

/// Offset in ticks that swing gives a note at `pos`.
pub fn swing_offset(pos: f64, swing: f64) -> f64 {
    if swing <= 0.0 {
        return 0.0;
    }
    let idx = js_round(pos / STEP_F);
    if (idx * STEP_F - pos).abs() > 0.5 {
        return 0.0;
    }
    if idx % 2.0 == 1.0 {
        swing * STEP_F * 0.5
    } else {
        0.0
    }
}

pub struct Scheduled<'a> {
    pub channel: &'a str,
    pub note: &'a Note,
    /// Absolute song tick of the start (after swing).
    pub tick: f64,
}

/// The notes of a pattern that start in `[from, to)`, with the pattern placed at
/// `origin` and only `[src_start, src_end)` of it audible.
#[allow(clippy::too_many_arguments)]
pub fn pattern_notes_in_range<'a>(
    pattern: &'a Pattern,
    origin: f64,
    src_start: f64,
    src_end: f64,
    from: f64,
    to: f64,
    swing: f64,
    out: &mut Vec<Scheduled<'a>>,
) {
    for (channel, notes) in &pattern.notes {
        for note in notes {
            if note.pos < src_start || note.pos >= src_end {
                continue;
            }
            let swung = if note.step == Some(true) {
                swing_offset(note.pos, swing)
            } else {
                0.0
            };
            let tick = origin + (note.pos - src_start) + swung;
            if tick >= from && tick < to {
                out.push(Scheduled {
                    channel,
                    note,
                    tick,
                });
            }
        }
    }
}

pub fn song_notes_in_range<'a>(
    p: &'a Project,
    from: f64,
    to: f64,
    muted_tracks: &[bool],
) -> Vec<Scheduled<'a>> {
    let mut out = Vec::new();
    for clip in &p.playlist.clips {
        if clip.kind != ClipKind::Pattern
            || clip.muted == Some(true)
            || muted_tracks.get(clip.track).copied().unwrap_or(false)
        {
            continue;
        }
        if clip.start >= to || clip.start + clip.length + PPQ_F < from {
            continue;
        }
        let Some(pat) = p.pattern(&clip.ref_id) else {
            continue;
        };
        pattern_notes_in_range(
            pat,
            clip.start,
            clip.offset,
            clip.offset + clip.length,
            from,
            to,
            p.swing,
            &mut out,
        );
    }
    out
}

/// Value of an automation clip at clip tick `t` (0 to 1).
pub fn automation_value_at(clip: &AutomationClip, t: f64) -> f64 {
    let pts = &clip.points;
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
        return 0.5;
    };
    if t <= first.t {
        return first.v;
    }
    for i in 1..pts.len() {
        if t <= pts[i].t {
            let (a, b) = (&pts[i - 1], &pts[i]);
            let k = (t - a.t) / (b.t - a.t).max(1e-9);
            return a.v + (b.v - a.v) * k;
        }
    }
    last.v
}

/// An automation clip placed in the playlist that drives the tempo.
fn tempo_clips(p: &Project) -> Vec<(&Clip, &AutomationClip)> {
    p.playlist
        .clips
        .iter()
        .filter(|c| {
            c.kind == ClipKind::Automation
                && c.muted != Some(true)
                && !p.playlist.tracks.get(c.track).is_some_and(|t| t.mute)
        })
        .filter_map(|c| {
            let auto = p.automation.iter().find(|a| a.id == c.ref_id)?;
            let t = auto.target.as_object()?;
            let is_tempo =
                t.get("kind")?.as_str()? == "master" && t.get("param")?.as_str()? == "tempo";
            is_tempo.then_some((c, auto))
        })
        .collect()
}

pub fn has_tempo_automation(p: &Project, song: bool) -> bool {
    song && !tempo_clips(p).is_empty()
}

/// BPM in effect at `tick` (tempo automation applies in song mode).
pub fn tempo_at(p: &Project, tick: f64, song: bool) -> f64 {
    if !song {
        return p.bpm;
    }
    let cell = (tick / TEMPO_CELL).floor() * TEMPO_CELL;
    for (clip, auto) in tempo_clips(p) {
        if cell >= clip.start && cell < clip.start + clip.length {
            let v = automation_value_at(auto, cell - clip.start + clip.offset);
            return TEMPO_RANGE.0 + (TEMPO_RANGE.1 - TEMPO_RANGE.0) * v.clamp(0.0, 1.0);
        }
    }
    p.bpm
}

/// Seconds to play from tick `from` to tick `to`, following the tempo map.
pub fn seconds_between(p: &Project, from: f64, to: f64, song: bool) -> f64 {
    if to <= from {
        return 0.0;
    }
    if !has_tempo_automation(p, song) {
        return ticks_to_seconds(to - from, p.bpm);
    }
    let mut sec = 0.0;
    let mut t = from;
    while t < to {
        let next = to.min(((t / TEMPO_CELL).floor() + 1.0) * TEMPO_CELL);
        sec += ticks_to_seconds(next - t, tempo_at(p, t, song));
        t = next;
    }
    sec
}
