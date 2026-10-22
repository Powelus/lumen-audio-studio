// SPDX-License-Identifier: GPL-3.0-or-later
//! Second bug hunt: flows an FL Studio user tries, each driven as a user does.

use super::Harness;

#[test]
fn the_toolbar_fits_a_small_screen_without_overlap() {
    for (w, hgt) in [(1280.0, 720.0), (1280.0, 800.0), (1366.0, 768.0), (2560.0, 1440.0)] {
        let h = Harness::demo_sized(w, hgt);
        let w = h.ctx.content_rect().right();
        let master = h.find_prefix("Master: ")[0].clone();
        for label in ["Save", "Export", "Browser", "Analyser"] {
            let n = h.get(label);
            assert!(
                n.rect.right() <= w + 0.5,
                "{w}x{hgt}: {label} at {:?} is off the screen",
                n.rect
            );
            assert!(
                !n.rect.intersects(master.rect),
                "{w}x{hgt}: {label} at {:?} is under the master knob at {:?}",
                n.rect,
                master.rect
            );
        }
        eprintln!(
            "{w} export {:?} master {:?}",
            h.get("Export").rect,
            master.rect
        );
        assert!(master.rect.right() <= w + 0.5, "{w}: master off screen");
    }
}
