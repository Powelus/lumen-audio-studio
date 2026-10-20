// SPDX-License-Identifier: GPL-3.0-or-later
// Renders a .las project with the web version (Powelus/pow-daw) in headless
// Chromium and writes the mix as raw f32 little-endian: the left channel, then
// the right. The Rust renderer is compared against this. Build the web app and
// start `npm run preview` (port 4173) first, copy this file to <pow-daw>/scripts/
// and run it from the repository root:
//
//   node scripts/zz_render_web.mjs <in.las> <out.f32> [sampleRate] [tailSeconds] [chromium] [stems]
//
// With `stems` as the last argument the master and each used insert are written one
// after the other (left, then right, of each), and the names are printed.
import { chromium } from 'playwright';
import { readFileSync, writeFileSync } from 'node:fs';

const [input, output, rate = '48000', tail = '2', exe, stems] = process.argv.slice(2);
const executablePath = exe ?? process.env.CHROMIUM_PATH;
const browser = await chromium.launch({ executablePath, args: ['--autoplay-policy=no-user-gesture-required'] });
const page = await browser.newPage();
page.on('pageerror', (e) => console.error('page error:', e.message));
await page.goto(process.env.WEB_URL ?? 'http://localhost:4173/');
await page.waitForFunction(() => window.__las);
const bytes = readFileSync(input).toString('base64');
const result = await page.evaluate(
  async ({ bytes, rate, tail, stems }) => {
    const L = window.__las;
    const bin = Uint8Array.from(atob(bytes), (c) => c.charCodeAt(0));
    const bundle = await L.lasFormat.decodeLas(bin);
    // a sample pool at the render's rate, so factory samples are made at that rate
    const pool = new L.audio.samples.constructor(Number(rate));
    await Promise.all([...bundle.samples.entries()].map(([id, raw]) => pool.addData(raw.data, raw.name, raw.mime, id)));
    if (stems) {
      const list = await L.render.renderStems(bundle.project, pool, { mode: 'song', sampleRate: Number(rate), tail: Number(tail) });
      const encA = (a) => {
        let s = '';
        const u = new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
        for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
        return btoa(s);
      };
      return { sr: Number(rate), names: list.map((x) => x.name), data: list.map((x) => [encA(x.result.channels[0]), encA(x.result.channels[1])]), warnings: bundle.warnings };
    }
    const r = await L.render.renderMix(bundle.project, pool, { mode: 'song', sampleRate: Number(rate), tail: Number(tail) });
    const enc = (a) => {
      let s = '';
      const u = new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
      for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
      return btoa(s);
    };
    return { sr: r.sampleRate, l: enc(r.channels[0]), r: enc(r.channels[1]), warnings: bundle.warnings };
  },
  { bytes, rate, tail, stems: !!stems },
);
if (result.names) {
  writeFileSync(output, Buffer.concat(result.data.flat().map((x) => Buffer.from(x, 'base64'))));
  console.log(JSON.stringify({ sr: result.sr, names: result.names }));
  await browser.close();
  process.exit(0);
}
const l = Buffer.from(result.l, 'base64');
const r = Buffer.from(result.r, 'base64');
writeFileSync(output, Buffer.concat([l, r]));
console.log(JSON.stringify({ sr: result.sr, frames: l.length / 4, warnings: result.warnings }));
await browser.close();
