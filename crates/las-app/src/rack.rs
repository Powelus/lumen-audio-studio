// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel colours, shared by the rack, the roll and the playlist.

use crate::theme::accent;
use eframe::egui::Color32;
use las_project::Channel;

/// A channel's colour (`#rrggbb`), or the accent when it has none.
pub fn channel_colour(c: &Channel) -> Color32 {
    channel_colour_of(&c.color)
}

/// A `#rrggbb` colour, or the accent when it is not one.
pub fn channel_colour_of(color: &str) -> Color32 {
    // the violet that new projects of the web version are saved with follows the theme
    if color.eq_ignore_ascii_case("#7c5cff") {
        return accent();
    }
    let h = color.trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    accent()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_default_violet_follows_the_theme() {
        let _g = crate::theme::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ctx = eframe::egui::Context::default();
        crate::theme::apply(&ctx, "fl");
        assert_eq!(channel_colour_of("#7c5cff"), crate::theme::accent());
        assert_ne!(
            channel_colour_of("#7C5CFF"),
            Color32::from_rgb(0x7c, 0x5c, 0xff)
        );
        // other colours are what they say
        assert_eq!(
            channel_colour_of("#ff5c8a"),
            Color32::from_rgb(0xff, 0x5c, 0x8a)
        );
    }

    #[test]
    fn colours_parse_and_fall_back() {
        let with = |color: &str| Channel {
            color: color.into(),
            ..Default::default()
        };
        assert_eq!(
            channel_colour(&with("#10ff20")),
            Color32::from_rgb(0x10, 0xff, 0x20)
        );
        assert_eq!(channel_colour(&with("nonsense")), accent());
    }
}
