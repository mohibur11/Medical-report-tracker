/**
 * File type and EXIF orientation detection from magic bytes.
 *
 * Extensions are never trusted: scanner and "scan to PDF" phone apps routinely
 * emit .pdf files whose bytes are a bare JPEG, and WhatsApp strips extensions
 * entirely. Everything downstream — the vault extension, whether OCR runs, and
 * whether orientation must be baked — keys off what is actually in the file.
 */

export type FileKind = 'jpeg' | 'png' | 'pdf' | 'heic' | 'tiff' | 'webp' | 'unknown';

/** EXIF orientation values 2,4,5,7 are mirrored; 3,6,8 are pure rotations.
 *  1 means upright. Anything but 1 lands sideways in a merged PDF if not baked. */
export type Orientation = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;

const startsWith = (b: Uint8Array, sig: number[], offset = 0): boolean =>
  sig.every((v, i) => b[offset + i] === v);

/** Reading past the end yields 0 rather than undefined. Every caller below has
 *  already bounds-checked; this keeps the bit arithmetic honest about its type
 *  instead of propagating `number | undefined` through every shift. */
const at = (b: Uint8Array, i: number): number => b[i] ?? 0;

export function sniff(buf: Uint8Array): FileKind {
  if (buf.length < 12) return 'unknown';

  if (startsWith(buf, [0xff, 0xd8, 0xff])) return 'jpeg';
  if (startsWith(buf, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return 'png';
  if (startsWith(buf, [0x25, 0x50, 0x44, 0x46])) return 'pdf'; // %PDF
  if (startsWith(buf, [0x49, 0x49, 0x2a, 0x00]) || startsWith(buf, [0x4d, 0x4d, 0x00, 0x2a])) return 'tiff';

  // RIFF....WEBP
  if (startsWith(buf, [0x52, 0x49, 0x46, 0x46]) && startsWith(buf, [0x57, 0x45, 0x42, 0x50], 8)) return 'webp';

  // ISO-BMFF: ....ftyp<brand>
  if (startsWith(buf, [0x66, 0x74, 0x79, 0x70], 4)) {
    const brand = String.fromCharCode(...buf.slice(8, 12));
    if (['heic', 'heix', 'hevc', 'hevx', 'heim', 'heis', 'mif1', 'msf1'].includes(brand)) return 'heic';
  }

  return 'unknown';
}

/** Vault extension for a sniffed kind. Deliberately does not round-trip the
 *  source extension — a JPEG named .pdf becomes .jpg. */
export function extensionFor(kind: FileKind): string {
  switch (kind) {
    case 'jpeg': return 'jpg';
    case 'png': return 'png';
    case 'pdf': return 'pdf';
    case 'heic': return 'heic';
    case 'tiff': return 'tif';
    case 'webp': return 'webp';
    default: return 'bin';
  }
}

/**
 * Read the EXIF orientation tag (0x0112) out of a JPEG's APP1 segment.
 * Returns 1 when there is no EXIF block or no orientation tag — the same
 * treatment a viewer gives it.
 *
 * Implemented directly rather than pulled from a library because it is the only
 * EXIF field the app needs, and it runs on every imported photo.
 */
export function readJpegOrientation(buf: Uint8Array): Orientation {
  if (!startsWith(buf, [0xff, 0xd8, 0xff])) return 1;

  let p = 2;
  while (p + 4 <= buf.length) {
    if (at(buf, p) !== 0xff) { p++; continue; }

    const marker = at(buf, p + 1);
    // Standalone markers carry no length payload.
    if (marker === 0xd8 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) { p += 2; continue; }
    if (marker === 0xda || marker === 0xd9) break; // start of scan / end of image

    const segLen = (at(buf, p + 2) << 8) | at(buf, p + 3);
    if (segLen < 2) break;

    if (marker === 0xe1 && p + 4 + 6 <= buf.length) {
      const header = String.fromCharCode(...buf.slice(p + 4, p + 10));
      if (header === 'Exif\0\0') {
        const tiff = p + 10;
        const found = readOrientationFromTiff(buf, tiff, Math.min(tiff + segLen, buf.length));
        if (found) return found;
      }
    }

    p += 2 + segLen;
  }
  return 1;
}

function readOrientationFromTiff(buf: Uint8Array, tiff: number, end: number): Orientation | null {
  if (tiff + 8 > end) return null;

  const le = at(buf, tiff) === 0x49 && at(buf, tiff + 1) === 0x49;
  const be = at(buf, tiff) === 0x4d && at(buf, tiff + 1) === 0x4d;
  if (!le && !be) return null;

  const u16 = (o: number) =>
    le ? at(buf, o) | (at(buf, o + 1) << 8) : (at(buf, o) << 8) | at(buf, o + 1);
  const u32 = (o: number) =>
    le
      ? (at(buf, o) | (at(buf, o + 1) << 8) | (at(buf, o + 2) << 16) | (at(buf, o + 3) << 24)) >>> 0
      : ((at(buf, o) << 24) | (at(buf, o + 1) << 16) | (at(buf, o + 2) << 8) | at(buf, o + 3)) >>> 0;

  if (u16(tiff + 2) !== 0x2a) return null;

  const ifd0 = tiff + u32(tiff + 4);
  if (ifd0 + 2 > end) return null;

  const count = u16(ifd0);
  for (let i = 0; i < count; i++) {
    const entry = ifd0 + 2 + i * 12;
    if (entry + 12 > end) break;
    if (u16(entry) === 0x0112) {
      const v = u16(entry + 8);
      return v >= 1 && v <= 8 ? (v as Orientation) : null;
    }
  }
  return null;
}

/** True when the pixels must be rewritten at ingest. Untreated, these land
 *  sideways or mirrored in the merged PDF — no PDF library reads the tag. */
export function needsOrientationBake(o: Orientation): boolean {
  return o !== 1;
}
