// SPDX-License-Identifier: GPL-3.0-or-later
//! The command palette (Ctrl+K), like the quick search of Ableton, Bitwig and Studio
//! One: type a few letters of anything (a menu action, a window, an instrument, an
//! effect, a preset, a sample or a recent project) and press Enter.

/// What an entry of the palette does.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    New,
    Open,
    Demo,
    Save,
    SaveAs,
    Import,
    Export,
    Undo,
    Redo,
    History,
    NewPattern,
    /// Show or hide a window: playlist, rack, roll, mixer, synth, browser, tuner,
    /// reference.
    Window(&'static str),
    Shortcuts,
    About,
    Capture,
    Consolidate,
    SaveTemplate,
    /// A built-in template by index, or a saved one by path.
    Template(usize),
    UserTemplate(String),
    /// Render the selected channel to an audio clip.
    RenderChannel,
    AddInstrument(String),
    /// Put an effect on the selected mixer insert.
    AddEffect(String),
    /// A synth preset: instrument (Facet 0, Mist 1, Polaris 2) and its index.
    Preset(usize, usize),
    /// A factory sample, added as a sampler channel.
    Sample(String, String),
    Recent(String),
    Theme(String),
}

/// One entry: what it says, what kind it is (shown on the right), what it does.
#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub kind: &'static str,
    pub hint: String,
    pub cmd: Cmd,
}

impl Item {
    pub fn new(label: impl Into<String>, kind: &'static str, cmd: Cmd) -> Self {
        Self {
            label: label.into(),
            kind,
            hint: String::new(),
            cmd,
        }
    }

    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = h.into();
        self
    }
}

/// The menu actions and windows, which are always there.
pub fn actions() -> Vec<Item> {
    let a = |l: &str, c: Cmd, h: &str| Item::new(l, "Action", c).hint(h);
    let w = |l: &str, id: &'static str, h: &str| Item::new(l, "Window", Cmd::Window(id)).hint(h);
    vec![
        a("New project", Cmd::New, "Ctrl+N"),
        a("Open project...", Cmd::Open, "Ctrl+O"),
        a("Open demo song", Cmd::Demo, ""),
        a("Save", Cmd::Save, "Ctrl+S"),
        a("Save as...", Cmd::SaveAs, ""),
        a("Import samples...", Cmd::Import, ""),
        a("Export audio...", Cmd::Export, "Ctrl+R"),
        a("Undo", Cmd::Undo, "Ctrl+Z"),
        a("Redo", Cmd::Redo, "Ctrl+Y"),
        a("History", Cmd::History, ""),
        a("New pattern", Cmd::NewPattern, "F4"),
        a(
            "Capture MIDI (notes just played)",
            Cmd::Capture,
            "Ctrl+Shift+C",
        ),
        a("Consolidate selected clips", Cmd::Consolidate, "playlist"),
        a(
            "Render channel to audio clip",
            Cmd::RenderChannel,
            "selected channel",
        ),
        a("Save as template...", Cmd::SaveTemplate, ""),
        a("Keyboard shortcuts", Cmd::Shortcuts, "F1"),
        a("About", Cmd::About, ""),
        w("Playlist", "playlist", "F5"),
        w("Channel rack", "rack", "F6"),
        w("Piano roll", "roll", "F7"),
        w("Mixer", "mixer", "F9"),
        w("Browser", "browser", "F8"),
        w("Synth", "synth", ""),
        w("Tuner", "tuner", ""),
        w("Reference track (A/B)", "reference", ""),
    ]
}

/// How well `query` matches `text`: `None` when its letters do not all appear in
/// order. Higher is better: letters in a row, at word starts and early count more.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut s = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in t.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            let word_start = i == 0 || !t[i - 1].is_alphanumeric();
            s += 1;
            if word_start {
                s += 8;
            }
            if last.is_some_and(|l| l + 1 == i) {
                s += 5;
            }
            if i == 0 {
                s += 4;
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // shorter texts and early matches win ties
    Some(s * 4 - t.len() as i32 / 4 - last.unwrap_or(0) as i32 / 8)
}

/// The entries matching `query`, best first (all of them, in order, for no query).
pub fn search(items: &[Item], query: &str, limit: usize) -> Vec<usize> {
    let mut hits: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let best = score(query, &it.label)?;
            Some((best, i))
        })
        .collect();
    if !query.trim().is_empty() {
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    hits.into_iter().take(limit).map(|(_, i)| i).collect()
}

/// What the palette remembers while it is open.
#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// Focus the text box on the next frame.
    pub focus: bool,
}

impl Palette {
    pub fn show(&mut self, query: &str) {
        self.open = true;
        self.query = query.to_string();
        self.selected = 0;
        self.focus = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_in_order_match_and_others_do_not() {
        assert!(score("mx", "Mixer").is_some());
        assert!(score("pr", "Piano roll").is_some());
        assert!(score("xm", "Mixer").is_none());
        assert!(score("", "anything") == Some(0));
        assert!(score("MIX", "mixer").is_some(), "case does not matter");
    }

    #[test]
    fn word_starts_and_runs_rank_first() {
        let items: Vec<Item> = [
            "Reverb",
            "Piano roll",
            "Playlist",
            "Phaser",
            "Open project...",
        ]
        .iter()
        .map(|l| Item::new(*l, "x", Cmd::New))
        .collect();
        let hits = search(&items, "pr", 10);
        assert_eq!(
            items[hits[0]].label, "Piano roll",
            "P and R both start words"
        );
        assert!(!hits.iter().any(|&i| items[i].label == "Playlist"));
        let hits = search(&items, "rev", 10);
        assert_eq!(items[hits[0]].label, "Reverb");
        assert_eq!(search(&items, "", 3), vec![0, 1, 2], "everything, in order");
    }

    #[test]
    fn every_menu_action_and_window_is_findable() {
        let items = actions();
        for q in [
            "capture",
            "tuner",
            "consolidate",
            "template",
            "export",
            "mixer",
            "a/b",
        ] {
            assert!(!search(&items, q, 5).is_empty(), "{q}");
        }
        let hits = search(&items, "cap", 1);
        assert_eq!(items[hits[0]].cmd, Cmd::Capture);
    }
}
