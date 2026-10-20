// SPDX-License-Identifier: GPL-3.0-or-later
// The demo song with only one channel's notes left, for finding which part of a full
// render differs from the web version. Copy into <pow-daw>/tests/zz_split.test.ts and run:
//   DEMO=<demo.las> SPLIT_OUT=<dir> npx vitest run tests/zz_split.test.ts
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { decodeLas, encodeLas } from '../src/storage/lasFormat';

it('writes one project per channel', async () => {
  const out = process.env.SPLIT_OUT!;
  mkdirSync(out, { recursive: true });
  const bundle = await decodeLas(new Uint8Array(readFileSync(process.env.DEMO!)));
  const only = process.env.ONLY ? process.env.ONLY.split(',') : null;
  bundle.project.channels.forEach(async () => {});
  for (const ch of bundle.project.channels) {
    if (only && !only.includes(ch.name)) continue;
    const p = structuredClone(bundle.project);
    for (const pat of p.patterns) for (const id of Object.keys(pat.notes)) if (id !== ch.id) pat.notes[id] = [];
    p.name = `demo_${ch.name}`;
    const file = `${out}/demo_${ch.name.replace(/\W+/g, '_')}.las`;
    writeFileSync(file, await encodeLas(p, bundle.samples, { encrypt: false }));
    console.log(file);
  }
});
