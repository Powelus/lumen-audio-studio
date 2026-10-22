// SPDX-License-Identifier: GPL-3.0-or-later
//! Bringing windows to the front. Every path that opens a window (a menu, the top
//! bar, a double click, a command) calls [`raise`]; the window takes the request when
//! it is next drawn and egui moves its layer to the top, opening it out if it was
//! collapsed. egui keeps a closed window's place in the stacking order, so without
//! this a reopened window comes back behind the others.

use crate::shell::Windows;
use eframe::egui;
use std::cell::RefCell;

/// The app's windows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Win {
    Rack,
    Playlist,
    Roll,
    Mixer,
    Synth,
    Export,
    About,
    Keys,
    History,
    Settings,
    Analyser,
    ChanSettings,
    Effect,
    SampleEditor,
    Automation,
}

impl Win {
    /// The egui id of the window (the instrument window's is given by its face).
    pub fn id(self) -> egui::Id {
        egui::Id::unique(("las-window", self as u8))
    }
}

thread_local! {
    static PENDING: RefCell<Vec<Win>> = const { RefCell::new(Vec::new()) };
}

/// Ask for window `w` to come to the front when it is next drawn.
pub fn raise(w: Win) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if !p.contains(&w) {
            p.push(w);
        }
    });
}

/// The windows waiting to come to the front.
#[cfg(test)]
pub fn pending() -> Vec<Win> {
    PENDING.with(|p| p.borrow().clone())
}

/// Forget the requests (tests start from none).
#[cfg(test)]
pub fn clear() {
    PENDING.with(|p| p.borrow_mut().clear());
}

/// Take the request for `w`, if there is one.
pub fn take(w: Win) -> bool {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let had = p.contains(&w);
        p.retain(|x| *x != w);
        had
    })
}

/// Bring window `w`, whose egui id is `id`, to the front if it was asked for.
pub fn front(ctx: &egui::Context, w: Win, id: egui::Id) {
    if take(w) {
        bring(ctx, id);
    }
}

/// Move the window with id `id` to the top and open it out if it was collapsed.
pub fn bring(ctx: &egui::Context, id: egui::Id) {
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
    let mut c = egui::collapsing_header::CollapsingState::load_with_default_open(
        ctx,
        id.with("collapsing"),
        true,
    );
    if !c.is_open() {
        c.set_open(true);
        c.store(ctx);
    }
    ctx.request_repaint();
}

/// Whether the window with id `id` is drawn but has another window over its middle
/// or its title bar.
pub fn covered(ctx: &egui::Context, id: egui::Id) -> bool {
    let layer = egui::LayerId::new(egui::Order::Middle, id);
    let Some(r) = ctx.memory(|m| m.area_rect(id)) else {
        return false;
    };
    [r.center(), egui::pos2(r.center().x, r.top() + 8.0)]
        .into_iter()
        .any(|p| {
            ctx.layer_id_at(p)
                .is_some_and(|l| l != layer && l.order == egui::Order::Middle)
        })
}

/// The windows open in `now` that were closed in `before`: whatever opened them,
/// they come to the front.
pub fn newly_opened(before: &Windows, now: &Windows) -> Vec<Win> {
    let mut out = Vec::new();
    macro_rules! each {
        ($($f:ident => $w:ident),*) => {$(
            if now.$f && !before.$f {
                out.push(Win::$w);
            }
        )*};
    }
    each!(rack => Rack, playlist => Playlist, roll => Roll, mixer => Mixer, synth => Synth,
        export => Export, about => About, keys => Keys, history => History,
        settings => Settings, analyser => Analyser);
    out
}

impl Windows {
    /// Open window `w` (one of those with a flag here) and bring it to the front.
    pub fn show(&mut self, w: Win) {
        match w {
            Win::Rack => self.rack = true,
            Win::Playlist => self.playlist = true,
            Win::Roll => self.roll = true,
            Win::Mixer => self.mixer = true,
            Win::Synth => self.synth = true,
            Win::Export => self.export = true,
            Win::About => self.about = true,
            Win::Keys => self.keys = true,
            Win::History => self.history = true,
            Win::Settings => self.settings = true,
            Win::Analyser => self.analyser = true,
            _ => {}
        }
        raise(w);
    }

    /// The top bar's buttons and the View menu, as FL Studio's F keys: a closed
    /// window opens and comes to the front, one open but `covered` by another comes to
    /// the front, and one open on top closes.
    pub fn toggle(&mut self, w: Win, covered: bool) {
        let open = match w {
            Win::Rack => &mut self.rack,
            Win::Playlist => &mut self.playlist,
            Win::Roll => &mut self.roll,
            Win::Mixer => &mut self.mixer,
            Win::Synth => &mut self.synth,
            Win::Analyser => &mut self.analyser,
            _ => return self.show(w),
        };
        if *open && !covered {
            *open = false;
        } else {
            self.show(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Windows {
        Windows {
            rack: false,
            playlist: false,
            roll: false,
            mixer: false,
            synth: false,
            export: false,
            about: false,
            keys: false,
            history: false,
            settings: false,
            analyser: false,
        }
    }

    #[test]
    fn opening_a_window_asks_for_it_to_come_to_the_front() {
        clear();
        let mut w = none();
        w.show(Win::Mixer);
        assert!(w.mixer);
        assert_eq!(pending(), vec![Win::Mixer]);
        // already open: asked again, still once
        w.show(Win::Mixer);
        assert_eq!(pending(), vec![Win::Mixer]);
        assert!(take(Win::Mixer));
        assert!(!take(Win::Mixer));
        // the top bar toggles: open raises, close does not
        w.toggle(Win::Roll, false);
        assert!(w.roll);
        assert_eq!(pending(), vec![Win::Roll]);
        clear();
        // open behind another window: it comes to the front and stays open
        w.toggle(Win::Roll, true);
        assert!(w.roll);
        assert_eq!(pending(), vec![Win::Roll]);
        clear();
        w.toggle(Win::Roll, false);
        assert!(!w.roll);
        assert!(pending().is_empty());
    }

    #[test]
    fn a_window_opened_by_its_flag_alone_is_found() {
        let before = none();
        let mut now = none();
        now.playlist = true;
        now.settings = true;
        assert_eq!(
            newly_opened(&before, &now),
            vec![Win::Playlist, Win::Settings]
        );
        assert!(newly_opened(&now, &now).is_empty());
    }
}
