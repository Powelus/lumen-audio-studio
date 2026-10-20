// SPDX-License-Identifier: GPL-3.0-or-later
// Small projects for checking the Rust mixer and scheduler against the web
// version: the instruments, effects, routing, sidechain and arpeggiator.
// Patches avoid unseeded randomness (random phase, noise), so the web render is
// repeatable. Run from a checkout of the web repository, then render each file
// with tools/project/render-web.mjs (see that file):
//
//   cp tools/project/mix.gen.test.ts <pow-daw>/tests/zz_mix_export.test.ts
//   MIX_OUT=<this repo>/crates/las-engine/tests/fixtures npx vitest run tests/zz_mix_export.test.ts
import { mkdirSync, writeFileSync } from 'node:fs';
import { it } from 'vitest';
import { createChannel, createEffectSlot, emptyProject } from '../src/core/project';
import type { Project } from '../src/core/types';
import { encodeLas } from '../src/storage/lasFormat';

const out = process.env.MIX_OUT!;
const BEAT = 96;
let seq = 0;
const note = (key: number, beat: number, beats: number, vel = 0.8, extra = {}) => ({ id: `n${++seq}`, key, pos: Math.round(beat * BEAT), len: Math.round(beats * BEAT), vel, pan: 0, ...extra });

/** Four steady oscillators-off patch with one saw or sine voice, no randomness. */
function patch(table: string, extra: Record<string, unknown> = {}) {
  return {
    osc: [{ on: true, table, voices: 1, level: 0.8, randPhase: 0, phase: 0 }, { on: false, randPhase: 0 }, { on: false, randPhase: 0 }, { on: false, randPhase: 0 }],
    env: [{ a: 0.005, h: 0, d: 0.2, s: 0.7, r: 0.15 }],
    level: 0.6,
    ...extra,
  };
}

function base(name: string, bpm = 120): Project {
  const p = emptyProject(name);
  p.bpm = bpm;
  p.masterVolume = 0.9;
  p.meta = { createdAt: 1767225600000, updatedAt: 1767225600000 };
  return p;
}

function place(p: Project, beats: number) {
  p.playlist.clips.push({ id: 'clip1', kind: 'pattern', refId: p.patterns[0].id, track: 0, start: 0, length: beats * BEAT, offset: 0 });
}

const cases: Record<string, () => Project> = {
  // one polaris voice, three notes, straight to the master
  single: () => {
    const p = base('single');
    const ch = createChannel('polaris', { name: 'Lead', mixerTrack: 1, state: { patch: patch('saw', { filters: [{ on: true, type: 'lp24', cutoff: 3000, res: 0.2 }] }), fx: [], arp: { on: false } } });
    p.channels.push(ch);
    p.patterns[0].notes[ch.id] = [note(60, 0, 1), note(64, 1, 0.5, 0.5), note(67, 2, 1.5, 1)];
    place(p, 4);
    return p;
  },
  // faders, pan, width, effects (one with latency), a send to a reverb, master limiter
  mixer: () => {
    const p = base('mixer', 128);
    const a = createChannel('polaris', { name: 'A', mixerTrack: 1, volume: 0.8, pan: 0.3, state: { patch: patch('saw'), fx: [], arp: { on: false } } });
    const b = createChannel('triosc', { name: 'B', mixerTrack: 2, state: { patch: patch('sine'), fx: [], arp: { on: false } } });
    p.channels.push(a, b);
    p.patterns[0].notes[a.id] = [note(57, 0, 1), note(60, 1, 1), note(64, 2, 2)];
    p.patterns[0].notes[b.id] = [note(72, 0.5, 1), note(76, 2, 1.5, 0.6)];
    p.mixer[1].volume = 0.7;
    p.mixer[1].pan = -0.4;
    p.mixer[1].width = 1.5;
    p.mixer[1].slots[0] = createEffectSlot('las-eq', { bands: [{ on: true, type: 'highpass', freq: 200, gain: 0, q: 0.7 }] });
    p.mixer[1].slots[1] = createEffectSlot('las-limiter');
    p.mixer[2].sends.push({ target: 3, amount: 0.4, sidechainOnly: false });
    p.mixer[2].slots[0] = createEffectSlot('las-dist', { drive: 6, mix: 0.5 });
    p.mixer[3].name = 'Reverb';
    p.mixer[3].slots[0] = createEffectSlot('las-hall', { decay: 1.5, wet: 1, dry: 0 });
    p.mixer[0].slots[0] = createEffectSlot('las-limiter');
    place(p, 5);
    return p;
  },
  // a kick insert ducks a pad's compressor: once by choosing the source, once the FL way
  sidechain: () => {
    const p = base('sidechain', 124);
    const kick = createChannel('polaris', { name: 'Kick', mixerTrack: 1, state: { patch: patch('sine', { env: [{ a: 0.001, h: 0, d: 0.12, s: 0, r: 0.05 }] }), fx: [], arp: { on: false } } });
    const pad = createChannel('polaris', { name: 'Pad', mixerTrack: 2, state: { patch: patch('saw', { env: [{ a: 0.01, h: 0, d: 0.1, s: 1, r: 0.1 }] }), fx: [], arp: { on: false } } });
    const pad2 = createChannel('polaris', { name: 'Pad 2', mixerTrack: 3, state: { patch: patch('saw', { env: [{ a: 0.01, h: 0, d: 0.1, s: 1, r: 0.1 }] }), fx: [], arp: { on: false } } });
    p.channels.push(kick, pad, pad2);
    p.patterns[0].notes[kick.id] = [0, 1, 2, 3].map((b) => note(36, b, 0.25, 1));
    p.patterns[0].notes[pad.id] = [note(52, 0, 4, 0.7)];
    p.patterns[0].notes[pad2.id] = [note(55, 0, 4, 0.7)];
    p.mixer[2].slots[0] = createEffectSlot('las-ducker', { sidechain: 1, threshold: -30, ratio: 8, attack: 1, release: 120 });
    p.mixer[1].sends.push({ target: 3, amount: 1, sidechainOnly: true });
    p.mixer[3].slots[0] = createEffectSlot('las-ducker', { sidechain: null, threshold: -30, ratio: 8, attack: 1, release: 120 });
    place(p, 4);
    return p;
  },
  // facet with arp and an effects rack, mist with its texture
  engines: () => {
    const p = base('engines', 110);
    const f = createChannel('facet', { name: 'Facet', mixerTrack: 1, state: { patch: {}, fx: [{ type: 'las-chorus', on: true, state: { mix: 0.4 } }, { type: 'las-delay', on: true, state: { wet: 0.3, time: 0.75 } }], arp: { on: true, mode: 'updown', div: 0.25, octaves: 2, gate: 0.6 } } });
    const m = createChannel('mist', { name: 'Mist', mixerTrack: 2, state: { patch: {}, fx: [{ type: 'las-hall', on: true, state: { decay: 2, wet: 0.3 } }], arp: { on: false } } });
    p.channels.push(f, m);
    p.patterns[0].notes[f.id] = [note(60, 0, 2), note(64, 0, 2), note(67, 0, 2), note(62, 2, 2), note(65, 2, 2), note(69, 2, 2)];
    p.patterns[0].notes[m.id] = [note(48, 0, 3, 0.9)];
    place(p, 4);
    return p;
  },
  // nebula: two layers, macros, chord arp, plus a polaris with a rack
  nebula: () => {
    const p = base('nebula', 100);
    const n = createChannel('nebula', {
      name: 'Nebula',
      mixerTrack: 1,
      state: {
        layers: [
          { on: true, name: 'A', level: 0.7, pan: -0.3, transpose: 0, patch: patch('saw', { filters: [{ on: true, type: 'lp12', cutoff: 2000, res: 0.1 }] }) },
          { on: true, name: 'B', level: 0.5, pan: 0.4, transpose: 12, patch: patch('sine') },
          { on: false, name: 'C', level: 1, pan: 0, transpose: 0, patch: patch('saw') },
        ],
        macros: { cutoff: 0.7, res: 0.1, attack: 0.3, release: 0.6 },
        master: 0.7,
        fx: [{ type: 'las-phaser', on: true, state: { mix: 0.5 } }],
        arp: { on: true, mode: 'chord', div: 0.5, octaves: 1, gate: 0.8 },
      },
    });
    p.channels.push(n);
    p.patterns[0].notes[n.id] = [note(57, 0, 2), note(60, 0, 2), note(64, 0, 2), note(55, 2, 2), note(59, 2, 2), note(62, 2, 2)];
    place(p, 4);
    return p;
  },
};

// the sampler: factory drums as step (one-shot) and piano roll (gated) notes, with
// pan, velocity, tuning, envelope, filter, boost, loop, reverse and choke
cases.sampler = () => {
  const p = base('sampler', 120);
  const kick = createChannel('sampler', { name: 'Kick', mixerTrack: 1, sampleId: 'factory:kick-punch', volume: 0.9, state: { root: 60, tune: 0, fine: 0, attack: 0.001, decay: 0.3, sustain: 1, release: 0.08, reverse: false, loop: false, start: 0, end: 1, cut: false, filterType: 'lowpass', cutoff: 20000, res: 0.7, velToVol: 0.7, boost: 0 } });
  const snare = createChannel('sampler', { name: 'Snare', mixerTrack: 2, sampleId: 'factory:snare-fat', pan: 0.3, state: { root: 60, tune: -2, fine: 15, attack: 0.002, decay: 0.2, sustain: 0.6, release: 0.1, reverse: false, loop: false, start: 0.05, end: 0.8, cut: false, filterType: 'lowpass', cutoff: 3000, res: 4, velToVol: 0.5, boost: 2 } });
  const hat = createChannel('sampler', { name: 'Hat', mixerTrack: 3, sampleId: 'factory:hat-open', state: { root: 60, tune: 5, fine: 0, attack: 0.001, decay: 0.3, sustain: 1, release: 0.05, reverse: false, loop: false, start: 0, end: 1, cut: true, filterType: 'highpass', cutoff: 2000, res: 1, velToVol: 0.7, boost: 0 } });
  const rev = createChannel('sampler', { name: 'Rev', mixerTrack: 4, sampleId: 'factory:clap-wide', state: { root: 60, tune: 0, fine: 0, attack: 0.01, decay: 0.3, sustain: 1, release: 0.1, reverse: true, loop: false, start: 0, end: 1, cut: false, filterType: 'lowpass', cutoff: 20000, res: 0.7, velToVol: 0.7, boost: 0 } });
  const loop = createChannel('sampler', { name: 'Loop', mixerTrack: 5, sampleId: 'factory:808-c', state: { root: 36, tune: 0, fine: 0, attack: 0.005, decay: 0.3, sustain: 1, release: 0.2, reverse: false, loop: true, start: 0.1, end: 0.3, cut: false, filterType: 'lowpass', cutoff: 20000, res: 0.7, velToVol: 0.7, boost: 0 } });
  p.channels.push(kick, snare, hat, rev, loop);
  p.patterns[0].notes[kick.id] = [0, 4, 8, 12].map((s) => note(60, s / 4, 0.25, 1, { step: true }));
  p.patterns[0].notes[snare.id] = [note(60, 1, 0.5, 0.8), note(64, 3, 0.25, 0.4, { pan: -0.5 })];
  p.patterns[0].notes[hat.id] = [note(60, 0.5, 0.25, 0.9, { step: true }), note(60, 0.75, 0.25, 0.5, { step: true }), note(62, 1.5, 0.25, 0.7)];
  p.patterns[0].notes[rev.id] = [note(60, 2, 1, 1, { step: true })];
  p.patterns[0].notes[loop.id] = [note(36, 0, 1.5, 0.9), note(43, 2, 1.5, 0.7)];
  p.swing = 0.4;
  place(p, 4);
  return p;
};

// automation clips: insert volume and pan, master volume, channel volume, pan and
// pitch, partly covered and offset clips, an insert with plugin latency
const auto = (p: Project, id: string, target: unknown, points: { t: number; v: number }[], start: number, length: number, offset = 0, track = 2) => {
  p.automation.push({ id, name: id, color: '#fff', target: target as never, points, length: 4 * BEAT });
  p.playlist.clips.push({ id: `c_${id}`, kind: 'automation', refId: id, track, start: start * BEAT, length: length * BEAT, offset });
};
cases.automation = () => {
  const p = base('automation', 120);
  const a = createChannel('polaris', { name: 'A', mixerTrack: 1, state: { patch: patch('saw', { env: [{ a: 0.005, h: 0, d: 0.2, s: 1, r: 0.15 }] }), fx: [], arp: { on: false } } });
  const b = createChannel('polaris', { name: 'B', mixerTrack: 2, state: { patch: patch('sine'), fx: [], arp: { on: false } } });
  p.channels.push(a, b);
  p.patterns[0].notes[a.id] = [note(48, 0, 4, 0.9), note(55, 2, 2, 0.7)];
  p.patterns[0].notes[b.id] = [note(72, 0, 1.5), note(76, 2, 1.5), note(79, 4, 1.5), note(72, 6, 2)];
  p.mixer[1].slots[0] = createEffectSlot('las-limiter');
  place(p, 8);
  const ramp = (...v: number[]) => v.map((x, i) => ({ t: i * BEAT, v: x }));
  auto(p, 'v1', { kind: 'mixer', track: 1, param: 'volume' }, ramp(1, 0.4, 0.9, 0.2), 0, 3);
  auto(p, 'p2', { kind: 'mixer', track: 2, param: 'pan' }, ramp(0.5, 0, 1, 0.5), 1, 4, BEAT / 2);
  auto(p, 'mv', { kind: 'master', param: 'volume' }, ramp(0.7, 0.9, 0.5), 0, 8);
  auto(p, 'cv', { kind: 'channel', channelId: b.id, param: 'volume' }, ramp(1, 0.5, 0.8), 2, 4, 0, 3);
  auto(p, 'cp', { kind: 'channel', channelId: a.id, param: 'pan' }, ramp(0.5, 1, 0), 4, 3, 0, 3);
  auto(p, 'pitch', { kind: 'channel', channelId: b.id, param: 'pitch' }, ramp(0.5, 0.75, 0.5, 0.25), 4, 4, 0, 4);
  return p;
};
// audio clips: gain, fades, reverse, pitch, tempo follow by stretch and by resample,
// a crossfade between overlapping clips, a clip offset into its source
cases.clips = () => {
  const p = base('clips', 120);
  const mono = createChannel('audioclip', { name: 'Mono', mixerTrack: 1, sampleId: 'factory:ride', state: { gain: -2 } });
  const stereo = createChannel('audioclip', { name: 'Stereo', mixerTrack: 2, sampleId: 'factory:stab-minor', pan: 0.2, state: { gain: 0 } });
  p.channels.push(mono, stereo);
  p.samples.push({ id: 'factory:ride', name: 'Ride', factory: true }, { id: 'factory:stab-minor', name: 'Stab', factory: true });
  const clip = (id: string, ch: string, track: number, start: number, length: number, offset = 0, audio?: Record<string, unknown>) =>
    p.playlist.clips.push({ id, kind: 'audio', refId: ch, track, start: start * BEAT, length: length * BEAT, offset, ...(audio ? { audio } : {}) } as never);
  clip('m1', mono.id, 0, 0, 2, 0, { gain: -4, fadeIn: BEAT / 4, fadeOut: BEAT / 2, fadeInCurve: 0.5, fadeOutCurve: -0.5 });
  clip('m2', mono.id, 0, 1.5, 2, 20, { stretch: { bpm: 100, mode: 'stretch' } });
  clip('m3', mono.id, 1, 5, 2, 0, { stretch: { bpm: 150, mode: 'resample' }, reverse: true });
  clip('s1', stereo.id, 2, 0, 1, 0, { pitch: 3 });
  clip('s2', stereo.id, 2, 2, 1, 12, { reverse: true, gain: 2 });
  clip('s3', stereo.id, 2, 4, 2, 0, { pitch: -5, stretch: { bpm: 90, mode: 'stretch' } });
  clip('s4', stereo.id, 3, 7, 1);
  place(p, 8);
  return p;
};
// tempo automation (the song speeds up and slows down)
cases.tempo = () => {
  const p = base('tempo', 120);
  const a = createChannel('polaris', { name: 'A', mixerTrack: 1, state: { patch: patch('saw'), fx: [], arp: { on: true, mode: 'up', div: 0.25, octaves: 1, gate: 0.6 } } });
  p.channels.push(a);
  p.patterns[0].notes[a.id] = [note(60, 0, 4), note(64, 0, 4), note(67, 0, 4), note(62, 4, 4), note(65, 4, 4)];
  place(p, 8);
  auto(p, 'tempo', { kind: 'master', param: 'tempo' }, [{ t: 0, v: 0.2 }, { t: 4 * BEAT, v: 0.9 }], 0, 4);
  auto(p, 'tempo2', { kind: 'master', param: 'tempo' }, [{ t: 0, v: 0.9 }, { t: 4 * BEAT, v: 0.4 }], 4, 4);
  return p;
};

// effect parameter automation: worklet ramps (comp threshold, hall wet), a slot's mix,
// and state updates on a grid (chorus mix, distortion drive, filter cutoff)
const fxAuto = (name: string, effects: [string, Record<string, unknown>][], autos: [string, string, { t: number; v: number }[], number, number, number?][]): Project => {
  const p = base(name, 120);
  const ch = createChannel('polaris', { name: 'Saw', mixerTrack: 1, state: { patch: patch('saw', { env: [{ a: 0.005, h: 0, d: 0.2, s: 1, r: 0.15 }], filters: [{ on: true, type: 'lp24', cutoff: 4000, res: 0 }] }), fx: [], arp: { on: false } } });
  p.channels.push(ch);
  p.patterns[0].notes[ch.id] = [note(48, 0, 2, 0.9), note(55, 2, 2, 0.8), note(60, 4, 3, 1)];
  const slots = effects.map(([id, state]) => createEffectSlot(id, state));
  slots.forEach((slot, i) => (p.mixer[1].slots[i] = slot));
  place(p, 8);
  autos.forEach(([id, param, points, start, length, slotIndex], i) =>
    auto(p, `${id}${i}`, { kind: 'effect', track: 1, slotId: slots[slotIndex ?? 0].id, param }, points, start, length, 0, 2 + i),
  );
  return p;
};
const ramp3 = (a: number, b: number, c: number) => [{ t: 0, v: a }, { t: 2 * BEAT, v: b }, { t: 4 * BEAT, v: c }];
cases.fxa_ramped = () =>
  fxAuto('fxa_ramped', [['las-comp', { threshold: -30, ratio: 6, attack: 3, release: 80, makeup: 3 }], ['las-hall', { decay: 1.4, wet: 0.2, dry: 0.5 }]], [
    ['thr', 'threshold', ramp3(0.9, 0.2, 0.7), 0, 4, 0],
    ['mk', 'makeup', ramp3(0.2, 0.8, 0.4), 4, 4, 0],
    ['wet', 'wet', ramp3(0.1, 0.9, 0.3), 0, 8, 1],
  ]);
cases.fxa_mix = () =>
  fxAuto('fxa_mix', [['las-limiter', {}], ['las-hall', { decay: 1.2, wet: 1, dry: 0 }]], [
    ['mixa', 'mix', ramp3(0.1, 1, 0.3), 0, 4, 1],
    ['mixb', 'mix', ramp3(0.8, 0.2, 0.9), 4, 4, 1],
  ]);
cases.fxa_timed = () =>
  fxAuto('fxa_timed', [['las-filter', { type: 'lowpass', cutoff: 1500, res: 0.5, lfoRate: 0, lfoDepth: 0 }]], [
    ['cut', 'cutoff', ramp3(0.2, 0.9, 0.4), 0, 4, 0],
    ['res', 'res', ramp3(0.1, 0.6, 0.2), 4, 4, 0],
  ]);
cases.fxa_timed2 = () =>
  fxAuto('fxa_timed2', [['las-dist', { mode: 'tube', drive: 4, tone: 6000, output: -6, mix: 1 }], ['las-chorus', { rate: 0.8, depth: 0.5, mix: 0.3 }]], [
    ['drive', 'drive', ramp3(0.1, 0.8, 0.3), 0, 4, 0],
    ['cmix', 'mix', ramp3(0.2, 0.9, 0.5), 0, 8, 1],
  ]);

cases.fxa_native2 = () =>
  fxAuto('fxa_native2', [['las-phaser', { rate: 0.6, depth: 0.7, feedback: 0.3, mix: 0.7 }], ['las-stereo', { width: 1.2, pan: 0, gain: 0 }], ['las-reverb', { decay: 1.2, wet: 0.5 }]], [
    ['pc', 'center', ramp3(0.3, 0.8, 0.4), 0, 4, 0],
    ['pm', 'mix', ramp3(0.2, 0.9, 0.6), 4, 4, 0],
    ['sw', 'width', ramp3(0.2, 0.9, 0.5), 0, 4, 1],
    ['sp', 'pan', ramp3(0.2, 0.9, 0.5), 4, 4, 1],
    ['rw', 'wet', ramp3(0.1, 0.8, 0.3), 0, 4, 2],
  ]);
// the same projects without the effects that Web Audio builds from its own nodes,
// so the instruments, mixer and scheduler are compared on their own
cases.nebuladry = () => {
  const p = cases.nebula();
  const state = p.channels[0].state as { fx?: unknown[] };
  state.fx = [];
  p.name = 'nebuladry';
  return p;
};
cases.mixerdry = () => {
  const p = cases.mixer();
  p.mixer[1].width = 1;
  p.mixer[1].slots[0] = null;
  p.mixer[2].slots[0] = null;
  p.name = 'mixerdry';
  return p;
};
cases.enginesdry = () => {
  const p = cases.engines();
  const state = p.channels[0].state as { fx?: { type: string }[] };
  state.fx = (state.fx ?? []).filter((f) => f.type !== 'las-chorus');
  p.name = 'enginesdry';
  return p;
};

// each effect built from Web Audio nodes, alone on an insert
const NATIVE: Record<string, Record<string, unknown>> = {
  'las-eq': { bands: [{ on: true, type: 'lowshelf', freq: 120, gain: 6, q: 0.9 }, { on: true, type: 'peaking', freq: 800, gain: -9, q: 2 }, { on: true, type: 'peaking', freq: 2500, gain: 7, q: 1 }, { on: true, type: 'highshelf', freq: 7000, gain: -6, q: 0.9 }, { on: true, type: 'lowpass', freq: 12000, gain: 0, q: 3 }, { on: true, type: 'highpass', freq: 60, gain: 0, q: 3 }, { on: true, type: 'notch', freq: 1500, gain: 0, q: 4 }], output: -2 },
  'las-dist': { mode: 'tube', drive: 9, tone: 6000, output: -6, mix: 1 },
  'las-chorus': { rate: 0.8, depth: 0.5, mix: 0.6 },
  'las-phaser': { rate: 0.6, depth: 0.7, mix: 0.7 },
  'las-filter': { type: 'lp', cutoff: 1500, res: 0.3, lfoRate: 1.2, lfoDepth: 1 },
  'las-stereo': { width: 1.6, pan: 0.3, gain: -3 },
  'las-maximizer': {},
  'las-reverb': { decay: 1.2, wet: 0.5 },
};
for (const [id, state] of Object.entries(NATIVE)) {
  cases[`fx_${id.slice(4)}`] = () => {
    const p = base(`fx_${id}`);
    const ch = createChannel('polaris', { name: 'Saw', mixerTrack: 1, state: { patch: patch('saw', { filters: [{ on: true, type: 'lp24', cutoff: 4000, res: 0 }] }), fx: [], arp: { on: false } } });
    p.channels.push(ch);
    p.patterns[0].notes[ch.id] = [note(48, 0, 1, 0.9), note(55, 1, 1, 0.6), note(60, 2, 1.5, 1)];
    p.mixer[1].slots[0] = createEffectSlot(id, state);
    place(p, 4);
    return p;
  };
}

// the sweep without feedback, and a chorus with feedback (a Web Audio cycle runs a
// render quantum behind)
cases.fx_phaser_nofb = () => {
  const p = cases.fx_phaser();
  p.mixer[1].slots[0] = createEffectSlot('las-phaser', { rate: 0.6, depth: 0.7, feedback: 0, mix: 0.7 });
  p.name = 'phaser_nofb';
  return p;
};
cases.fx_chorus_fb = () => {
  const p = cases.fx_chorus();
  p.mixer[1].slots[0] = createEffectSlot('las-chorus', { rate: 0.8, depth: 2, delay: 9, feedback: 0.5, mix: 0.6 });
  p.name = 'chorus_fb';
  return p;
};

// each automation alone
const SPLIT: [string, [string, Record<string, unknown>], string, number, number][] = [
  ['eqf', ['las-eq', NATIVE['las-eq']], 'bands.1.freq', 0, 4],
  ['eqg', ['las-eq', NATIVE['las-eq']], 'bands.2.gain', 0, 4],
  ['eqq', ['las-eq', NATIVE['las-eq']], 'bands.3.q', 2, 4],
  ['eqo', ['las-eq', NATIVE['las-eq']], 'output', 0, 8],
  ['eqt', ['las-eq', NATIVE['las-eq']], 'bands.0.type', 4, 2],
  ['phc', ['las-phaser', { rate: 0.6, depth: 0.7, feedback: 0.3, mix: 0.7 }], 'center', 0, 4],
  ['phm', ['las-phaser', { rate: 0.6, depth: 0.7, feedback: 0.3, mix: 0.7 }], 'mix', 4, 4],
  ['stw', ['las-stereo', { width: 1.2, pan: 0, gain: 0 }], 'width', 0, 4],
  ['stp', ['las-stereo', { width: 1.2, pan: 0, gain: 0 }], 'pan', 4, 4],
  ['rvw', ['las-reverb', { decay: 1.2, wet: 0.5 }], 'wet', 0, 4],
];
for (const [name, effect, param, start, length] of SPLIT) {
  cases[`fxs_${name}`] = () => fxAuto(`fxs_${name}`, [effect], [[name, param, ramp3(0.2, 0.9, 0.4), start, length, 0]]);
}
it('writes the mix projects', async () => {
  mkdirSync(out, { recursive: true });
  for (const [name, make] of Object.entries(cases)) {
    const p = make();
    writeFileSync(`${out}/${name}.las`, await encodeLas(p, new Map(), { encrypt: false }));
    console.log(name, p.channels.length);
  }
});
