// SPDX-License-Identifier: GPL-3.0-or-later
// Generates the reference audio for the wavetable synth parity test from the
// synth AudioWorklet of the web version (Powelus/pow-daw). The worklet needs a
// few browser globals, which this file provides. Run it from a checkout of that
// repository (see docs/PARITY.md):
//
//   cp tools/parity/synth.gen.test.ts <pow-daw>/tests/zz_synth_gen.test.ts
//   PARITY_CASES=<this repo>/tools/parity/synth.cases.json \
//   PARITY_OUT=<this repo>/crates/las-dsp/tests/fixtures \
//   npx vitest run tests/zz_synth_gen.test.ts
//
// Math.random is fixed (the original is not seeded), so the cases leave out what
// depends on it: random phase, noise, sample and hold, and the "rand" source.
import { readFileSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';

const SR = 48000;
const BLOCK = 128;
const FRAMES = 12000; // a quarter of a second

it('writes the synth reference renders', async () => {
  const g = globalThis as any;
  let Processor: any;
  g.sampleRate = SR;
  g.currentTime = 0;
  g.AudioWorkletProcessor = class {
    port = { onmessage: null };
  };
  g.registerProcessor = (_name: string, ctor: unknown) => {
    Processor = ctor;
  };
  Math.random = () => 0.5;
  await import('../src/audio/worklets/synth.worklet');

  const cases = JSON.parse(readFileSync(process.env.PARITY_CASES!, 'utf8'));
  for (const c of cases) {
    const p = new Processor({ processorOptions: { patch: c.patch, events: c.events } });
    const L = new Float32Array(FRAMES);
    const R = new Float32Array(FRAMES);
    for (let i = 0; i < FRAMES; i += BLOCK) {
      const n = Math.min(BLOCK, FRAMES - i);
      const l = new Float32Array(BLOCK);
      const r = new Float32Array(BLOCK);
      g.currentTime = i / SR;
      p.process([], [[l, r]]);
      L.set(l.subarray(0, n), i);
      R.set(r.subarray(0, n), i);
    }
    // little-endian f32: left channel, then right
    const out = Buffer.alloc(FRAMES * 8);
    for (let i = 0; i < FRAMES; i++) {
      out.writeFloatLE(L[i], i * 4);
      out.writeFloatLE(R[i], (FRAMES + i) * 4);
    }
    writeFileSync(`${process.env.PARITY_OUT}/synth_${c.name}.f32`, out);
  }
});
