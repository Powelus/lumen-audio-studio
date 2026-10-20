// SPDX-License-Identifier: GPL-3.0-or-later
//! The MIDI files match the ones the web version writes (`tools/project/midi.gen.test.ts`).

use las_engine::export::{export_midi, MidiSource};
use las_project::decode;

fn read(path: &str) -> Vec<u8> {
    let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&full).unwrap_or_else(|e| panic!("{full}: {e}"))
}

#[test]
fn the_demo_song_exports_the_same_midi_file() {
    let bundle = decode(&read("../las-project/tests/fixtures/demo.las")).unwrap();
    assert!(export_midi(&bundle.project, MidiSource::Song) == read("tests/fixtures/demo_song.mid"));
    let id = String::from_utf8(read("tests/fixtures/demo_pattern.id")).unwrap();
    assert!(
        export_midi(&bundle.project, MidiSource::Pattern(Some(&id)))
            == read("tests/fixtures/demo_pattern.mid")
    );
}
