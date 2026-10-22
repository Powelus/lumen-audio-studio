// SPDX-License-Identifier: GPL-3.0-or-later
//! MIDI keyboard input.

use crate::audio::{Command, MidiSender};
use las_dsp::engine::Cc;
use midir::{Ignore, MidiInput, MidiInputConnection};

/// Connect to the first MIDI input port, if there is one. Keep the returned
/// value alive to stay connected.
pub fn connect(
    mut tx: MidiSender,
    log: crate::workflow::capture::SharedLog,
) -> Option<(MidiInputConnection<()>, String)> {
    let mut input = MidiInput::new("Lumen Audio Studio").ok()?;
    input.ignore(Ignore::SysexAndTime);
    let port = input.ports().into_iter().next()?;
    let name = input
        .port_name(&port)
        .unwrap_or_else(|_| "MIDI input".into());
    let conn = input
        .connect(
            &port,
            "lumen-input",
            move |_, msg, _| {
                if let Some(c) = parse(msg) {
                    // remembered for Capture MIDI
                    if let (Command::NoteOn { key, vel }, Ok(mut l)) = (&c, log.lock()) {
                        l.push(*key, *vel, true);
                    } else if let (Command::NoteOff { key }, Ok(mut l)) = (&c, log.lock()) {
                        l.push(*key, 0.0, false);
                    }
                    let _ = tx.push(c);
                }
            },
            (),
        )
        .ok()?;
    Some((conn, name))
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
    fn short_or_unknown_messages_are_ignored() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[0x90]).is_none());
        assert!(parse(&[0xF8]).is_none());
    }
}
