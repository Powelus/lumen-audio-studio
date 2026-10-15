// SPDX-License-Identifier: GPL-3.0-or-later
// Generates the reference audio for the effects parity test from the effect
// worklets of the web version (Powelus/pow-daw). The worklets need a few browser
// globals, which this file provides. Run it from a checkout of that repository
// (see docs/PARITY.md):
//
//   cp tools/parity/fx.gen.test.ts <pow-daw>/tests/zz_fx_gen.test.ts
//   PARITY_CASES=<this repo>/tools/parity/fx.cases.json \
//   PARITY_OUT=<this repo>/crates/las-dsp/tests/fixtures \
//   npx vitest run tests/zz_fx_gen.test.ts
import { readFileSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';

const SR = 48000;
const BLOCK = 128;
const FRAMES = 188 * BLOCK;

/** A deterministic stereo test signal: tones, a sweep and decaying noise bursts. */
function makeInput() {
  let seed = 987654321;
  const rnd = () => {
    seed ^= seed << 13;
    seed ^= seed >>> 17;
    seed ^= seed << 5;
    return ((seed >>> 0) / 4294967296) * 2 - 1;
  };
  const L = new Float32Array(FRAMES);
  const R = new Float32Array(FRAMES);
  const SL = new Float32Array(FRAMES);
  const SR_ = new Float32Array(FRAMES);
  for (let i = 0; i < FRAMES; i++) {
    const t = i / SR;
    const burst = Math.exp(-((t % 0.12) * 28));
    const sweep = Math.sin(2 * Math.PI * (200 * t + 3000 * t * t));
    const noise = rnd() * burst * 0.5;
    L[i] = 0.45 * Math.sin(2 * Math.PI * 220 * t) * (0.4 + 0.6 * burst) + 0.2 * sweep + noise;
    R[i] = 0.4 * Math.sin(2 * Math.PI * 330 * t + 0.5) * (0.4 + 0.6 * burst) + 0.2 * sweep * 0.8 - noise * 0.7;
    // sidechain: kick-like bursts every quarter second
    const k = Math.exp(-((t % 0.25) * 30));
    SL[i] = 0.9 * Math.sin(2 * Math.PI * 60 * t) * k;
    SR_[i] = SL[i] * 0.9;
  }
  return { L, R, SL, SR: SR_ };
}

it('writes the effect reference renders', async () => {
  const g = globalThis as any;
  const procs: Record<string, any> = {};
  g.sampleRate = SR;
  g.currentTime = 0;
  g.currentFrame = 0;
  g.AudioWorkletProcessor = class {
    port = { onmessage: null, postMessage() {} };
  };
  g.registerProcessor = (name: string, ctor: unknown) => {
    procs[name] = ctor;
  };
  await import('../src/audio/worklets/fx.worklet');

  const x = makeInput();
  // the shared input: left, right, sidechain left, sidechain right
  const inBuf = Buffer.alloc(FRAMES * 16);
  [x.L, x.R, x.SL, x.SR].forEach((ch, c) => ch.forEach((v, i) => inBuf.writeFloatLE(v, (c * FRAMES + i) * 4)));
  writeFileSync(`${process.env.PARITY_OUT}/fx_input.f32`, inBuf);

  const cases = JSON.parse(readFileSync(process.env.PARITY_CASES!, 'utf8'));
  for (const c of cases) {
    const p = new procs[c.processor]({ processorOptions: { params: c.params, transport: c.transport } });
    const L = new Float32Array(FRAMES);
    const R = new Float32Array(FRAMES);
    for (let i = 0; i < FRAMES; i += BLOCK) {
      const inputs = [[x.L.slice(i, i + BLOCK), x.R.slice(i, i + BLOCK)]];
      if (c.sidechain) inputs.push([x.SL.slice(i, i + BLOCK), x.SR.slice(i, i + BLOCK)]);
      const out = [new Float32Array(BLOCK), new Float32Array(BLOCK)];
      g.currentTime = i / SR;
      g.currentFrame = i;
      p.process(inputs, [out]);
      L.set(out[0], i);
      R.set(out[1], i);
    }
    const buf = Buffer.alloc(FRAMES * 8);
    for (let i = 0; i < FRAMES; i++) {
      buf.writeFloatLE(L[i], i * 4);
      buf.writeFloatLE(R[i], (FRAMES + i) * 4);
    }
    writeFileSync(`${process.env.PARITY_OUT}/fx_${c.name}.f32`, buf);
  }
});
