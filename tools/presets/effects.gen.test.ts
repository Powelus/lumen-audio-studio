// SPDX-License-Identifier: GPL-3.0-or-later
// Exports the effect descriptors of the web version (Powelus/pow-daw): every
// effect's name, category, parameters (ranges, curves, units, options), default
// state and factory presets, as one JSON file. The app draws an effect's
// controls from this. Run it from a checkout of that repository:
//
//   cp tools/presets/effects.gen.test.ts <pow-daw>/tests/zz_effects_export.test.ts
//   EFFECTS_OUT=<this repo>/crates/las-dsp/effects/effects.json \
//   npx vitest run tests/zz_effects_export.test.ts
import { writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { EFFECTS, SHAPER_PRESETS } from '../src/plugins/effects';
import { EFFECT_PRESETS } from '../src/plugins/effectPresets';

it('exports the effect descriptors', () => {
  const effects = EFFECTS.map((e) => ({
    id: e.id,
    name: e.name,
    category: e.category,
    description: e.description,
    sidechain: !!e.sidechain,
    params: e.params,
    defaultState: e.defaultState(),
    presets: (EFFECT_PRESETS[e.id] ?? []).map((p) => ({ name: p.name, category: p.category, state: p.state })),
  }));
  writeFileSync(process.env.EFFECTS_OUT!, JSON.stringify({ effects, shaperPresets: SHAPER_PRESETS }) + '\n');
  console.log(effects.length, effects.reduce((n, e) => n + e.presets.length, 0));
});
