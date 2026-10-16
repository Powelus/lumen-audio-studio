// SPDX-License-Identifier: GPL-3.0-or-later
// Reference time stretches from the web version (Powelus/pow-daw): a few test
// signals (tones with drum hits, stereo and mono) stretched by several factors.
// Writes the inputs and outputs as one raw f32 file plus an index. Run it from a
// checkout of that repository:
//
//   cp tools/project/stretch.gen.test.ts <pow-daw>/tests/zz_stretch_export.test.ts
//   STRETCH_OUT=<this repo>/crates/las-dsp/tests/fixtures npx vitest run tests/zz_stretch_export.test.ts
import { writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { timeStretch } from '../src/audio/dsp/stretch';

const SR = 24000;

function signal(kind: string): Float32Array[] {
  const n = Math.floor(SR * 1.2);
  let seed = 4242;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 4294967296) * 2 - 1;
  const l = new Float32Array(n);
  const r = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    const t = i / SR;
    const tone = 0.4 * Math.sin(2 * Math.PI * 220 * t) + 0.2 * Math.sin(2 * Math.PI * 331 * t);
    l[i] = tone;
    r[i] = kind === 'mono' ? tone : 0.4 * Math.sin(2 * Math.PI * 220 * t + 0.5) + 0.2 * Math.sin(2 * Math.PI * 440 * t);
  }
  if (kind !== 'tones') {
    for (const hit of [0.1, 0.45, 0.8]) {
      const s = Math.floor(hit * SR);
      for (let i = 0; i < 2400 && s + i < n; i++) {
        const env = Math.exp(-i / 400);
        l[s + i] += rnd() * env * 0.6;
        r[s + i] += rnd() * env * 0.6;
      }
    }
  }
  return kind === 'mono' ? [l] : [l, r];
}

it('exports the stretches', () => {
  const out = process.env.STRETCH_OUT!;
  const index: { kind: string; factor: number; channels: number; inFrames: number; outFrames: number }[] = [];
  const parts: Float32Array[] = [];
  for (const [kind, factor] of [['tones', 1.25], ['hits', 0.8], ['hits', 2], ['mono', 1.5], ['hits', 0.55]] as const) {
    const input = signal(kind);
    const res = timeStretch(input, SR, factor);
    index.push({ kind, factor, channels: input.length, inFrames: input[0].length, outFrames: res[0].length });
    parts.push(...input, ...res);
  }
  const all = new Float32Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    all.set(p, o);
    o += p.length;
  }
  writeFileSync(`${out}/stretch.f32`, new Uint8Array(all.buffer));
  writeFileSync(`${out}/stretch.json`, JSON.stringify(index));
});
