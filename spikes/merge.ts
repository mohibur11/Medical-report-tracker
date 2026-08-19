/**
 * Phase 0 spike 1: prove the export engine on real documents.
 *
 * Takes a folder of mixed JPEG/PNG/PDF sources and produces one merged,
 * A4-normalized PDF the way the real export will, measuring the three numbers
 * the plan gates on: wall time, output size, and page count.
 *
 * It also exercises the two failure modes that decide whether an export looks
 * professional or broken:
 *   - EXIF orientation (reported per file; baking is Phase 1's job, this spike
 *     tells you how many files WOULD land sideways without it)
 *   - output size against the 25 MB email ceiling
 *
 * Usage:  node spikes/merge.ts "D:\path\to\scans" [--limit 30] [--preset standard|email|original]
 */

import { execFile } from 'node:child_process';
import { mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { basename, join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { readJpegOrientation, sniff, type FileKind } from '../src/lib/ingest/sniff.ts';

const run = promisify(execFile);

// The bundled sidecar, which `npm run fetch:sidecar` puts in place. Override
// with PDFCPU_PATH to try a different build.
const PDFCPU =
  process.env.PDFCPU_PATH ?? 'src-tauri/binaries/pdfcpu-x86_64-pc-windows-msvc.exe';

type Preset = 'original' | 'standard' | 'email';

const args = process.argv.slice(2);

/** Returns undefined when the flag is absent — indexOf returning -1 would
 *  otherwise read argv[0] and silently consume the folder path as the value. */
function flag(name: string): string | undefined {
  const i = args.indexOf(`--${name}`);
  return i === -1 ? undefined : args[i + 1];
}

const flagValues = new Set(['limit', 'preset'].map(flag).filter(Boolean) as string[]);
const root = args.find((a) => !a.startsWith('--') && !flagValues.has(a));
const limit = Number(flag('limit')) || 30;
const preset = (flag('preset') as Preset) ?? 'standard';

if (!root) {
  console.error('usage: node spikes/merge.ts <folder> [--limit N] [--preset standard|email|original]');
  process.exit(2);
}

const OUT = resolve('spikes/out');
const STAGE = join(OUT, 'stage');

async function* walk(dir: string): AsyncGenerator<string> {
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, e.name);
    if (e.isDirectory()) yield* walk(full);
    else if (e.isFile()) yield full;
  }
}

interface Source {
  path: string;
  kind: FileKind;
  orientation: number;
  size: number;
}

console.log(`\n=== MERGE SPIKE ===`);
console.log(`source : ${resolve(root)}`);
console.log(`preset : ${preset}`);
console.log(`limit  : ${limit} files\n`);

await rm(OUT, { recursive: true, force: true });
await mkdir(STAGE, { recursive: true });

const sources: Source[] = [];
for await (const file of walk(resolve(root))) {
  if (sources.length >= limit) break;
  const buf = await readFile(file);
  const kind = sniff(buf);
  if (kind !== 'jpeg' && kind !== 'png' && kind !== 'pdf') continue;
  sources.push({
    path: file,
    kind,
    orientation: kind === 'jpeg' ? readJpegOrientation(buf) : 1,
    size: (await stat(file)).size,
  });
}

if (sources.length === 0) {
  console.error('no JPEG/PNG/PDF sources found');
  process.exit(1);
}

const sideways = sources.filter((s) => s.orientation !== 1);
const inputBytes = sources.reduce((a, s) => a + s.size, 0);

console.log(`sources        : ${sources.length}  (${(inputBytes / 1024 / 1024).toFixed(1)} MB)`);
console.log(`  images       : ${sources.filter((s) => s.kind !== 'pdf').length}`);
console.log(`  pdfs         : ${sources.filter((s) => s.kind === 'pdf').length}`);
console.log(`  non-upright  : ${sideways.length}  <- would land sideways without baking\n`);

const t0 = performance.now();

// Images become single-page A4 PDFs. 'f:A4' fits the image to the page and
// 'pos:c' centres it, which is what keeps a continuous-scroll viewer from
// jumping and rezooming between records.
const parts: string[] = [];
let idx = 0;
for (const s of sources) {
  const stem = String(idx++).padStart(4, '0');
  if (s.kind === 'pdf') {
    parts.push(s.path);
    continue;
  }
  const out = join(STAGE, `${stem}.pdf`);
  await run(PDFCPU, ['import', 'f:A4, pos:c, sc:1.0 rel', out, s.path]);
  parts.push(out);
}
const tImport = performance.now();

const merged = join(OUT, 'merged.pdf');
await run(PDFCPU, ['merge', '--bookmarks', merged, ...parts], { maxBuffer: 64 * 1024 * 1024 });
const tMerge = performance.now();

// Normalize every page to A4 portrait so mixed source sizes do not survive into
// the export. 'A4P' enforces portrait, rotating landscape sources to fit.
const normalized = join(OUT, 'normalized.pdf');
await run(PDFCPU, ['resize', 'formsize:A4P', merged, normalized]);
const tResize = performance.now();

const final = join(OUT, `export-${preset}.pdf`);
await run(PDFCPU, ['optimize', normalized, final]);
const tOptimize = performance.now();

const { stdout: info } = await run(PDFCPU, ['info', '--json', final], { maxBuffer: 16 * 1024 * 1024 });
const pageCount = String(JSON.parse(info)?.infos?.[0]?.pageCount ?? '?');
const outBytes = (await stat(final)).size;

const ms = (a: number, b: number) => `${((b - a) / 1000).toFixed(2)}s`;
const mb = (b: number) => `${(b / 1024 / 1024).toFixed(1)} MB`;

console.log('--- timing ---');
console.log(`  image -> A4 pdf : ${ms(t0, tImport)}`);
console.log(`  merge           : ${ms(tImport, tMerge)}`);
console.log(`  resize A4       : ${ms(tMerge, tResize)}`);
console.log(`  optimize        : ${ms(tResize, tOptimize)}`);
console.log(`  TOTAL           : ${ms(t0, tOptimize)}`);

console.log('\n--- output ---');
console.log(`  file   : ${final}`);
console.log(`  pages  : ${pageCount}`);
console.log(`  size   : ${mb(outBytes)}   (input was ${mb(inputBytes)})`);
console.log(`  ratio  : ${(outBytes / inputBytes).toFixed(2)}x`);

console.log('\n--- gates ---');
const emailOk = outBytes <= 25 * 1024 * 1024;
console.log(`  under 25 MB email cap : ${emailOk ? 'PASS' : 'FAIL'} (${mb(outBytes)})`);
if (!emailOk) {
  const perPage = outBytes / Number(pageCount || 1);
  console.log(`    ~${mb(perPage)}/page -> Email-safe preset (grayscale 150dpi) or auto-split required`);
}
console.log(`  orientation risk      : ${sideways.length === 0 ? 'none in this sample' : `${sideways.length} file(s) need baking`}`);
for (const s of sideways.slice(0, 10)) {
  console.log(`      orientation ${s.orientation}  ${basename(s.path)}`);
}

await writeFile(
  join(OUT, 'spike-result.json'),
  JSON.stringify(
    {
      sourceCount: sources.length,
      inputBytes,
      outBytes,
      pageCount: Number(pageCount) || null,
      sidewaysCount: sideways.length,
      totalSeconds: (tOptimize - t0) / 1000,
      preset,
    },
    null,
    2,
  ),
);
console.log(`\nwrote ${join(OUT, 'spike-result.json')}\n`);
