/**
 * Generate test photos carrying real EXIF orientation tags.
 *
 * Phones write these constantly and no PDF library reads them, so these are the
 * files that expose whether ingest bakes orientation correctly. Drop the output
 * folder into the app: every one of them should appear upright, and the rotated
 * ones should report "rotated upright".
 *
 * Usage: node spikes/make-fixtures.mjs
 */

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const OUT = 'tests/fixtures/photos';

/** Splice an EXIF APP1 segment carrying `orientation` into an existing JPEG. */
function withOrientation(jpeg, orientation) {
  const u16 = (v) => [v & 0xff, (v >> 8) & 0xff];
  const u32 = (v) => [v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, (v >> 24) & 0xff];

  const tiff = [
    0x49, 0x49, ...u16(0x2a), ...u32(8), // little-endian, IFD0 at offset 8
    ...u16(1),                            // one entry
    ...u16(0x0112), ...u16(3), ...u32(1), // Orientation, SHORT, count 1
    ...u16(orientation), ...u16(0),       // value, padded to 4 bytes
    ...u32(0),                            // no next IFD
  ];

  const exif = [0x45, 0x78, 0x69, 0x66, 0x00, 0x00, ...tiff]; // "Exif\0\0"
  const segLen = exif.length + 2;

  // Strip any APP1 the source already had, then insert ours right after SOI.
  let body = jpeg.subarray(2);
  if (body[0] === 0xff && body[1] === 0xe1) {
    const existing = (body[2] << 8) | body[3];
    body = body.subarray(2 + existing);
  }

  return Buffer.concat([
    Buffer.from([0xff, 0xd8, 0xff, 0xe1, (segLen >> 8) & 0xff, segLen & 0xff]),
    Buffer.from(exif),
    body,
  ]);
}

await mkdir(OUT, { recursive: true });

// A landscape source. Tagged 6 or 8 it becomes a portrait page — which is exactly
// what a phone produces when you photograph a report holding the phone upright.
const source = await readFile('tests/fixtures/lab-report-synthetic.jpg');

const cases = [
  ['01-upright.jpg', 1, 'already correct — must pass through untouched'],
  ['02-rotate-90cw.jpg', 6, 'the common phone case'],
  ['03-rotate-270cw.jpg', 8, 'phone held the other way'],
  ['04-upside-down.jpg', 3, 'rotated 180'],
  ['05-mirrored.jpg', 2, 'mirror horizontal — must not be treated as a rotation'],
];

for (const [name, orientation, why] of cases) {
  await writeFile(join(OUT, name), withOrientation(source, orientation));
  console.log(`${name.padEnd(22)} orientation ${orientation}  ${why}`);
}

// Byte-identical copy of the first file: must be caught by content hash, not name.
await writeFile(join(OUT, '06-duplicate-of-01.jpg'), withOrientation(source, 1));
console.log('06-duplicate-of-01.jpg orientation 1  identical bytes — must be rejected as duplicate');

// An iPhone HEIC header. Nothing in this stack decodes HEIC; it must fail with a
// message the user can act on rather than as a corrupt JPEG.
const heic = Buffer.concat([
  Buffer.from([0, 0, 0, 0x18]),
  Buffer.from('ftypheic'),
  Buffer.alloc(64),
]);
await writeFile(join(OUT, '07-iphone.heic'), heic);
console.log('07-iphone.heic         —            must be skipped with an actionable message');

console.log(`\nwrote ${cases.length + 2} fixtures to ${OUT}`);
