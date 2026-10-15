// SPDX-License-Identifier: GPL-3.0-or-later
//! The preset lists the window shows: the factory banks plus the user's own
//! presets, which are kept as JSON files in the configuration folder.

use las_dsp::patch::merge_patch;
use las_dsp::presets::bank;
use serde_json::{json, Value};
use std::path::PathBuf;

/// One entry of a preset list.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub category: String,
    /// The complete patch.
    pub patch: Value,
    /// Saved by the user (can be deleted).
    pub user: bool,
    /// The effects rack the preset brings: `{type, on, state}` items.
    pub fx: Vec<Value>,
}

/// The folder for the app's own files.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("lumen-audio-studio"))
}

fn user_file(bank_id: &str) -> Option<PathBuf> {
    Some(
        config_dir()?
            .join("presets")
            .join(format!("{bank_id}.json")),
    )
}

/// The list for an instrument: Init (the defaults), the factory presets, then
/// the user's presets. `defaults` is the instrument's complete default patch.
pub fn entries(bank_id: &str, defaults: &Value) -> Vec<Entry> {
    let mut out = vec![Entry {
        name: "Init".into(),
        category: "Init".into(),
        patch: defaults.clone(),
        user: false,
        fx: Vec::new(),
    }];
    if let Some(b) = bank(bank_id) {
        for (i, p) in b.presets.iter().enumerate() {
            if p.name.eq_ignore_ascii_case("init") {
                continue;
            }
            if let Some(patch) = b.patch(i) {
                out.push(Entry {
                    name: p.name.clone(),
                    category: p.category.clone(),
                    patch: merge_patch(defaults, &patch),
                    user: false,
                    fx: p
                        .state
                        .get("fx")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                });
            }
        }
    }
    out.extend(load_user(bank_id, defaults));
    out
}

fn load_user(bank_id: &str, defaults: &Value) -> Vec<Entry> {
    let Some(text) = user_file(bank_id).and_then(|f| std::fs::read_to_string(f).ok()) else {
        return Vec::new();
    };
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|p| {
            Some(Entry {
                name: p["name"].as_str()?.to_string(),
                category: "User".into(),
                patch: merge_patch(defaults, p.get("patch")?),
                user: true,
                fx: p
                    .get("fx")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Write the user's presets of one instrument.
pub fn save_user(bank_id: &str, entries: &[Entry]) -> Result<(), String> {
    let file = user_file(bank_id).ok_or("no configuration folder")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let list: Vec<Value> = entries
        .iter()
        .filter(|e| e.user)
        .map(|e| json!({ "name": e.name, "patch": e.patch, "fx": e.fx }))
        .collect();
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    // write to a temporary file first, so a crash cannot leave half a file
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display()))
}

/// Whether a preset matches what the user typed in the search box.
pub fn matches(e: &Entry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || e.name.to_lowercase().contains(&q) || e.category.to_lowercase().contains(&q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_starts_with_init_and_holds_the_factory_presets() {
        let defaults = json!({ "level": 0.7 });
        let list = entries("polaris", &defaults);
        assert_eq!(list[0].name, "Init");
        assert!(list.len() > 30);
        assert!(list.iter().all(|e| e.patch.is_object()));
        // Init appears once
        assert_eq!(list.iter().filter(|e| e.name == "Init").count(), 1);
        assert_eq!(entries("no-bank", &defaults).len(), 1);
    }

    #[test]
    fn search_matches_names_and_categories() {
        let e = Entry {
            name: "Supersaw Lead".into(),
            category: "Lead".into(),
            patch: json!({}),
            user: false,
            fx: Vec::new(),
        };
        assert!(matches(&e, ""));
        assert!(matches(&e, "super"));
        assert!(matches(&e, "LEAD"));
        assert!(!matches(&e, "pad"));
    }

    #[test]
    fn user_presets_round_trip_through_a_file() {
        let dir = std::env::temp_dir().join(format!("lumen-test-{}", std::process::id()));
        // point the configuration folder at a scratch folder for this test
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        if cfg!(windows) || cfg!(target_os = "macos") {
            // the folder differs on these systems; the round trip is covered on Linux
            return;
        }
        let defaults = json!({ "level": 0.7, "tune": 0 });
        let mut list = entries("polaris", &defaults);
        let n = list.len();
        list.push(Entry {
            name: "Mine".into(),
            category: "User".into(),
            patch: json!({ "level": 0.2 }),
            user: true,
            fx: Vec::new(),
        });
        save_user("polaris", &list).unwrap();
        let again = entries("polaris", &defaults);
        assert_eq!(again.len(), n + 1);
        let mine = again.last().unwrap();
        assert!(mine.user && mine.name == "Mine");
        assert_eq!(mine.patch["level"], 0.2);
        assert_eq!(mine.patch["tune"], 0, "filled in from the defaults");
        std::fs::remove_dir_all(dir).ok();
    }
}
