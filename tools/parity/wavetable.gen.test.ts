// SPDX-License-Identifier: GPL-3.0-or-later
// Generates the reference numbers for the wavetable parity test from the
// TypeScript code of the web version (Powelus/pow-daw). Run it from a checkout
// of that repository (see docs/PARITY.md):
//
//   cp tools/parity/wavetable.gen.test.ts <pow-daw>/tests/zz_wavetable_gen.test.ts
//   PARITY_OUT=<this repo>/crates/las-dsp/tests/fixtures \
//   npx vitest run tests/zz_wavetable_gen.test.ts
import { writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { getTable, TABLE_IDS } from '../src/audio/dsp/wavetables';

// for every built-in table: the first, a middle and the last frame, at mip
// levels 0 and 4, the first 128 samples of each (little-endian f32)
it('writes the wavetable reference numbers', () => {
  const out: number[] = [];
  const ids = [...TABLE_IDS, 'no-such-table'];
  for (const id of ids) {
    const t = getTable(id);
    const frames = [0, Math.floor((t.frames - 1) / 2), t.frames - 1];
    for (const level of [0, 4]) {
      for (const f of frames) {
        const data = t.mips[level][f];
        for (let i = 0; i < 128; i++) out.push(data[i]);
      }
    }
  }
  const buf = Buffer.alloc(out.length * 4);
  out.forEach((v, i) => buf.writeFloatLE(v, i * 4));
  writeFileSync(`${process.env.PARITY_OUT}/wavetables.f32`, buf);
  console.log(ids.join(','), out.length);
});
