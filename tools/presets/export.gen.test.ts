// SPDX-License-Identifier: GPL-3.0-or-later
// Exports the factory presets of the web version (Powelus/pow-daw) as JSON, one
// bank file per instrument. Run it from a checkout of that repository:
//
//   cp tools/presets/export.gen.test.ts <pow-daw>/tests/zz_presets_export.test.ts
//   PRESETS_OUT=<this repo>/crates/las-dsp/presets \
//   npx vitest run tests/zz_presets_export.test.ts
//
// Each file holds the instrument's init patch (complete) and its presets. A
// preset's `patch` lists only what differs from the init patch; merge it over
// the init patch with merge_patch. Everything else in a preset's `state`
// (effects rack, arpeggiator, Nebula layers and macros) is kept for later.
import { writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { INSTRUMENTS, SYNTH_INITS, ENGINE_INITS } from '../src/plugins/instruments';
import { defaultPatch, defaultFilter } from '../src/audio/synth/patch';

const banks = ['polaris', 'vortex', 'monolith', 'zenith', 'triosc', 'facet', 'mist', 'nebula'];

it('exports the factory presets', () => {
  for (const id of banks) {
    const inst = INSTRUMENTS.find((i) => i.id === id)!;
    let init: unknown;
    if (SYNTH_INITS[id]) init = SYNTH_INITS[id]();
    else if (ENGINE_INITS[id]) init = ENGINE_INITS[id]();
    else {
      // Nebula plays layers; each layer's patch is merged over this base
      const p = defaultPatch();
      p.osc = p.osc.map((o) => ({ ...o, on: false, route: 'A' }));
      p.filters = [defaultFilter({ on: false }), defaultFilter({ on: false })];
      init = p;
    }
    const presets = (inst.presets ?? []).map((p) => ({ name: p.name, category: p.category, state: p.state }));
    const text = JSON.stringify({ id, name: inst.name, init, presets }, null, 0);
    writeFileSync(`${process.env.PRESETS_OUT}/${id}.json`, text + '\n');
    console.log(id, presets.length, text.length);
  }
});
