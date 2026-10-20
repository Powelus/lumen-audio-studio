// SPDX-License-Identifier: GPL-3.0-or-later
// MIDI files the web version writes for the demo song, for the Rust exporter to match
// byte for byte. Copy into <pow-daw>/tests/zz_midi.test.ts and run:
//   DEMO=<demo.las> MIDI_OUT=<dir> npx vitest run tests/zz_midi.test.ts
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { exportMidi } from '../src/formats/midi';
import { decodeLas } from '../src/storage/lasFormat';

it('writes the midi files', async () => {
  const out = process.env.MIDI_OUT!;
  mkdirSync(out, { recursive: true });
  const { project } = await decodeLas(new Uint8Array(readFileSync(process.env.DEMO!)));
  writeFileSync(`${out}/demo_song.mid`, exportMidi(project, 'song'));
  const pat = project.patterns[2];
  writeFileSync(`${out}/demo_pattern.mid`, exportMidi(project, 'pattern', pat.id));
  writeFileSync(`${out}/demo_pattern.id`, pat.id);
});
