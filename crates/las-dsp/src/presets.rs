// SPDX-License-Identifier: GPL-3.0-or-later
//! The factory presets of the web version, one bank per instrument.
//!
//! The banks are JSON files exported from the web version by
//! `tools/presets/export.gen.test.ts`. A bank holds the instrument's complete
//! init patch and its presets; a preset lists only what differs from the init
//! patch, so its patch is `merge_patch(init, preset.patch)`.

use crate::patch::merge_patch;
use serde_json::Value;
use std::sync::OnceLock;

/// One preset.
pub struct Preset {
    pub name: String,
    pub category: String,
    /// Only what differs from the bank's init patch.
    pub patch: Value,
    /// The rest of the preset's state: effects rack, arpeggiator, and for Nebula
    /// the layers and macros. Not used by the engines yet.
    pub state: Value,
}

/// The presets of one instrument.
pub struct Bank {
    pub id: String,
    pub name: String,
    /// The instrument's complete init patch.
    pub init: Value,
    pub presets: Vec<Preset>,
}

impl Bank {
    /// The complete patch of preset `index`, ready for `set_patch`.
    pub fn patch(&self, index: usize) -> Option<Value> {
        let p = self.presets.get(index)?;
        Some(merge_patch(&self.init, &p.patch))
    }

    /// The index of the preset with this name (case does not matter).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.presets
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The categories in the order they first appear.
    pub fn categories(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in &self.presets {
            if !out.contains(&p.category.as_str()) {
                out.push(&p.category);
            }
        }
        out
    }
}

/// Instruments that have a bank.
pub const BANK_IDS: [&str; 8] = [
    "polaris", "vortex", "monolith", "zenith", "triosc", "facet", "mist", "nebula",
];

fn parse(text: &str) -> Bank {
    let v: Value = serde_json::from_str(text).expect("a valid preset bank");
    let str_of = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let presets = v["presets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| Preset {
                    name: str_of(p, "name"),
                    category: str_of(p, "category"),
                    patch: p["state"]
                        .get("patch")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Default::default())),
                    state: p["state"].clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Bank {
        id: str_of(&v, "id"),
        name: str_of(&v, "name"),
        init: v["init"].clone(),
        presets,
    }
}

/// The factory bank of an instrument (see [`BANK_IDS`]).
pub fn bank(id: &str) -> Option<&'static Bank> {
    macro_rules! bank {
        ($file:literal) => {{
            static CELL: OnceLock<Bank> = OnceLock::new();
            CELL.get_or_init(|| parse(include_str!(concat!("../presets/", $file))))
        }};
    }
    Some(match id {
        "polaris" => bank!("polaris.json"),
        "vortex" => bank!("vortex.json"),
        "monolith" => bank!("monolith.json"),
        "zenith" => bank!("zenith.json"),
        "triosc" => bank!("triosc.json"),
        "facet" => bank!("facet.json"),
        "mist" => bank!("mist.json"),
        "nebula" => bank!("nebula.json"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bank_loads() {
        let mut total = 0;
        for id in BANK_IDS {
            let b = bank(id).unwrap_or_else(|| panic!("no bank {id}"));
            assert_eq!(b.id, id);
            assert!(!b.presets.is_empty(), "{id} is empty");
            assert!(b.init.is_object());
            total += b.presets.len();
        }
        // the web version ships about 440 factory presets (Nebula 230 of them)
        assert!(total >= 440, "{total}");
        assert!(bank("nope").is_none());
    }

    #[test]
    fn names_are_unique_within_a_bank() {
        for id in BANK_IDS {
            let b = bank(id).unwrap();
            let mut names: Vec<&str> = b.presets.iter().map(|p| p.name.as_str()).collect();
            names.sort_unstable();
            let before = names.len();
            names.dedup();
            assert_eq!(names.len(), before, "{id} has a repeated name");
        }
    }

    #[test]
    fn a_preset_merges_over_the_init_patch() {
        let b = bank("polaris").unwrap();
        let i = b.find("supersaw lead").expect("Supersaw Lead");
        let p = b.patch(i).unwrap();
        assert_eq!(p["osc"][0]["voices"], 8);
        // untouched values come from the init patch
        assert_eq!(p["filterRouting"], "parallel");
        assert!(b.categories().contains(&"Lead"));
    }
}
