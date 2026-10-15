// SPDX-License-Identifier: GPL-3.0-or-later
// Generates the reference audio for the Facet parity test from the TypeScript
// engine of the web version (Powelus/pow-daw). Run it from a checkout of that
// repository (see docs/PARITY.md):
//
//   cp tools/parity/facet.gen.test.ts <pow-daw>/tests/zz_facet_gen.test.ts
//   PARITY_CASES=<this repo>/tools/parity/facet.cases.json \
//   PARITY_OUT=<this repo>/crates/las-dsp/tests/fixtures \
//   npx vitest run tests/zz_facet_gen.test.ts
import { readFileSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { FmEngine } from '../src/audio/synth/fm';

const SR = 48000;
const BLOCK = 128;
const FRAMES = 24000; // half a second

it('writes the Facet reference renders', () => {
  const cases = JSON.parse(readFileSync(process.env.PARITY_CASES!, 'utf8'));
  for (const c of cases) {
    const e = new FmEngine(SR, c.seed);
    e.setPatch(c.patch);
    e.queue(c.events);
    const L = new Float32Array(FRAMES);
    const R = new Float32Array(FRAMES);
    const bl = new Float32Array(BLOCK);
    const br = new Float32Array(BLOCK);
    for (let i = 0; i < FRAMES; i += BLOCK) {
      const n = Math.min(BLOCK, FRAMES - i);
      const l = bl.subarray(0, n);
      const r = br.subarray(0, n);
      l.fill(0);
      r.fill(0);
      e.process(l, r, i / SR);
      L.set(l, i);
      R.set(r, i);
    }
    // little-endian f32: left channel, then right
    const out = Buffer.alloc(FRAMES * 8);
    for (let i = 0; i < FRAMES; i++) {
      out.writeFloatLE(L[i], i * 4);
      out.writeFloatLE(R[i], (FRAMES + i) * 4);
    }
    writeFileSync(`${process.env.PARITY_OUT}/facet_${c.name}.f32`, out);
  }
});
