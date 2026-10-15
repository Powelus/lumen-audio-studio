// SPDX-License-Identifier: GPL-3.0-or-later
//! Files made by the web version (see `tools/project/project.gen.test.ts`) must
//! read the same here, and a project must survive being written and read again.

use las_project::{decode, encode, migrate_project, Sample};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn json_fixture(name: &str) -> Value {
    serde_json::from_slice(&fixture(name)).expect("valid JSON")
}

/// Numbers as `f64` (so 1 and 1.0 agree) and ids that were not in the input masked.
fn normalise(v: &Value, known: &HashSet<String>) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().unwrap_or(0.0)),
        Value::Array(a) => Value::Array(a.iter().map(|x| normalise(x, known)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| {
                    let x = match (k.as_str(), x) {
                        ("id", Value::String(s)) if !known.contains(s) => json!("<new>"),
                        _ => normalise(x, known),
                    };
                    (k.clone(), x)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn ids_in(v: &Value, out: &mut HashSet<String>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| ids_in(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                match (k.as_str(), x) {
                    ("id", Value::String(s)) => {
                        out.insert(s.clone());
                    }
                    _ => ids_in(x, out),
                }
            }
        }
        _ => {}
    }
}

fn first_difference(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                match (x.get(k), y.get(k)) {
                    (Some(p), Some(q)) => {
                        if let Some(d) = first_difference(p, q, &format!("{path}/{k}")) {
                            return Some(d);
                        }
                    }
                    (p, q) => return Some(format!("{path}/{k}: {p:?} against {q:?}")),
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: {} items against {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| first_difference(p, q, &format!("{path}/{i}")))
        }
        _ if a == b => None,
        _ => Some(format!("{path}: {a} against {b}")),
    }
}

fn same(a: &Value, b: &Value) {
    let mut known = HashSet::new();
    ids_in(a, &mut known);
    ids_in(b, &mut known);
    // ids present on only one side are made up on that side
    let (na, nb) = (normalise(a, &known), normalise(b, &known));
    if let Some(d) = first_difference(&na, &nb, "") {
        panic!("projects differ at {d}");
    }
}

#[test]
fn the_demo_song_from_the_web_version_reads_unchanged() {
    let expected = json_fixture("demo.project.json");
    for file in ["demo.las", "demo-plain.las"] {
        let bundle = decode(&fixture(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(bundle.warnings.is_empty(), "{file}: {:?}", bundle.warnings);
        let got = serde_json::to_value(&bundle.project).unwrap();
        // the demo's ids all come from the file, so compare them too
        let mut known = HashSet::new();
        ids_in(&expected, &mut known);
        let (a, b) = (normalise(&got, &known), normalise(&expected, &known));
        if let Some(d) = first_difference(&a, &b, "") {
            panic!("{file} differs at {d}");
        }
        assert_eq!(bundle.project.name, "Lumen Nights (demo)");
        assert_eq!(bundle.project.channels.len(), 13);
        let sample = &bundle.samples["smp_test"];
        assert_eq!(sample.name, "test.wav");
        assert_eq!(sample.mime, "audio/wav");
        assert_eq!(sample.data, fixture("sample.bin"));
    }
}

#[test]
fn a_damaged_version_one_project_is_migrated_and_repaired_like_the_original() {
    let input = json_fixture("legacy.input.json");
    let migrated = migrate_project(&input).expect("migrates");
    let expected_warnings = json_fixture("legacy.warnings.json");
    let want: Vec<&str> = expected_warnings["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(migrated.warnings, want);
    assert_eq!(migrated.migrated_from, Some(1));
    let got = serde_json::to_value(&migrated.project).unwrap();
    let expected = json_fixture("legacy.migrated.json");
    let mut known = HashSet::new();
    ids_in(&input, &mut known);
    let (a, b) = (normalise(&got, &known), normalise(&expected, &known));
    if let Some(d) = first_difference(&a, &b, "") {
        panic!("legacy project differs at {d}");
    }
    // unknown fields are kept
    assert_eq!(got["futureField"], json!({ "keep": "me" }));
}

#[test]
fn a_project_survives_being_written_and_read_again() {
    let original = decode(&fixture("demo.las")).unwrap();
    let mut samples = BTreeMap::new();
    samples.insert(
        "smp_test".to_string(),
        Sample {
            name: "test.wav".into(),
            mime: "audio/wav".into(),
            data: fixture("sample.bin"),
        },
    );
    for encrypt in [true, false] {
        let bytes = encode(&original.project, &samples, encrypt).unwrap();
        let again = decode(&bytes).unwrap();
        assert!(again.warnings.is_empty());
        same(
            &serde_json::to_value(&original.project).unwrap(),
            &serde_json::to_value(&again.project).unwrap(),
        );
        assert_eq!(again.samples, samples);
    }
}

#[test]
fn damaged_files_are_refused_with_a_reason() {
    let good = fixture("demo.las");
    let say = |bytes: &[u8]| {
        decode(bytes)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    };
    assert!(
        say(b"not a project at all, just text, nothing more to see here").contains("Not a Lumen")
    );
    assert!(say(&good[..good.len() - 40]).contains("truncated"));
    // a flipped bit in the payload
    let mut flipped = good.clone();
    let last = flipped.len() - 20;
    flipped[last] ^= 1;
    assert!(say(&flipped).contains("damaged or was modified"));
    // a flipped bit in the header (covered by the authentication)
    let mut header = good.clone();
    header[28] = 1;
    assert!(say(&header).contains("damaged or was modified"));
    // a newer format
    let mut newer = good.clone();
    newer[4] = 9;
    assert!(say(&newer).contains("newer version"));
    // plain files have a CRC
    let mut plain = fixture("demo-plain.las");
    let last = plain.len() - 20;
    plain[last] ^= 1;
    assert!(say(&plain).contains("integrity"));
}

#[test]
fn a_newer_project_version_is_refused() {
    let err = migrate_project(&json!({ "version": 99 })).err().unwrap();
    assert!(err.to_string().contains("newer version"));
    assert!(migrate_project(&json!([1, 2])).is_err());
}

#[test]
fn an_empty_object_becomes_a_usable_project() {
    let m = migrate_project(&json!({})).unwrap();
    assert_eq!(m.project.mixer.len(), 25);
    assert_eq!(m.project.mixer[3].slots.len(), 10);
    assert_eq!(m.project.mixer[0].output, None);
    assert_eq!(m.project.mixer[1].output, Some(0));
    assert_eq!(m.project.playlist.tracks.len(), 24);
    assert_eq!(m.project.patterns.len(), 1);
    assert_eq!(m.project.bpm, 128.0);
    assert!(m.warnings.is_empty());
}
