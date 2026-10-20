// SPDX-License-Identifier: GPL-3.0-or-later
//! The wavetables the oscillator screens draw, built once for the screen.

use las_dsp::wavetable::TableLibrary;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static LIB: RefCell<TableLibrary> = RefCell::new(TableLibrary::new());
    static FRAMES: RefCell<HashMap<String, Vec<Vec<f32>>>> = RefCell::new(HashMap::new());
}

/// The single-cycle frames of a built-in table (full bandwidth).
pub fn frames(id: &str) -> Vec<Vec<f32>> {
    if let Some(f) = FRAMES.with(|m| m.borrow().get(id).cloned()) {
        return f;
    }
    let t = LIB.with(|l| l.borrow_mut().get(id));
    let f: Vec<Vec<f32>> = t.mips.first().cloned().unwrap_or_default();
    FRAMES.with(|m| m.borrow_mut().insert(id.to_string(), f.clone()));
    f
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_saw_table_has_a_falling_or_rising_ramp() {
        let f = super::frames("saw");
        assert!(!f.is_empty());
        let cycle = &f[0];
        assert!(cycle.len() > 64);
        let peak = cycle.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.1);
    }
}
