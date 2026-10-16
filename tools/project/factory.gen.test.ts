// SPDX-License-Identifier: GPL-3.0-or-later
// Exports the factory sample kit of the web version (Powelus/pow-daw) at 24 kHz as
// one raw f32 file plus an index, for the Rust port to compare with. Run it from a
// checkout of that repository:
//
//   cp tools/project/factory.gen.test.ts <pow-daw>/tests/zz_factory_export.test.ts
//   FACTORY_OUT=<this repo>/crates/las-engine/tests/fixtures npx vitest run tests/zz_factory_export.test.ts
import { writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { FACTORY_SAMPLES } from '../src/audio/factory';

it('exports the factory samples', () => {
  const out = process.env.FACTORY_OUT!;
  const index: { id: string; channels: number; frames: number }[] = [];
  const parts: Float32Array[] = [];
  for (const s of FACTORY_SAMPLES) {
    const chans = s.render(24000);
    index.push({ id: s.id, channels: chans.length, frames: chans[0].length });
    parts.push(...chans);
  }
  const total = parts.reduce((n, p) => n + p.length, 0);
  const all = new Float32Array(total);
  let o = 0;
  for (const p of parts) {
    all.set(p, o);
    o += p.length;
  }
  writeFileSync(`${out}/factory.f32`, new Uint8Array(all.buffer));
  writeFileSync(`${out}/factory.json`, JSON.stringify(index));
});
