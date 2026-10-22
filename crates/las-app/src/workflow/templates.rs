// SPDX-License-Identifier: GPL-3.0-or-later
//! Project templates (File > New from template) and crash safety: an autosave every
//! couple of minutes, and on the next start after a crash an offer to restore it.

use las_project::create::{add_channel, default_project, empty_project, pattern, uid};
use las_project::{Clip, ClipKind, Project, PPQ};
use std::path::{Path, PathBuf};

/// The templates that come with the app: name and what it is for.
pub const BUILT_IN: [(&str, &str); 3] = [
    ("Empty", "No channels: start from silence"),
    (
        "Beat",
        "Kick, clap, hat, snare and a synth, with a four on the floor",
    ),
    (
        "Song",
        "Drums, bass and chords, with intro, verse and chorus patterns arranged",
    ),
];

/// A built-in template as a fresh project.
pub fn built_in(index: usize) -> Project {
    match index {
        0 => empty_project("Untitled"),
        1 => default_project(),
        _ => song(),
    }
}

fn song() -> Project {
    let mut p = default_project();
    p.name = "Untitled song".into();
    let bass = add_channel(&mut p, "monolith");
    p.channels[bass].name = "Bass".into();
    let chords = add_channel(&mut p, "polaris");
    p.channels[chords].name = "Chords".into();
    for (c, name) in [(bass, "Bass"), (chords, "Chords")] {
        let t = p.channels[c].mixer_track;
        p.mixer[t].name = name.into();
    }
    let (bass_id, chords_id) = (p.channels[bass].id.clone(), p.channels[chords].id.clone());
    let drums = p.patterns[0].clone();
    let beat = f64::from(PPQ);
    let note = |key: i32, pos: f64, len: f64| las_project::Note {
        id: uid("n"),
        key,
        pos,
        len,
        vel: 0.8,
        ..Default::default()
    };
    // the first pattern stays the beat; the others are made around it
    p.patterns[0].name = "Drums".into();
    let mut bassline = pattern(1, Some("Bass"));
    bassline.notes.insert(
        bass_id,
        [36, 36, 43, 41]
            .iter()
            .enumerate()
            .map(|(i, &k)| note(k, i as f64 * beat, beat * 0.9))
            .collect(),
    );
    let mut chord = pattern(2, Some("Chords"));
    chord.notes.insert(
        chords_id,
        [60, 64, 67]
            .iter()
            .map(|&k| note(k, 0.0, beat * 4.0))
            .collect(),
    );
    let ids = [drums.id.clone(), bassline.id.clone(), chord.id.clone()];
    p.patterns.push(bassline);
    p.patterns.push(chord);
    // intro: drums; verse: drums and bass; chorus: everything
    p.playlist.clips.clear();
    let bar = beat * 4.0;
    let sections: [(&[usize], f64, f64); 3] = [
        (&[0], 0.0, 4.0),
        (&[0, 1], 4.0, 8.0),
        (&[0, 1, 2], 12.0, 8.0),
    ];
    for (uses, at, bars) in sections {
        for &u in uses {
            for b in 0..bars as usize {
                p.playlist.clips.push(Clip {
                    id: uid("clip_"),
                    kind: ClipKind::Pattern,
                    ref_id: ids[u].clone(),
                    track: u,
                    start: (at + b as f64) * bar,
                    length: bar,
                    ..Default::default()
                });
            }
        }
    }
    for (i, name) in ["Drums", "Bass", "Chords"].iter().enumerate() {
        p.playlist.tracks[i].name = (*name).into();
    }
    p
}

/// Where saved templates and the autosave live.
pub fn dir(sub: &str) -> Option<PathBuf> {
    Some(crate::presets::config_dir()?.join(sub))
}

/// The saved templates in `dir`: name and file, by name.
pub fn user_templates(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "las"))
        .map(|p| {
            let name = p
                .file_stem()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned());
            (name, p)
        })
        .collect();
    out.sort();
    out
}

/// A file name made safe from a template name.
pub fn file_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.trim().is_empty() {
        "Template".into()
    } else {
        s
    }
}

/// A project made from a template is a new one: fresh id, never saved.
pub fn fresh(mut p: Project) -> Project {
    p.id = uid("prj_");
    let now = las_project::create::now_ms();
    p.meta.created_at = now;
    p.meta.updated_at = now;
    p
}

/// Crash safety. A lock file says the app is running; it is removed on a clean exit.
/// Found at start, it means the last session ended without that, and the autosave
/// (if any) is offered back.
pub struct Safety {
    pub dir: PathBuf,
    /// The last session did not end cleanly and left an autosave.
    pub crashed: bool,
}

pub const AUTOSAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(120);

impl Safety {
    /// Look at `dir` and take the lock for this session.
    pub fn start(dir: PathBuf) -> Self {
        let lock = dir.join("running.lock");
        let crashed = lock.exists() && dir.join("autosave.las").exists();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&lock, std::process::id().to_string());
        Self { dir, crashed }
    }

    pub fn autosave_path(&self) -> PathBuf {
        self.dir.join("autosave.las")
    }

    /// Write the autosave (atomically).
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let tmp = self.dir.join("autosave.las.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.autosave_path()).map_err(|e| e.to_string())
    }

    /// A clean exit: no crash to report next time.
    pub fn finish(&self) {
        let _ = std::fs::remove_file(self.dir.join("running.lock"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_templates_are_projects_that_play() {
        assert!(built_in(0).channels.is_empty());
        let beat = built_in(1);
        assert!(beat.channels.len() >= 4);
        let s = built_in(2);
        assert_eq!(s.patterns.len(), 3);
        assert_eq!(s.playlist.tracks[1].name, "Bass");
        // twenty bars: intro 4, verse 8, chorus 8
        let bar = f64::from(PPQ) * 4.0;
        let end = s
            .playlist
            .clips
            .iter()
            .map(|c| c.start + c.length)
            .fold(0.0, f64::max);
        assert_eq!(end, 20.0 * bar);
        let r = las_engine::render_mix(
            &s,
            &las_engine::RenderOptions {
                sample_rate: 22050.0,
                tail: 0.0,
                mode: las_engine::Mode::Range {
                    from: 12.0 * bar,
                    to: 13.0 * bar,
                },
            },
        )
        .unwrap();
        assert!(r.left.iter().any(|v| v.abs() > 0.01), "the chorus sounds");
        // a template saved and loaded comes back, as a new project
        let bytes = las_project::encode(&s, &Default::default(), true).unwrap();
        let back = fresh(las_project::decode(&bytes).unwrap().project);
        assert_eq!(back.patterns.len(), 3);
        assert_ne!(back.id, s.id);
    }

    #[test]
    fn template_names_become_safe_file_names() {
        assert_eq!(file_name("My / beat"), "My _ beat");
        assert_eq!(file_name("  "), "Template");
    }

    #[test]
    fn a_session_that_did_not_end_cleanly_offers_its_autosave() {
        let dir = std::env::temp_dir().join(format!("lumen-safety-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // first run: nothing to restore; it autosaves, then crashes (no finish)
        let a = Safety::start(dir.clone());
        assert!(!a.crashed);
        a.write(b"project").unwrap();
        let b = Safety::start(dir.clone());
        assert!(b.crashed, "the lock was left behind");
        assert_eq!(std::fs::read(b.autosave_path()).unwrap(), b"project");
        // this one ends cleanly: the next start has nothing to offer
        b.finish();
        let c = Safety::start(dir.clone());
        assert!(!c.crashed);
        c.finish();
        assert!(user_templates(&dir).iter().any(|(n, _)| n == "autosave"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
