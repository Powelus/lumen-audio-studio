// SPDX-License-Identifier: GPL-3.0-or-later
//! Interaction tests: the whole app runs headless, frame by frame, and the tests click,
//! drag and type at widgets found by their labels (through the AccessKit tree egui
//! builds), then look at what changed in the app.

mod flows;
mod glyphs;

use crate::app::App;
use eframe::egui::{self, accesskit, pos2, Event, Key, Modifiers, PointerButton, Pos2, Rect};

/// A config folder of its own for the test process, so tests never read or write the
/// user's settings, presets or autosaves.
fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("las-uitest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::set_var("APPDATA", &dir);
    });
}

/// One widget in the last frame's tree.
#[derive(Clone, Debug)]
pub struct Node {
    pub label: String,
    pub rect: Rect,
    pub role: accesskit::Role,
    pub disabled: bool,
}

pub struct Harness {
    pub ctx: egui::Context,
    pub app: App,
    frame: eframe::Frame,
    events: Vec<Event>,
    modifiers: Modifiers,
    pub nodes: Vec<Node>,
    time: f64,
    size: egui::Vec2,
    pub pointer: Pos2,
    /// Things the frames sent to the clipboard.
    pub copied: Vec<String>,
}

impl Harness {
    /// The app as it starts, with no sound device.
    pub fn new() -> Self {
        Self::sized(1600.0, 1000.0)
    }

    /// The app on a screen of `w` by `h` pixels.
    pub fn sized(w: f32, height: f32) -> Self {
        isolate();
        // each test (each runs on a thread of its own) has a config folder of its own
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("las-uitest-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::presets::TEST_CONFIG.with(|c| *c.borrow_mut() = Some(dir));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = App::new_with(&cc, false);
        let mut h = Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            events: Vec::new(),
            modifiers: Modifiers::NONE,
            nodes: Vec::new(),
            time: 0.0,
            size: egui::vec2(w, height),
            pointer: pos2(-10.0, -10.0),
            copied: Vec::new(),
        };
        h.run(3);
        h
    }

    /// The app with the demo song open.
    pub fn demo() -> Self {
        Self::demo_sized(1600.0, 1000.0)
    }

    pub fn demo_sized(w: f32, height: f32) -> Self {
        let mut h = Self::sized(w, height);
        h.app.open_demo();
        h.run(3);
        h
    }

    /// Run one frame with the events queued so far.
    pub fn step(&mut self) {
        self.time += 1.0 / 30.0;
        // the screen has a fixed size in pixels, as a monitor has; the app's zoom
        // changes how many points that is
        let ppp = self.ctx.pixels_per_point();
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size / ppp)),
            time: Some(self.time),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        if let Some(v) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            v.native_pixels_per_point = Some(1.0);
        }
        let app = &mut self.app;
        let frame = &mut self.frame;
        let ctx = self.ctx.clone();
        let mut out = self.ctx.run_ui(raw, |ui| {
            eframe::App::logic(app, &ctx, frame);
            eframe::App::ui(app, ui, frame);
        });
        out.textures_delta.clear();
        for c in &out.platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                self.copied.push(t.clone());
            }
        }
        if let Some(u) = out.platform_output.accesskit_update {
            self.nodes = u
                .nodes
                .iter()
                .filter_map(|(_, n)| {
                    let b = n.bounds()?;
                    let label = n
                        .label()
                        .map(str::to_string)
                        .or_else(|| n.value().map(str::to_string))
                        .unwrap_or_default();
                    Some(Node {
                        label,
                        rect: Rect::from_min_max(
                            pos2(b.x0 as f32, b.y0 as f32),
                            pos2(b.x1 as f32, b.y1 as f32),
                        ),
                        role: n.role(),
                        disabled: n.is_disabled(),
                    })
                })
                .collect();
        }
    }

    pub fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.step();
        }
    }

    /// Every widget whose label is `label` (spaces collapsed, so menu entries with
    /// padded shortcuts are found by their words).
    pub fn find_all(&self, label: &str) -> Vec<Node> {
        let want = squash(label);
        self.nodes
            .iter()
            .filter(|n| squash(&n.label) == want)
            .cloned()
            .collect()
    }

    /// Widgets whose label starts with `prefix`.
    pub fn find_prefix(&self, prefix: &str) -> Vec<Node> {
        let want = squash(prefix);
        self.nodes
            .iter()
            .filter(|n| squash(&n.label).starts_with(&want))
            .cloned()
            .collect()
    }

    pub fn has(&self, label: &str) -> bool {
        !self.find_all(label).is_empty()
    }

    /// The one widget labelled `label` (the topmost, last drawn, if there are several).
    pub fn get(&self, label: &str) -> Node {
        let all = self.find_all(label);
        match all.last() {
            Some(n) => n.clone(),
            None => {
                let mut labels: Vec<_> = self
                    .nodes
                    .iter()
                    .filter(|n| !n.label.is_empty())
                    .map(|n| n.label.clone())
                    .collect();
                labels.sort();
                labels.dedup();
                panic!("no widget labelled {label:?}; there are: {labels:?}")
            }
        }
    }

    /// Hold (or let go of) Ctrl, Shift and Alt from the next frame on.
    pub fn set_modifiers(&mut self, m: Modifiers) {
        if m != self.modifiers {
            self.modifiers = m;
            self.events.push(Event::ModifiersChanged(m));
        }
    }

    pub fn hover(&mut self, at: Pos2) {
        self.pointer = at;
        self.events.push(Event::PointerMoved(at));
        self.step();
    }

    pub fn click_at(&mut self, at: Pos2) {
        self.click_with(at, PointerButton::Primary, Modifiers::NONE);
    }

    pub fn click_with(&mut self, at: Pos2, button: PointerButton, modifiers: Modifiers) {
        self.set_modifiers(modifiers);
        self.hover(at);
        self.press(at, button, modifiers, true);
        self.step();
        self.press(at, button, modifiers, false);
        self.step();
        self.set_modifiers(Modifiers::NONE);
        // a popup opened by the click settles its place over a frame or two
        self.run(3);
    }

    fn press(&mut self, at: Pos2, button: PointerButton, modifiers: Modifiers, pressed: bool) {
        self.events.push(Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers,
        });
    }

    pub fn double_click_at(&mut self, at: Pos2) {
        self.hover(at);
        for _ in 0..2 {
            self.press(at, PointerButton::Primary, Modifiers::NONE, true);
            self.step();
            self.press(at, PointerButton::Primary, Modifiers::NONE, false);
            self.step();
        }
        self.step();
    }

    pub fn right_click_at(&mut self, at: Pos2) {
        self.click_with(at, PointerButton::Secondary, Modifiers::NONE);
    }

    /// Press at `from`, move to `to` in steps, let go.
    pub fn drag(&mut self, from: Pos2, to: Pos2) {
        self.drag_with(from, to, PointerButton::Primary, Modifiers::NONE);
    }

    pub fn drag_with(&mut self, from: Pos2, to: Pos2, button: PointerButton, m: Modifiers) {
        self.set_modifiers(m);
        self.hover(from);
        self.press(from, button, m, true);
        self.step();
        for i in 1..=8 {
            let p = from + (to - from) * (i as f32 / 8.0);
            self.pointer = p;
            self.events.push(Event::PointerMoved(p));
            self.step();
        }
        self.press(to, button, m, false);
        self.step();
        self.set_modifiers(Modifiers::NONE);
        self.step();
    }

    /// Click the widget labelled `label`.
    pub fn click(&mut self, label: &str) {
        let n = self.get(label);
        assert!(!n.disabled, "{label:?} is disabled");
        self.click_at(n.rect.center());
    }

    pub fn double_click(&mut self, label: &str) {
        let n = self.get(label);
        self.double_click_at(n.rect.center());
    }

    pub fn right_click(&mut self, label: &str) {
        let n = self.get(label);
        self.right_click_at(n.rect.center());
    }

    /// Open the menu `path[0]` of the menu bar and click through the rest.
    pub fn menu(&mut self, path: &[&str]) {
        for (i, label) in path.iter().enumerate() {
            let n = self.get(label);
            if i + 1 < path.len() {
                // sub-menus open on click or on hover
                self.click_at(n.rect.center());
                if !self.has(path[i + 1]) {
                    self.hover(n.rect.center());
                    self.run(2);
                }
            } else {
                assert!(!n.disabled, "menu entry {label:?} is disabled");
                self.click_at(n.rect.center());
            }
        }
        self.run(2);
    }

    pub fn key_with(&mut self, key: Key, modifiers: Modifiers) {
        self.set_modifiers(modifiers);
        for pressed in [true, false] {
            self.events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            });
            self.step();
        }
        self.set_modifiers(Modifiers::NONE);
        self.step();
    }

    pub fn key(&mut self, key: Key) {
        self.key_with(key, Modifiers::NONE);
    }

    pub fn type_text(&mut self, text: &str) {
        self.events.push(Event::Text(text.to_string()));
        self.step();
        self.step();
    }

    pub fn project(&self) -> &las_project::Project {
        &self
            .app
            .project
            .loaded
            .as_ref()
            .expect("a project is open")
            .project
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
