import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  MAX_PATH,
  UNKNOWN_DATE,
  buildName,
  isValidDocDate,
  patientSlug,
  slugify,
  titleSlug,
  truncateGraphemes,
  yearFolder,
} from '../src/lib/naming/sanitize.ts';

import { readFileSync } from 'node:fs';

const ROOT_LEN = 'C:\\Users\\a-user\\Documents\\MedicineReportTracker\\'.length;

/**
 * The same table `src-tauri/src/naming.rs` runs against. Two implementations of
 * the naming grammar exist on purpose — Rust owns the filesystem, TypeScript gives
 * the review grid an instant preview — and this is what stops them drifting.
 */
const CASES = JSON.parse(
  readFileSync(new URL('./fixtures/naming-cases.json', import.meta.url), 'utf8'),
) as {
  slugs: Array<{ kind: 'patient' | 'title'; input: string; want: string }>;
  dates: Array<{ input: string; valid: boolean }>;
  names: Array<{
    date: string; patient: string; title: string; ext: string; seq?: number; fileName: string;
  }>;
};

test('conforms to the shared naming table (same cases as the Rust implementation)', () => {
  for (const c of CASES.slugs) {
    const got = c.kind === 'patient' ? patientSlug(c.input) : titleSlug(c.input);
    assert.equal(got, c.want, `${c.kind}(${JSON.stringify(c.input)})`);
  }

  for (const c of CASES.dates) {
    assert.equal(isValidDocDate(c.input), c.valid, `isValidDocDate(${JSON.stringify(c.input)})`);
  }

  for (const c of CASES.names) {
    const got = buildName(
      { docDate: c.date, patientName: c.patient, title: c.title, ext: c.ext, seq: c.seq ?? 1 },
      ROOT_LEN,
    );
    assert.equal(got.fileName, c.fileName, `buildName(${JSON.stringify(c)})`);
  }
});

test('illegal Windows characters are folded to separators', () => {
  assert.equal(slugify('CBC: Report <final>', 60), 'CBC-Report-final');
  assert.equal(slugify('a/b\\c|d?e*f"g', 60), 'a-b-c-d-e-f-g');
});

test('control characters do not survive into a filename', () => {
  assert.equal(slugify('Thyroid\u0000\u0007Profile\u001f', 60), 'Thyroid-Profile');
});

test('reserved DOS device names are escaped, including as folder names', () => {
  assert.equal(patientSlug('CON'), 'CON_');
  assert.equal(patientSlug('nul'), 'nul_');
  assert.equal(patientSlug('COM1'), 'COM1_');
  assert.equal(patientSlug('LPT9'), 'LPT9_');
  // Not reserved once it is part of a longer name.
  assert.equal(patientSlug('Connor'), 'Connor');
});

test('trailing dots and spaces are stripped (Windows silently rewrites them)', () => {
  assert.equal(patientSlug('Rahim Uddin .'), 'Rahim-Uddin');
  assert.equal(patientSlug('  Rahim  '), 'Rahim');
  assert.equal(titleSlug('Lipid Profile...'), 'Lipid-Profile');
});

test('invisible characters are stripped so two folders cannot look identical', () => {
  const withZwsp = 'Rahim\u200bUddin';
  const withBidi = 'Rahim\u202eUddin';
  assert.equal(patientSlug(withZwsp), patientSlug('RahimUddin'));
  assert.equal(patientSlug(withBidi), patientSlug('RahimUddin'));
});

test('names are NFC-normalized so NFD and NFC forms collapse to one folder', () => {
  const nfc = 'Jos\u00e9';           // é as a single code point
  const nfd = 'Jose\u0301';          // e + combining acute
  assert.equal(patientSlug(nfc), patientSlug(nfd));
});

test('empty and all-illegal input falls back rather than producing an empty segment', () => {
  assert.equal(patientSlug(''), 'Unknown-Patient');
  assert.equal(patientSlug('///'), 'Unknown-Patient');
  assert.equal(titleSlug('***'), 'Untitled');
});

test('truncation never splits a grapheme cluster', () => {
  const flag = '\u{1F1E7}\u{1F1E9}';        // 🇧🇩, 4 UTF-16 units
  assert.equal(truncateGraphemes(flag, 2), '');   // refuses to emit half a flag
  assert.equal(truncateGraphemes(flag, 4), flag);

  const combining = 'a\u0301b\u0301c\u0301';      // á b́ ć as base+combining pairs
  assert.equal(truncateGraphemes(combining, 3), 'a\u0301');
});

test('doc_date validation accepts real dates and both sentinels', () => {
  assert.ok(isValidDocDate('2026-03-14'));
  assert.ok(isValidDocDate('2024-02-29'));       // leap year
  assert.ok(isValidDocDate('2026-03-00'));       // month-only sentinel
  assert.ok(isValidDocDate(UNKNOWN_DATE));

  assert.ok(!isValidDocDate('2023-02-29'));      // not a leap year
  assert.ok(!isValidDocDate('2026-13-01'));
  assert.ok(!isValidDocDate('2026-00-14'));
  assert.ok(!isValidDocDate('14/03/2026'));      // the format that cannot sort
  assert.ok(!isValidDocDate('1899-01-01'));
});

test('sentinel dates still sort correctly as plain text', () => {
  const dates = ['2026-03-14', '0000-00-00', '2026-03-00', '2025-12-01'];
  assert.deepEqual(
    [...dates].sort(),
    ['0000-00-00', '2025-12-01', '2026-03-00', '2026-03-14'],
  );
});

test('undated documents are quarantined, not filed into a wrong year', () => {
  assert.equal(yearFolder('2026-03-14'), '2026');
  assert.equal(yearFolder(UNKNOWN_DATE), 'Undated');
  assert.equal(yearFolder('garbage'), 'Undated');
});

test('canonical name is ISO-first and self-describing', () => {
  const b = buildName(
    { docDate: '2026-03-14', patientName: 'Rahim Uddin', title: 'Thyroid Profile', ext: 'pdf' },
    ROOT_LEN,
  );
  assert.equal(b.fileName, '2026-03-14_Rahim-Uddin_Thyroid-Profile.pdf');
  assert.equal(b.relPath, 'Rahim-Uddin\\2026\\2026-03-14_Rahim-Uddin_Thyroid-Profile.pdf');
  assert.equal(b.titleTruncated, false);
});

test('collision suffixes are zero-padded and only appear past seq 1', () => {
  const base = { docDate: '2026-03-14', patientName: 'Rahim', title: 'CBC', ext: 'jpg' };
  assert.ok(buildName({ ...base, seq: 1 }, ROOT_LEN).fileName.endsWith('_CBC.jpg'));
  assert.ok(buildName({ ...base, seq: 2 }, ROOT_LEN).fileName.endsWith('_CBC__02.jpg'));
  assert.ok(buildName({ ...base, seq: 12 }, ROOT_LEN).fileName.endsWith('_CBC__12.jpg'));
});

test('collision suffixes sort in numeric order within a day', () => {
  const base = { docDate: '2026-03-14', patientName: 'Rahim', title: 'CBC', ext: 'jpg' };
  const names = [3, 1, 12, 2].map((seq) => buildName({ ...base, seq }, ROOT_LEN).fileName);
  const sorted = [...names].sort();
  assert.deepEqual(sorted, [
    '2026-03-14_Rahim_CBC.jpg',
    '2026-03-14_Rahim_CBC__02.jpg',
    '2026-03-14_Rahim_CBC__03.jpg',
    '2026-03-14_Rahim_CBC__12.jpg',
  ]);
});

test('path never exceeds MAX_PATH even with hostile input', () => {
  const b = buildName(
    {
      docDate: '2026-03-14',
      patientName: 'A'.repeat(200),
      title: 'Ultrasonogram of Whole Abdomen with Doppler Study and Contrast '.repeat(5),
      ext: 'pdf',
    },
    ROOT_LEN,
  );
  assert.ok(b.titleTruncated);
  assert.ok(
    ROOT_LEN + b.relPath.length <= MAX_PATH,
    `path was ${ROOT_LEN + b.relPath.length}, limit ${MAX_PATH}`,
  );
});

test('a deeply nested vault root still yields a legal path', () => {
  const deepRoot = 'C:\\Users\\a-user\\OneDrive - Some Long Organisation Name\\Documents\\MedicineReportTracker\\'.length;
  const b = buildName(
    {
      docDate: '2026-03-14',
      patientName: 'Mohammad Rahim Uddin Chowdhury',
      title: 'Ultrasonogram of Whole Abdomen with Doppler',
      ext: 'pdf',
    },
    deepRoot,
  );
  assert.ok(deepRoot + b.relPath.length <= MAX_PATH,
    `path was ${deepRoot + b.relPath.length}, limit ${MAX_PATH}`);
});

test('truncated titles never end on a separator or dot', () => {
  for (let n = 10; n < 90; n++) {
    const b = buildName(
      { docDate: '2026-03-14', patientName: 'R'.repeat(n), title: 'Complete Blood Count With ESR', ext: 'pdf' },
      ROOT_LEN,
    );
    assert.ok(!/[-.]\.(pdf)$/.test(b.fileName), `bad tail at n=${n}: ${b.fileName}`);
  }
});

test('the patient slug is identical in the folder and in the filename', () => {
  const b = buildName(
    { docDate: '2026-03-14', patientName: 'Rahim Uddin', title: 'CBC', ext: 'pdf' },
    ROOT_LEN,
  );
  assert.ok(b.fileName.includes(b.patientFolder));
});

test('case-differing patient names produce slugs that collide on NTFS', () => {
  // NTFS is case-insensitive but case-preserving: these must be caught by the
  // UNIQUE INDEX on upper(folder_slug), not silently become two folders.
  assert.notEqual(patientSlug('Rahim'), patientSlug('rahim'));
  assert.equal(patientSlug('Rahim').toUpperCase(), patientSlug('rahim').toUpperCase());
});

test('extension comes from the caller and is normalized to lowercase', () => {
  const b = buildName(
    { docDate: '2026-03-14', patientName: 'Rahim', title: 'CBC', ext: 'PDF' },
    ROOT_LEN,
  );
  assert.ok(b.fileName.endsWith('.pdf'));
});
