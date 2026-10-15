// SPDX-License-Identifier: GPL-3.0-or-later
// Reference files for the Rust project crate, made by the web version
// (Powelus/pow-daw): the demo song as JSON and as .las files (encrypted and
// plain, with an embedded sample), and a damaged version 1 project with what
// the web version's migration and repair make of it. Run it from a checkout of
// that repository:
//
//   cp tools/project/project.gen.test.ts <pow-daw>/tests/zz_project_export.test.ts
//   PROJECT_OUT=<this repo>/crates/las-project/tests/fixtures \
//   npx vitest run tests/zz_project_export.test.ts
//
// It can also check a file written by Rust: set LAS_FROM_RUST to the path of a
// .las file and it decodes it and prints the project name.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { expect, it, vi } from 'vitest';
import { createDemoSong } from '../src/core/demoSong';
import { migrateProject } from '../src/core/migrate';
import { decodeLas, encodeLas } from '../src/storage/lasFormat';

const out = process.env.PROJECT_OUT!;

it('writes the reference files', async () => {
  mkdirSync(out, { recursive: true });
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));

  // the demo song, with one embedded sample
  const demo = createDemoSong();
  demo.samples.push({ id: 'smp_test', name: 'test.wav', duration: 0.5 });
  demo.meta = { ...demo.meta, createdAt: 1767225600000, updatedAt: 1767225600000, author: 'Reference' };
  const bytes = new Uint8Array(1000).map((_, i) => (i * 31 + 7) & 255);
  const samples = new Map([['smp_test', { data: bytes.buffer as ArrayBuffer, name: 'test.wav', mime: 'audio/wav' }]]);
  writeFileSync(`${out}/demo.project.json`, JSON.stringify(demo));
  writeFileSync(`${out}/demo.las`, await encodeLas(demo, samples, { encrypt: true }));
  writeFileSync(`${out}/demo-plain.las`, await encodeLas(demo, samples, { encrypt: false }));
  writeFileSync(`${out}/sample.bin`, bytes);

  // a version 1 project with damage in it
  const legacy = {
    version: 1,
    id: 'prj_legacy',
    name: 'Old project',
    bpm: 5000,
    timeSig: [4, 4],
    swing: 3,
    masterVolume: 2,
    channels: [
      { id: 'ch_a', name: 'Pad', pluginId: 'triosc', kind: 'instrument', state: {}, volume: 9, pan: 0, pitch: 0, mixerTrack: 1, mute: false, rootKey: 60, color: '#fff' },
      { id: 'ch_a', name: 'Pad copy', pluginId: 'gone-synth', kind: 'instrument', state: {}, volume: 1, pan: 0, pitch: 0, mixerTrack: 99, mute: false, rootKey: 60, color: '#fff' },
      { id: 'ch_p', name: 'Pad', pluginId: 'powpad', kind: 'instrument', state: { pads: [] }, volume: 1, pan: 0, pitch: 0, mixerTrack: 0, mute: false, rootKey: 60, color: '#fff' },
    ],
    patterns: [
      {
        id: 'pat_1',
        name: 'A',
        color: '#fff',
        length: 0,
        notes: {
          ch_a: [
            { id: 'n1', key: 60.4, pos: 0, len: 24, vel: 3, pan: 0 },
            { id: 'n1', key: 64, pos: 24, len: 0, vel: 1, pan: 0 },
            { id: 'n3', key: 400, pos: 48, len: 24, vel: 0.5, pan: 5 },
          ],
          ch_x: 'oops',
        },
      },
    ],
    playlist: {
      tracks: [{ id: 't1', name: 'T', color: '#222', mute: false, height: 56 }],
      clips: [
        { id: 'c1', kind: 'pattern', refId: 'pat_1', track: 0, start: 0, length: 384, offset: 0 },
        { id: 'c2', kind: 'pattern', refId: 'missing', track: 7, start: 0, length: 384, offset: -4 },
        { id: 'c3', kind: 'audio', refId: 'ch_a', track: 0, start: -1, length: 10, offset: 0 },
      ],
    },
    automation: [{ id: 'a1', name: 'Dangling', color: '#fff', target: { kind: 'mixer', track: 77, param: 'volume' }, points: [{ t: 10, v: 2 }, { t: 0, v: -1 }, { t: 'x', v: 1 }], length: 0 }],
    mixer: [
      { id: 'm0', name: 'Master', color: '#000', volume: 1, pan: 0, width: 1, mute: false, solo: false, slots: [{ id: 'f1', pluginId: 'pow-limiter', enabled: true, mix: 4, state: {} }], sends: [], output: 5, fxBypass: false },
      { id: 'm1', name: 'A', color: '#000', volume: 1, pan: 0, width: 1, mute: false, solo: false, slots: [{ id: 'f1', pluginId: 'pow-eq', enabled: true, mix: 1, state: {} }, { pluginId: 'nothing' }, null], sends: [{ target: 2, amount: 1, sidechainOnly: false }, { target: 1, amount: 1, sidechainOnly: false }, { target: 40, amount: 1, sidechainOnly: false }], output: 0, fxBypass: false },
      { id: 'm2', name: 'B', color: '#000', volume: 7, pan: 0, width: 1, mute: false, solo: false, slots: [], sends: [{ target: 1, amount: 1, sidechainOnly: false }], output: 1, fxBypass: false },
    ],
    samples: [{ id: 's1', name: 'one' }, { id: 's1', name: 'two' }, { id: 7 }],
    meta: { createdAt: 1, updatedAt: 2 },
    markers: [{ id: 'k2', tick: 96, name: 'B' }, { id: 'k1', tick: 0, name: 'A' }, { id: 'k0', tick: -5, name: 'bad' }],
    midiMap: [{ id: 'mm', channel: 0, cc: 1, target: { kind: 'master', param: 'volume' } }, { id: 'mx', channel: 99, cc: 1, target: { kind: 'master', param: 'volume' } }],
    futureField: { keep: 'me' },
  };
  const res = migrateProject(legacy);
  writeFileSync(`${out}/legacy.input.json`, JSON.stringify(legacy));
  writeFileSync(`${out}/legacy.migrated.json`, JSON.stringify(res.project));
  writeFileSync(`${out}/legacy.warnings.json`, JSON.stringify({ warnings: res.warnings, migratedFrom: res.migratedFrom }));
  console.log(res.warnings);
  vi.useRealTimers();
});

it.runIf(!!process.env.LAS_FROM_RUST)('reads a file written by Rust', async () => {
  const b = await decodeLas(new Uint8Array(readFileSync(process.env.LAS_FROM_RUST!)));
  expect(b.project.name).toBe(process.env.LAS_EXPECT_NAME ?? 'Lumen Nights (demo)');
  expect(b.samples.size).toBe(1);
  console.log('rust file ok', b.project.name, b.project.channels.length, b.warnings);
});
