// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounce in place and consolidate, as Studio One and Bitwig do: render a channel or a
//! mixer insert (in the song or in one pattern) to a WAV stored in the project and put
//! it on the playlist as an audio clip; or merge selected playlist clips into one.

use las_engine::{Mode, RenderOptions};
use las_project::{ClipKind, Project, Sample};
use std::collections::{BTreeMap, HashSet};

/// What is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A channel of the rack, alone, through its insert.
    Channel(usize),
    /// A mixer insert after its fader (0 is the master).
    Insert(usize),
}

/// Over what stretch: the whole song or one pattern (by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Song,
    Pattern(usize),
}

/// A render ready to run off the interface thread.
pub struct Job {
    pub project: Project,
    pub opts: RenderOptions,
    pub tap: Option<usize>,
    pub name: String,
}

/// Sample rate of bounced audio.
pub const RATE: f64 = 44100.0;

/// A copy of the project in which only `channel` makes sound.
fn only_channel(p: &Project, channel: usize) -> Option<Project> {
    let id = p.channels.get(channel)?.id.clone();
    let mut q = p.clone();
    for pat in &mut q.patterns {
        pat.notes.retain(|k, _| *k == id);
    }
    q.playlist
        .clips
        .retain(|c| c.kind != ClipKind::Audio || c.ref_id == id);
    for (i, c) in q.channels.iter_mut().enumerate() {
        if i == channel {
            c.mute = false;
        }
    }
    Some(q)
}

/// Prepare the render of `what` over `scope`.
pub fn job(p: &Project, what: Source, scope: Scope) -> Option<Job> {
    let (project, tap, name) = match what {
        Source::Channel(i) => {
            let c = p.channels.get(i)?;
            let insert = c.mixer_track.min(p.mixer.len().saturating_sub(1));
            (only_channel(p, i)?, Some(insert), c.name.clone())
        }
        Source::Insert(i) => {
            let t = p.mixer.get(i)?;
            let name = if t.name.is_empty() {
                if i == 0 {
                    "Master".to_string()
                } else {
                    format!("Insert {i}")
                }
            } else {
                t.name.clone()
            };
            let mut q = p.clone();
            // render it as if it were heard on its own: no mute or solo elsewhere
            for t in &mut q.mixer {
                t.solo = false;
            }
            q.mixer[i].mute = false;
            (q, (i > 0).then_some(i), name)
        }
    };
    let mode = match scope {
        Scope::Song => Mode::Song,
        Scope::Pattern(i) => Mode::Pattern {
            id: p.patterns.get(i).map(|x| x.id.clone()),
            loops: 1,
        },
    };
    Some(Job {
        project,
        opts: RenderOptions {
            sample_rate: RATE,
            tail: 1.0,
            mode,
        },
        tap,
        name: format!("{name} (rendered)"),
    })
}

/// Run a render: a 24-bit WAV file.
pub fn run(job: &Job, samples: &BTreeMap<String, Sample>) -> Result<Vec<u8>, String> {
    let r = las_engine::render_tap(&job.project, samples, &job.opts, job.tap)?;
    let peak = las_engine::export::peak_of(&[&r.left, &r.right]);
    if peak < 1e-5 {
        return Err("nothing to render: it is silent".into());
    }
    Ok(las_engine::export::encode_wav(
        &[&r.left, &r.right],
        r.sample_rate as u32,
        las_engine::export::BitDepth::B24,
        false,
    ))
}

/// Put a rendered WAV on the playlist at `at` ticks on the first free track, as an
/// audio clip, and mute the source when asked. Returns the new channel.
pub fn place(
    p: &mut Project,
    samples: &mut BTreeMap<String, Sample>,
    wav: Vec<u8>,
    name: &str,
    at: f64,
    mute: Option<Source>,
) -> Result<usize, String> {
    let track = crate::record::free_track(p, at);
    let ch = crate::menus::import_audio_clip(p, samples, name, wav, at, track)?;
    match mute {
        Some(Source::Channel(i)) => {
            if let Some(c) = p.channels.get_mut(i) {
                c.mute = true;
            }
        }
        Some(Source::Insert(i)) if i > 0 => {
            if let Some(t) = p.mixer.get_mut(i) {
                t.mute = true;
            }
        }
        _ => {}
    }
    Ok(ch)
}

/// Where a pattern-scope bounce goes: the first place the pattern is used, or the
/// start of the song.
pub fn pattern_start(p: &Project, pattern: usize) -> f64 {
    let Some(id) = p.patterns.get(pattern).map(|x| &x.id) else {
        return 0.0;
    };
    p.playlist
        .clips
        .iter()
        .filter(|c| c.kind == ClipKind::Pattern && &c.ref_id == id)
        .map(|c| c.start)
        .fold(None, |m: Option<f64>, s| Some(m.map_or(s, |m| m.min(s))))
        .unwrap_or(0.0)
}

/// The render that consolidates the clips `ids`: only they play, between the start of
/// the first and the end of the last, without master effects. Also returns where the
/// result goes: (start tick, track).
pub fn consolidate_job(p: &Project, ids: &HashSet<String>) -> Option<(Job, f64, usize)> {
    let chosen: Vec<_> = p
        .playlist
        .clips
        .iter()
        .filter(|c| ids.contains(&c.id) && c.kind != ClipKind::Automation)
        .collect();
    let from = chosen.iter().map(|c| c.start).reduce(f64::min)?;
    let to = chosen.iter().map(|c| c.start + c.length).reduce(f64::max)?;
    let track = chosen.iter().map(|c| c.track).min()?;
    let mut q = p.clone();
    q.playlist
        .clips
        .retain(|c| ids.contains(&c.id) || c.kind == ClipKind::Automation);
    if let Some(m) = q.mixer.get_mut(0) {
        m.slots.iter_mut().for_each(|s| *s = None);
    }
    Some((
        Job {
            project: q,
            opts: RenderOptions {
                sample_rate: RATE,
                tail: 0.0,
                mode: Mode::Range { from, to },
            },
            tap: None,
            name: "Consolidated".into(),
        },
        from,
        track,
    ))
}

/// Replace the clips `ids` by one audio clip of `wav` at `at` on `track`.
pub fn replace_clips(
    p: &mut Project,
    samples: &mut BTreeMap<String, Sample>,
    ids: &HashSet<String>,
    wav: Vec<u8>,
    at: f64,
    track: usize,
) -> Result<usize, String> {
    p.playlist
        .clips
        .retain(|c| !ids.contains(&c.id) || c.kind == ClipKind::Automation);
    crate::menus::import_audio_clip(p, samples, "Consolidated", wav, at, track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use las_project::create::{add_channel, step_note};

    /// Two channels on their own inserts, each with notes, and the pattern placed.
    fn song() -> Project {
        let mut p = las_project::create::default_project();
        p.bpm = 120.0;
        p.patterns.truncate(1);
        p.patterns[0].notes.clear();
        p.playlist.clips.retain(|c| c.kind == ClipKind::Pattern);
        let a = add_channel(&mut p, "polaris");
        let b = add_channel(&mut p, "polaris");
        let (ida, idb) = (p.channels[a].id.clone(), p.channels[b].id.clone());
        p.patterns[0].notes.insert(ida, vec![step_note(0, 60, 0.9)]);
        p.patterns[0].notes.insert(idb, vec![step_note(8, 72, 0.9)]);
        p
    }

    fn ch(p: &Project, plugin: &str, nth: usize) -> usize {
        p.channels
            .iter()
            .enumerate()
            .filter(|(_, c)| c.plugin_id == plugin)
            .nth(nth)
            .unwrap()
            .0
    }

    fn level(wav: &[u8], a: f64, b: f64) -> f32 {
        let au = las_engine::library::decode_wav(wav).unwrap();
        let sr = au.sample_rate;
        au.channels[0][(a * sr) as usize..(b * sr) as usize]
            .iter()
            .fold(0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn a_channel_renders_alone_and_comes_back_as_an_audio_clip() {
        let mut p = song();
        let a = ch(&p, "polaris", 0);
        let samples = BTreeMap::new();
        let j = job(&p, Source::Channel(a), Scope::Pattern(0)).unwrap();
        let wav = run(&j, &samples).unwrap();
        // channel a plays on beat 1; channel b (beat 3, at 1 s) is not in it
        assert!(level(&wav, 0.0, 0.2) > 0.01, "the channel is heard");
        assert!(level(&wav, 1.0, 1.2) < level(&wav, 0.0, 0.2) * 0.2);
        let mut samples = BTreeMap::new();
        let before = p.playlist.clips.len();
        let c = place(
            &mut p,
            &mut samples,
            wav,
            &j.name,
            0.0,
            Some(Source::Channel(a)),
        )
        .unwrap();
        assert_eq!(p.channels[c].plugin_id, "audioclip");
        assert!(p.channels[a].mute, "the source is muted");
        assert_eq!(p.playlist.clips.len(), before + 1);
        let clip = p.playlist.clips.last().unwrap();
        assert_eq!(clip.kind, ClipKind::Audio);
        assert_ne!(clip.track, 0, "on a free track");
        assert!(samples.len() == 1, "the WAV is stored in the project");
    }

    #[test]
    fn an_insert_renders_with_what_plays_into_it_and_silence_is_refused() {
        let p = song();
        let b = ch(&p, "polaris", 1);
        let insert = p.channels[b].mixer_track;
        let j = job(&p, Source::Insert(insert), Scope::Song).unwrap();
        let wav = run(&j, &BTreeMap::new()).unwrap();
        assert!(level(&wav, 1.0, 1.3) > 0.01);
        assert!(level(&wav, 0.0, 0.3) < 1e-3);
        // an insert nothing plays into
        let empty = p.mixer.len() - 1;
        let j = job(&p, Source::Insert(empty), Scope::Song).unwrap();
        assert!(run(&j, &BTreeMap::new()).is_err());
        assert!(job(&p, Source::Channel(99), Scope::Song).is_none());
    }

    #[test]
    fn selected_clips_consolidate_into_one_audio_clip() {
        let mut p = song();
        let bar = f64::from(las_project::PPQ) * 4.0;
        let pat = p.patterns[0].id.clone();
        p.playlist.clips.clear();
        for (i, at) in [0.0, bar].into_iter().enumerate() {
            p.playlist.clips.push(las_project::Clip {
                id: format!("c{i}"),
                kind: ClipKind::Pattern,
                ref_id: pat.clone(),
                track: 1 + i,
                start: at,
                length: bar,
                ..Default::default()
            });
        }
        let ids: HashSet<String> = ["c0".to_string(), "c1".to_string()].into();
        let (j, at, track) = consolidate_job(&p, &ids).unwrap();
        assert_eq!((at, track), (0.0, 1));
        let wav = run(&j, &BTreeMap::new()).unwrap();
        let au = las_engine::library::decode_wav(&wav).unwrap();
        // two bars at 120 BPM are four seconds
        assert!((au.frames() as f64 / au.sample_rate - 4.0).abs() < 0.05);
        assert!(level(&wav, 2.0, 2.2) > 0.01, "the second clip is in it");
        let mut samples = BTreeMap::new();
        replace_clips(&mut p, &mut samples, &ids, wav, at, track).unwrap();
        assert_eq!(p.playlist.clips.len(), 1);
        let c = &p.playlist.clips[0];
        assert_eq!((c.kind, c.track, c.start), (ClipKind::Audio, 1, 0.0));
        assert!((c.length - 2.0 * bar).abs() < 2.0);
        assert!(consolidate_job(&p, &HashSet::new()).is_none());
    }

    #[test]
    fn a_pattern_bounce_goes_where_the_pattern_is_first_used() {
        let mut p = song();
        assert_eq!(pattern_start(&p, 0), 0.0);
        for c in &mut p.playlist.clips {
            c.start += 384.0;
        }
        assert_eq!(pattern_start(&p, 0), 384.0);
        assert_eq!(pattern_start(&p, 7), 0.0);
    }
}
