/**
 * Phase 0 spike: survey a real folder of documents.
 *
 * Answers, against the user's actual files rather than assumptions:
 *   - what file kinds are really in there (by magic bytes, not extension)
 *   - how many extensions LIE about their contents
 *   - what fraction of photos carry a non-upright EXIF orientation flag
 *     (risk #2: untreated, these land sideways in every merged PDF)
 *   - whether any PDF is a digital text-layer PDF vs a scan
 *   - how deep the resulting vault paths would be against the 259-char ceiling
 *
 * Usage:  node spikes/survey.ts "D:\path\to\scans"
 */

import { readdir, readFile, stat } from 'node:fs/promises';
import { extname, join, resolve } from 'node:path';

import { extensionFor, needsOrientationBake, readJpegOrientation, sniff, type FileKind } from '../src/lib/ingest/sniff.ts';

const ORIENTATION_LABEL: Record<number, string> = {
  1: 'upright',
  2: 'mirrored horizontal',
  3: 'rotated 180',
  4: 'mirrored vertical',
  5: 'mirrored + rotated 270 CW',
  6: 'rotated 90 CW',
  7: 'mirrored + rotated 90 CW',
  8: 'rotated 270 CW',
};

async function* walk(dir: string): AsyncGenerator<string> {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(full);
    else if (entry.isFile()) yield full;
  }
}

const root = process.argv[2];
if (!root) {
  console.error('usage: node spikes/survey.ts <folder>');
  process.exit(2);
}

const kinds = new Map<FileKind, number>();
const orientations = new Map<number, number>();
const liars: Array<{ path: string; ext: string; actual: FileKind }> = [];
let total = 0;
let totalBytes = 0;
let jpegCount = 0;
let pdfWithTextLayer = 0;
let pdfScanned = 0;
let longestPath = { len: 0, path: '' };

for await (const file of walk(resolve(root))) {
  const head = Buffer.alloc(65536);
  let bytesRead = 0;
  try {
    const fh = await (await import('node:fs/promises')).open(file, 'r');
    ({ bytesRead } = await fh.read(head, 0, 65536, 0));
    await fh.close();
  } catch {
    continue;
  }

  const buf = head.subarray(0, bytesRead);
  const kind = sniff(buf);
  if (kind === 'unknown') continue;

  total++;
  totalBytes += (await stat(file)).size;
  kinds.set(kind, (kinds.get(kind) ?? 0) + 1);

  if (file.length > longestPath.len) longestPath = { len: file.length, path: file };

  const declared = extname(file).slice(1).toLowerCase();
  const expected = extensionFor(kind);
  const aliases: Record<string, string[]> = { jpg: ['jpg', 'jpeg'], tif: ['tif', 'tiff'] };
  const ok = (aliases[expected] ?? [expected]).includes(declared);
  if (!ok) liars.push({ path: file, ext: declared || '(none)', actual: kind });

  if (kind === 'jpeg') {
    jpegCount++;
    // Orientation lives in APP1, always near the head of the file.
    const o = readJpegOrientation(buf);
    orientations.set(o, (orientations.get(o) ?? 0) + 1);
  }

  if (kind === 'pdf') {
    // A digital PDF has font resources; a pure scan is one big image XObject.
    // Cheap heuristic on the first 64 KB — the real check is pdf.js getTextContent.
    const asText = buf.toString('latin1');
    if (/\/Font\b/.test(asText) && !/\/Subtype\s*\/Image/.test(asText)) pdfWithTextLayer++;
    else pdfScanned++;
  }
}

const pct = (n: number, d: number) => (d === 0 ? '0.0' : ((n / d) * 100).toFixed(1));

console.log(`\n=== SURVEY: ${resolve(root)} ===\n`);
console.log(`files recognised : ${total}`);
console.log(`total size       : ${(totalBytes / 1024 / 1024).toFixed(1)} MB`);
console.log(`mean size        : ${total ? (totalBytes / total / 1024 / 1024).toFixed(2) : '0'} MB\n`);

console.log('--- kinds (by magic bytes) ---');
for (const [k, n] of [...kinds].sort((a, b) => b[1] - a[1])) {
  console.log(`  ${k.padEnd(8)} ${String(n).padStart(5)}  ${pct(n, total)}%`);
}

console.log('\n--- EXIF orientation (JPEG only) ---');
if (jpegCount === 0) {
  console.log('  no JPEGs found');
} else {
  let needBake = 0;
  for (const [o, n] of [...orientations].sort((a, b) => a[0] - b[0])) {
    if (needsOrientationBake(o as 1)) needBake += n;
    console.log(`  ${String(o)}  ${(ORIENTATION_LABEL[o] ?? '?').padEnd(26)} ${String(n).padStart(5)}  ${pct(n, jpegCount)}%`);
  }
  console.log(`\n  >>> ${needBake}/${jpegCount} (${pct(needBake, jpegCount)}%) REQUIRE orientation baking at ingest.`);
  console.log('      Untreated, exactly these land sideways in every merged PDF.');
}

console.log('\n--- PDFs ---');
console.log(`  likely digital (text layer) : ${pdfWithTextLayer}   <- near-free date extraction`);
console.log(`  likely scanned (image only) : ${pdfScanned}   <- needs OCR`);

console.log('\n--- extension lies ---');
if (liars.length === 0) console.log('  none');
else {
  console.log(`  ${liars.length}/${total} (${pct(liars.length, total)}%) files whose extension disagrees with their bytes:`);
  for (const l of liars.slice(0, 15)) console.log(`    .${l.ext.padEnd(6)} is actually ${l.actual.padEnd(6)}  ${l.path}`);
  if (liars.length > 15) console.log(`    ... and ${liars.length - 15} more`);
}

console.log('\n--- path budget ---');
console.log(`  longest source path: ${longestPath.len} chars`);
console.log(`  ${longestPath.path}`);
console.log(`  (vault ceiling is 259; LongPathsEnabled=0 on this machine)\n`);
