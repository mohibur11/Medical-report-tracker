import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  bestDate,
  formatDmy,
  parseDmyInput,
  rankDateCandidates,
} from '../src/lib/extract/dates.ts';

/**
 * Verbatim Windows.Media.Ocr output from spikes/ocr.ps1 over a synthetic
 * Bangladeshi lab report. Kept exactly as the engine produced it — including the
 * flattened table columns and the "ulU/mL" I/l confusion — so the ranker is tested
 * against real OCR behaviour rather than clean text.
 */
const OCR_LAB_REPORT =
  'POPULAR DIAGNOSTIC CENTRE LTD Department of Biochemistry Patient Name : MD RAHIM UDDIN ' +
  'Age / Sex : 47 Years / Male Date of Birth: 12/03/1978 Patient ID : PDC-2026-88214 ' +
  'Sample Collected : 14/03/2026 09:15 AM Received : 14/03/2026 Reported On : 15/03/2026 04:30 PM ' +
  'Referred By : Dr. AK M Salim THYROID PROFILE Test TSH FT3 FT 4 Result 6.82 2.91 0.88 ' +
  'Unit Reference ulU/mL 0.35 -4.94 pg/mL 2.30 - 4.20 ng/dL 0.70 - 1.48 ' +
  'Comment: TSH elevated. Suggest clinical correlation. Printed on : 15/03/2026';

const TODAY = new Date('2026-08-17T00:00:00Z');

test('picks the collection date, not the DOB, from real OCR output', () => {
  const top = bestDate(OCR_LAB_REPORT, { today: TODAY });
  assert.equal(top?.iso, '2026-03-14');
  assert.equal(top?.anchor, 'sample collected');
});

test('the date of birth is rejected outright, never merely outranked', () => {
  const ranked = rankDateCandidates(OCR_LAB_REPORT, { today: TODAY });
  assert.ok(
    !ranked.some((c) => c.iso === '1978-03-12'),
    'a 1978 DOB must never reach the candidate list — it would sort to the top of the folder forever',
  );
});

test('runner-up candidates are retained for the one-click correction', () => {
  const ranked = rankDateCandidates(OCR_LAB_REPORT, { today: TODAY });
  const isos = ranked.map((c) => c.iso);
  assert.deepEqual(isos, ['2026-03-14', '2026-03-15']);
  // The grid shows these as chips; each keeps its evidence.
  assert.ok(ranked.every((c) => typeof c.reason === 'string' && c.reason.length > 0));
});

test('"Printed on" scores below a real report date even at the same value', () => {
  const ranked = rankDateCandidates(OCR_LAB_REPORT, { today: TODAY });
  const reported = ranked.find((c) => c.iso === '2026-03-15');
  // 15/03 appears twice: "Reported On" and "Printed on". It must keep the better label.
  assert.equal(reported?.anchor, 'reported');
});

test('a known patient DOB is hard-rejected even when it looks plausible', () => {
  const text = 'Visit Date : 05/06/2019 Patient DOB 05/06/2019';
  const withoutDob = rankDateCandidates(text, { today: TODAY });
  assert.equal(withoutDob.length, 1);

  const withDob = rankDateCandidates(text, { today: TODAY, patientDob: '2019-06-05' });
  assert.equal(withDob.length, 0, 'must fall through to asking the user');
});

test('day-first is the convention', () => {
  const r = rankDateCandidates('Date : 03/04/2026', { today: TODAY });
  assert.equal(r[0]?.iso, '2026-04-03'); // 3 April, not 4 March
});

test('an impossible month is corrected by swapping, not discarded', () => {
  // Some labs print MM/DD despite local convention; 25 cannot be a month.
  const r = rankDateCandidates('Collected on 03/25/2026', { today: TODAY });
  assert.equal(r[0]?.iso, '2026-03-25');
});

test('future dates and pre-1990 dates are rejected', () => {
  assert.equal(rankDateCandidates('Date : 01/01/2099', { today: TODAY }).length, 0);
  assert.equal(rankDateCandidates('Date : 01/01/1985', { today: TODAY }).length, 0);
});

test('two-digit years roll back rather than land in the future', () => {
  const r = rankDateCandidates('Sample Collected : 14/03/24', { today: TODAY });
  assert.equal(r[0]?.iso, '2024-03-14');

  const r2 = rankDateCandidates('Sample Collected : 14/03/99', { today: TODAY });
  assert.equal(r2[0]?.iso, '1999-03-14');
});

test('textual month formats are recognised in both orders', () => {
  assert.equal(rankDateCandidates('Collected 14 Mar 2026', { today: TODAY })[0]?.iso, '2026-03-14');
  assert.equal(rankDateCandidates('Collected March 14, 2026', { today: TODAY })[0]?.iso, '2026-03-14');
  assert.equal(rankDateCandidates('Collected 14-March-2026', { today: TODAY })[0]?.iso, '2026-03-14');
});

test('ISO dates in digital PDF text are read without ambiguity', () => {
  const r = rankDateCandidates('Report generated 2026-03-14', { today: TODAY });
  assert.equal(r[0]?.iso, '2026-03-14');
});

test('invalid calendar dates are discarded, not clamped', () => {
  assert.equal(rankDateCandidates('Date : 31/02/2026', { today: TODAY }).length, 0);
  assert.equal(rankDateCandidates('Date : 32/01/2026', { today: TODAY }).length, 0);
});

test('a prescription with no date at all yields nothing to prefill', () => {
  // The handwritten-prescription case: this is the normal path, not an error.
  const text = 'Rx Tab. Napa 500mg 1+0+1 after meal Cap. Omeprazole 20mg BD Syp. Ambrox 2 tsf TDS';
  assert.equal(rankDateCandidates(text, { today: TODAY }).length, 0);
  assert.equal(bestDate(text, { today: TODAY }), null);
});

test('expiry and next-appointment dates do not win', () => {
  const text = 'Sample Collected : 14/03/2026 Next Appointment : 20/03/2026';
  assert.equal(bestDate(text, { today: TODAY })?.iso, '2026-03-14');
});

test('display format is DMY while storage stays ISO', () => {
  assert.equal(formatDmy('2026-03-14'), '14/03/2026');
  assert.equal(formatDmy('2026-03-00'), '03/2026');
  assert.equal(formatDmy('0000-00-00'), 'unknown');
});

test('the date field accepts what people actually type', () => {
  assert.equal(parseDmyInput('14/03/2026', TODAY), '2026-03-14');
  assert.equal(parseDmyInput('14-3-2026', TODAY), '2026-03-14');
  assert.equal(parseDmyInput('14.03.2026', TODAY), '2026-03-14');
  assert.equal(parseDmyInput('14 03 2026', TODAY), '2026-03-14');
  assert.equal(parseDmyInput('14032026', TODAY), '2026-03-14'); // keyboard-first: no separators
  assert.equal(parseDmyInput('140326', TODAY), '2026-03-14');

  assert.equal(parseDmyInput('31/02/2026', TODAY), null);
  assert.equal(parseDmyInput('not a date', TODAY), null);
});
