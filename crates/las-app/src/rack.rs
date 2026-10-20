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
