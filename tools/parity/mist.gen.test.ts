// SPDX-License-Identifier: GPL-3.0-or-later
// Generates the reference audio for the Mist parity test from the TypeScript
// engine of the web version (Powelus/pow-daw). Run it from a checkout of that
// repository (see docs/PARITY.md):
//
//   cp tools/parity/mist.gen.test.ts <pow-daw>/tests/zz_mist_gen.test.ts
//   PARITY_CASES=<this repo>/tools/parity/mist.cases.json \
//   PARITY_OUT=<this repo>/crates/las-dsp/tests/fixtures \
//   npx vitest run tests/zz_mist_gen.test.ts
import { readFileSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { defaultTexture, GranularEngine } from '../src/audio/synth/granular';

const SR = 48000;
const BLOCK = 128;
const FRAMES = 24000; // half a second

it('writes the Mist reference renders', () => {
  const cases = JSON.parse(readFileSync(process.env.PARITY_CASES!, 'utf8'));
  const src = defaultTexture(SR);
  // the built-in texture itself: the first and the last 4800 samples of both channels
  const tex = Buffer.alloc(4 * 4800 * 4);
  let o = 0;
  for (const ch of src) {
    for (const range of [[0, 4800], [ch.length - 4800, ch.length]]) {
      for (let i = range[0]; i < range[1]; i++) {
        tex.writeFloatLE(ch[i], o);
        o += 4;
      }
    }
  }
  writeFileSync(`${process.env.PARITY_OUT}/mist_texture.f32`, tex);
  for (const c of cases) {
    const e = new GranularEngine(SR, c.seed);
    e.setSource(src, SR);
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
    writeFileSync(`${process.env.PARITY_OUT}/mist_${c.name}.f32`, out);
    console.log(c.name, 'grains started', e.grainsStarted);
  }
});
