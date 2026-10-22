// SPDX-License-Identifier: GPL-3.0-or-later
//! MIDI keyboard input. Keys go straight to the audio thread (to the project's selected
//! channel, or the synth), so playing never waits for the screen. The interface also
//! gets every key and controller, for recording notes and for linked controls.

use crate::audio::{Command, MidiSender};
use las_dsp::engine::Cc;
use midir::{Ignore, MidiInput, MidiInputConnection};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the interface hears from MIDI input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiEvent {
    Note {
        key: u8,
        vel: f64,
        on: bool,
    },
    /// A controller (knob, slider) moved: MIDI channel 0 to 15, number, value 0 to 127.
    Control {
        channel: u8,
        cc: u8,
        value: u8,
    },
}

/// No channel: keys play the synth.
const SYNTH: usize = usize::MAX;

/// What the MIDI threads and the interface share.
struct Shared {
    audio: Mutex<Option<MidiSender>>,
    events: Mutex<Vec<MidiEvent>>,
    /// The project channel keys play, or [`SYNTH`].
    channel: AtomicUsize,
    /// When the last message came, ms after `epoch`, plus one (0 for never).
    last: AtomicU64,
    epoch: Instant,
    repaint: Mutex<Option<eframe::egui::Context>>,
}

/// MIDI input: the ports listened to.
pub struct MidiHub {
    shared: Arc<Shared>,
    conns: Vec<MidiInputConnection<()>>,
    /// The names of the ports connected.
    pub names: Vec<String>,
    /// Why nothing could be connected, if so.
    pub problem: Option<String>,
}

/// The names of the MIDI inputs there are.
pub fn ports() -> Vec<String> {
    let Ok(input) = MidiInput::new("Lumen Audio Studio") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

/// The ports to connect for a choice: "" is all of them, "none" none, otherwise the
/// port of that name (when it is there).
pub fn chosen<'a>(all: &'a [String], choice: &str) -> Vec<&'a String> {
    match choice {
        "" => all.iter().collect(),
        "none" => Vec::new(),
        name => all.iter().filter(|p| *p == name).collect(),
    }
}

impl MidiHub {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                audio: Mutex::new(None),
                events: Mutex::new(Vec::new()),
                channel: AtomicUsize::new(SYNTH),
                last: AtomicU64::new(0),
                epoch: Instant::now(),
                repaint: Mutex::new(None),
            }),
            conns: Vec::new(),
            names: Vec::new(),
            problem: None,
        }
    }

    /// Where keys go on the audio thread (a new queue after the output restarts).
    pub fn set_sender(&self, tx: Option<MidiSender>) {
        if let Ok(mut a) = self.shared.audio.lock() {
            *a = tx;
        }
    }

    /// Wake the interface when a message comes.
    pub fn set_repaint(&self, ctx: &eframe::egui::Context) {
        if let Ok(mut r) = self.shared.repaint.lock() {
            if r.is_none() {
                *r = Some(ctx.clone());
            }
        }
    }

    /// The project channel live keys play, or `None` for the synth.
    pub fn set_channel(&self, channel: Option<usize>) {
        self.shared
            .channel
            .store(channel.unwrap_or(SYNTH), Ordering::Relaxed);
    }

    /// Listen to the inputs `choice` names ("" for all).
    pub fn connect(&mut self, choice: &str) {
        self.conns.clear();
        self.names.clear();
        self.problem = None;
        let all = ports();
        if all.is_empty() {
            self.problem = Some("No MIDI inputs found".into());
            return;
        }
        for name in chosen(&all, choice) {
            let Ok(mut input) = MidiInput::new("Lumen Audio Studio") else {
                continue;
            };
            input.ignore(Ignore::SysexAndTime);
            let Some(port) = input
                .ports()
                .into_iter()
                .find(|p| input.port_name(p).ok().as_ref() == Some(name))
            else {
                continue;
            };
            let shared = self.shared.clone();
            match input.connect(&port, "lumen-input", move |_, msg, _| shared.take(msg), ()) {
                Ok(c) => {
                    self.conns.push(c);
                    self.names.push(name.clone());
                }
                Err(e) => self.problem = Some(format!("{name}: {e}")),
            }
        }
        if self.names.is_empty() && self.problem.is_none() {
            self.problem = Some(if choice == "none" {
                "MIDI input is off".into()
            } else {
                format!("{choice} is not connected")
            });
        }
    }

    /// What came in since the last call.
    pub fn drain(&self) -> Vec<MidiEvent> {
        self.shared
            .events
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }

    /// Whether a message came in the last moment (for the activity light).
    pub fn active(&self) -> bool {
        let last = self.shared.last.load(Ordering::Relaxed);
        last > 0 && {
            let now = self.shared.epoch.elapsed().as_millis() as u64 + 1;
            now.saturating_sub(last) < 150
        }
    }

    /// For tests: a message as if it came from a port.
    #[cfg(test)]
    fn feed(&self, msg: &[u8]) {
        self.shared.take(msg);
    }
}

impl Shared {
    /// A message from a port: keys to the audio thread at once, and everything to the
    /// interface.
    fn take(&self, msg: &[u8]) {
        let ms = self.epoch.elapsed().as_millis() as u64 + 1;
        self.last.store(ms, Ordering::Relaxed);
        let event = event(msg);
        if let Some(c) = parse(msg) {
            let ch = self.channel.load(Ordering::Relaxed);
            let c = match (c, event) {
                (_, Some(MidiEvent::Note { key, vel, on })) if ch != SYNTH => Command::PlayerNote {
                    channel: ch,
                    key,
                    vel,
                    on,
                },
                (Command::Controller { cc, value }, _) if ch != SYNTH => {
                    Command::PlayerController {
                        channel: ch,
                        cc,
                        value,
                    }
                }
                (c, _) => c,
            };
            if let Ok(mut a) = self.audio.lock() {
                if let Some(tx) = a.as_mut() {
                    let _ = tx.push(c);
                }
            }
        }
        if let Some(e) = event {
            if let Ok(mut q) = self.events.lock() {
                // nobody reading for a long while: keep the newest
                if q.len() > 4096 {
                    q.drain(..2048);
                }
                q.push(e);
            }
        }
        if let Ok(r) = self.repaint.lock() {
            if let Some(ctx) = r.as_ref() {
                ctx.request_repaint_after(Duration::from_millis(5));
            }
        }
    }
}

/// The keys and controllers of a message, for the interface.
pub fn event(msg: &[u8]) -> Option<MidiEvent> {
    let (status, a, b) = (
        *msg.first()?,
        *msg.get(1)?,
        msg.get(2).copied().unwrap_or(0),
    );
    match status & 0xF0 {
        0x90 if b > 0 => Some(MidiEvent::Note {
            key: a,
            vel: f64::from(b) / 127.0,
            on: true,
        }),
        0x90 | 0x80 => Some(MidiEvent::Note {
            key: a,
            vel: 0.0,
            on: false,
        }),
        0xB0 if a < 120 => Some(MidiEvent::Control {
            channel: status & 0x0F,
            cc: a,
            value: b.min(127),
        }),
        _ => None,
    }
}

/// Turn a MIDI message into a command (notes, pitch bend, mod wheel, pressure).
pub fn parse(msg: &[u8]) -> Option<Command> {
    let (status, a, b) = (
        *msg.first()?,
        *msg.get(1)?,
        msg.get(2).copied().unwrap_or(0),
    );
    match status & 0xF0 {
        0x90 if b > 0 => Some(Command::NoteOn {
            key: a,
            vel: b as f64 / 127.0,
        }),
        0x90 | 0x80 => Some(Command::NoteOff { key: a }),
        0xD0 => Some(Command::Controller {
            cc: Cc::At,
            value: a as f64 / 127.0,
        }),
        0xE0 => {
            let v = (((b as u16) << 7) | a as u16) as f64;
            Some(Command::Controller {
                cc: Cc::Pb,
                value: (v - 8192.0) / 8192.0,
            })
        }
        0xB0 => match a {
            1 => Some(Command::Controller {
                cc: Cc::Mw,
                value: b as f64 / 127.0,
            }),
            123 => Some(Command::AllOff),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_with_zero_velocity_is_a_note_off() {
        assert!(matches!(
            parse(&[0x90, 60, 0]),
            Some(Command::NoteOff { key: 60 })
        ));
        assert!(matches!(
            parse(&[0x90, 60, 127]),
            Some(Command::NoteOn { key: 60, .. })
        ));
    }

    #[test]
    fn pitch_bend_centre_is_zero() {
        match parse(&[0xE0, 0, 64]) {
            Some(Command::Controller { cc: Cc::Pb, value }) => assert!(value.abs() < 1e-9),
            _ => panic!("expected a pitch bend"),
        }
    }

    #[test]
    fn controllers_and_keys_reach_the_interface() {
        assert_eq!(
            event(&[0xB3, 74, 100]),
            Some(MidiEvent::Control {
                channel: 3,
                cc: 74,
                value: 100
            })
        );
        assert_eq!(
            event(&[0x80, 60, 10]),
            Some(MidiEvent::Note {
                key: 60,
                vel: 0.0,
                on: false
            })
        );
        // "all notes off" and the like are not knobs
        assert_eq!(event(&[0xB0, 123, 0]), None);
        assert_eq!(event(&[0xF8]), None);
    }

    #[test]
    fn keys_play_the_selected_channel_and_the_interface_hears_them() {
        let hub = MidiHub::new();
        let (tx, mut rx) = rtrb::RingBuffer::<Command>::new(16);
        hub.set_sender(Some(tx));
        hub.set_channel(Some(2));
        hub.feed(&[0x90, 64, 127]);
        assert!(matches!(
            rx.pop(),
            Ok(Command::PlayerNote {
                channel: 2,
                key: 64,
                on: true,
                ..
            })
        ));
        // the mod wheel goes to the selected channel too
        hub.feed(&[0xB0, 1, 127]);
        assert!(matches!(
            rx.pop(),
            Ok(Command::PlayerController {
                channel: 2,
                cc: Cc::Mw,
                ..
            })
        ));
        hub.set_channel(None);
        hub.feed(&[0x90, 65, 127]);
        assert!(matches!(rx.pop(), Ok(Command::NoteOn { key: 65, .. })));
        hub.feed(&[0xB0, 20, 5]);
        assert!(rx.pop().is_err(), "a plain knob is not for the synth");
        let got = hub.drain();
        assert_eq!(got.len(), 4, "the keys, the mod wheel and the knob");
        assert!(hub.drain().is_empty());
        assert!(hub.active());
    }

    #[test]
    fn the_inputs_chosen_are_all_one_or_none() {
        let all = vec!["A".to_string(), "B".to_string()];
        assert_eq!(chosen(&all, "").len(), 2);
        assert_eq!(chosen(&all, "B"), vec![&all[1]]);
        assert!(chosen(&all, "C").is_empty());
        assert!(chosen(&all, "none").is_empty());
    }

    #[test]
    fn short_or_unknown_messages_are_ignored() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[0x90]).is_none());
        assert!(parse(&[0xF8]).is_none());
    }
}
