/**
 * Date-ranking accuracy harness.
 *
 * The plan gates Phase 3 on a measurement, and it is deliberately not "was a date
 * found". A missing date announces itself — the field is empty and the user types
 * one. A WRONG date does not: it silently produces a mis-ordered history and, if
 * it is a date of birth, files a 2026 report under 1995 where it sorts to the top
 * of the folder forever.
 *
 * So this measures the top pick against ground truth read off the documents by
 * hand, and reports separately how often it chose a date of birth.
 *
 * Usage:
 *   powershell -File spikes/ocr.ps1 -Path <folder of page images>   # produces the OCR json
 *   node spikes/accuracy.mjs <ocr-result.json> <truth.json>
 *
 * truth.json — keep this OUTSIDE the repo, because it describes real records:
 *   {
 *     "today": "YYYY-MM-DD",
 *     "patientDobs": ["YYYY-MM-DD"],
 *     "expected": { "<image file name>": "YYYY-MM-DD" }
 *   }
 */

import { readFileSync } from 'node:fs';

import { rankDateCandidates } from '../src/lib/extract/dates.ts';

const [, , resultsPath, truthPath] = process.argv;
if (!resultsPath || !truthPath) {
  console.error('usage: node spikes/accuracy.mjs <ocr-result.json> <truth.json>');
  process.exit(2);
}

/** Tolerate a byte-order mark: several Windows tools write one, and JSON.parse
 *  rejects it outright. */
const readJson = (p) => JSON.parse(readFileSync(p, 'utf8').replace(/^﻿/, ''));

const rows = readJson(resultsPath);
const truth = readJson(truthPath);
const today = new Date(`${truth.today ?? '2026-01-01'}T00:00:00Z`);
const dobs = truth.patientDobs ?? [];

let correct = 0;
let wrong = 0;
let none = 0;
let pickedDob = 0;
const failures = [];

console.log('');
console.log(`${'document'.padEnd(44)}${'expected'.padEnd(12)}${'picked'.padEnd(20)}anchor`);
console.log('-'.repeat(96));

for (const row of rows) {
  const want = truth.expected?.[row.File];
  if (!want) continue;

  // Give the ranker the same advantage it has in the app, where a patient profile
  // supplies the date of birth to hard-reject.
  const ranked = rankDateCandidates(row.Text ?? '', { today, patientDob: dobs[0] ?? null });
  const top = ranked[0];
  const got = top?.iso ?? '(none)';
  const ok = got === want;

  if (!top) none++;
  else if (ok) correct++;
  else {
    wrong++;
    failures.push({ file: row.File, want, got, anchor: top.anchor, raw: top.raw });
  }
  if (top && dobs.includes(top.iso)) pickedDob++;

  console.log(
    row.File.slice(0, 42).padEnd(44) +
      want.padEnd(12) +
      (ok ? got : `${got} WRONG`).padEnd(20) +
      (top?.anchor ?? '-'),
  );
}

const total = correct + wrong + none;
const pct = (n) => (total === 0 ? '0.0' : ((n / total) * 100).toFixed(1));

console.log('');
console.log(`documents        : ${total}`);
console.log(`CORRECT date     : ${correct}  (${pct(correct)}%)`);
console.log(`WRONG date       : ${wrong}  (${pct(wrong)}%)   <- the silent failure`);
console.log(`no date at all   : ${none}  (${pct(none)}%)   <- harmless, the user types it`);
console.log(`picked a DOB     : ${pickedDob}`);

if (failures.length > 0) {
  console.log('\n--- wrong picks ---');
  for (const f of failures) {
    console.log(`  ${f.file}`);
    console.log(`    wanted ${f.want}, picked ${f.got} (read as "${f.raw}", anchor: ${f.anchor ?? 'none'})`);
  }
}

console.log('');
if (Number(pct(correct)) >= 70) {
  console.log(`PASS - ${pct(correct)}% correct. OCR prefill earns its place.`);
} else {
  console.log(`BELOW GATE - ${pct(correct)}% correct. Prefill may cost more than it saves.`);
}
