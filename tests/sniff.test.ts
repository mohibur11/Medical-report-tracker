import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  extensionFor,
  needsOrientationBake,
  readJpegOrientation,
  sniff,
} from '../src/lib/ingest/sniff.ts';

/** Build a minimal JPEG carrying an EXIF APP1 segment with the given orientation. */
function jpegWithOrientation(orientation: number, littleEndian = true): Uint8Array {
  const tiff: number[] = [];
  const u16 = (v: number) => (littleEndian ? [v & 0xff, (v >> 8) & 0xff] : [(v >> 8) & 0xff, v & 0xff]);
  const u32 = (v: number) =>
    littleEndian
      ? [v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, (v >> 24) & 0xff]
      : [(v >> 24) & 0xff, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff];

  tiff.push(...(littleEndian ? [0x49, 0x49] : [0x4d, 0x4d]));
  tiff.push(...u16(0x2a));
  tiff.push(...u32(8));            // IFD0 offset, relative to TIFF header
  tiff.push(...u16(1));            // one entry
  tiff.push(...u16(0x0112));       // Orientation tag
  tiff.push(...u16(3));            // SHORT
  tiff.push(...u32(1));            // count
  tiff.push(...u16(orientation));  // value, inline
  tiff.push(...u16(0));            // padding of the 4-byte value field
  tiff.push(...u32(0));            // next IFD = none

  const exif = [0x45, 0x78, 0x69, 0x66, 0x00, 0x00, ...tiff]; // "Exif\0\0" + TIFF
  const segLen = exif.length + 2;

  return new Uint8Array([
    0xff, 0xd8,                            // SOI
    0xff, 0xe1, (segLen >> 8) & 0xff, segLen & 0xff, ...exif,
    0xff, 0xdb, 0x00, 0x04, 0x00, 0x00,    // a dummy DQT so the walker advances
    0xff, 0xda, 0x00, 0x02,                // SOS — parser stops here
  ]);
}

test('magic bytes identify each supported kind', () => {
  assert.equal(sniff(new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0, 0, 0, 0, 0])), 'jpeg');
  assert.equal(sniff(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0])), 'png');
  assert.equal(sniff(new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37, 0, 0, 0, 0])), 'pdf');
  assert.equal(sniff(new Uint8Array([0x49, 0x49, 0x2a, 0x00, 0, 0, 0, 0, 0, 0, 0, 0])), 'tiff');
});

test('HEIC is detected by its ftyp brand, not its extension', () => {
  const heic = new Uint8Array([
    0, 0, 0, 0x18, 0x66, 0x74, 0x79, 0x70, 0x68, 0x65, 0x69, 0x63,
  ]);
  assert.equal(sniff(heic), 'heic');
  // The iPhone case that must fail loudly rather than silently: nothing in this
  // stack decodes HEIC, so it has to be recognised in order to be reported.
  assert.equal(extensionFor('heic'), 'heic');
});

test('WEBP needs both the RIFF header and the WEBP form type', () => {
  const webp = new Uint8Array([0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x45, 0x42, 0x50]);
  assert.equal(sniff(webp), 'webp');

  const riffWav = new Uint8Array([0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x41, 0x56, 0x45]);
  assert.equal(sniff(riffWav), 'unknown');
});

test('a JPEG masquerading as a PDF is caught by its bytes', () => {
  // Exactly what "scan to PDF" phone apps emit. Trusting the extension here
  // sends a JPEG into the PDF merge path and corrupts the export.
  const jpegBytes = new Uint8Array([0xff, 0xd8, 0xff, 0xe1, 0, 0, 0, 0, 0, 0, 0, 0]);
  assert.equal(sniff(jpegBytes), 'jpeg');
  assert.equal(extensionFor(sniff(jpegBytes)), 'jpg');
});

test('truncated and empty buffers are unknown, not a crash', () => {
  assert.equal(sniff(new Uint8Array([])), 'unknown');
  assert.equal(sniff(new Uint8Array([0xff, 0xd8])), 'unknown');
  assert.equal(extensionFor('unknown'), 'bin');
});

test('EXIF orientation is read for every rotation and mirror value', () => {
  for (let o = 1; o <= 8; o++) {
    assert.equal(readJpegOrientation(jpegWithOrientation(o)), o, `little-endian orientation ${o}`);
  }
});

test('big-endian (Motorola) EXIF is read correctly', () => {
  assert.equal(readJpegOrientation(jpegWithOrientation(6, false)), 6);
  assert.equal(readJpegOrientation(jpegWithOrientation(8, false)), 8);
});

test('a JPEG with no EXIF block reports upright', () => {
  const bare = new Uint8Array([0xff, 0xd8, 0xff, 0xdb, 0x00, 0x04, 0x00, 0x00, 0xff, 0xda, 0x00, 0x02]);
  assert.equal(readJpegOrientation(bare), 1);
});

test('a non-JPEG reports upright rather than misreading bytes', () => {
  assert.equal(readJpegOrientation(new Uint8Array([0x25, 0x50, 0x44, 0x46, 0, 0, 0, 0])), 1);
});

test('only orientation 1 skips the bake', () => {
  assert.equal(needsOrientationBake(1), false);
  for (const o of [2, 3, 4, 5, 6, 7, 8] as const) {
    assert.equal(needsOrientationBake(o), true, `orientation ${o} must be baked`);
  }
});
