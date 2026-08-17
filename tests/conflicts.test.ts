import { test } from 'node:test';
import assert from 'node:assert/strict';

import { detectConflicts, isBlocked } from '../src/lib/extract/conflicts.ts';
import { inferDateOrder, isAmbiguousOrder, rankDateCandidates, swapOrder } from '../src/lib/extract/dates.ts';

const TODAY = new Date('2026-08-17T00:00:00Z');

const find = (cs: ReturnType<typeof detectConflicts>, kind: string) => cs.find((c) => c.kind === kind);

test('a numeric date that reads both ways is recognised as ambiguous', () => {
  assert.ok(isAmbiguousOrder('04-07-2026'), '4 July or 7 April');
  assert.ok(isAmbiguousOrder('03/04/2026'));

  assert.ok(!isAmbiguousOrder('28/11/2024'), '28 cannot be a month');
  assert.ok(!isAmbiguousOrder('04-Jul-26'), 'a named month settles it');
  assert.ok(!isAmbiguousOrder('2026-03-14'), 'ISO is unambiguous');
  assert.ok(!isAmbiguousOrder('05/05/2026'), 'both readings are the same day');
});

test('the page settles its own date order when it can', () => {
  // 28 cannot be a month, so this lab writes day first — and that answers the
  // question for every other date on the page without asking anyone.
  assert.equal(inferDateOrder('Collected 28/11/2024 Reported 04/12/2024'), 'dmy');
  assert.equal(inferDateOrder('Collected 11/28/2024 Reported 12/04/2024'), 'mdy');
  assert.equal(inferDateOrder('Exam Date 04-07-2026'), null, 'nothing to go on');
  assert.equal(inferDateOrder('a 28/11/2024 b 11/28/2024'), null, 'contradictory, so do not guess');
});

test('an ambiguous date is asked about only when the page cannot settle it', () => {
  const asked = detectConflicts({
    chosen: '2026-07-04',
    chosenRaw: '04-07-2026',
    text: 'Exam Date 04-07-2026',
    today: TODAY,
  });
  const c = find(asked, 'ambiguous-date-order');
  assert.ok(c, 'must ask');
  assert.equal(c!.severity, 'ask');
  assert.deepEqual(c!.options?.map((o) => o.value), ['2026-07-04', '2026-04-07']);

  const settled = detectConflicts({
    chosen: '2026-07-04',
    chosenRaw: '04-07-2026',
    text: 'Collected 28/11/2025 Exam Date 04-07-2026',
    today: TODAY,
  });
  assert.equal(find(settled, 'ambiguous-date-order'), undefined, 'the page already answered it');
});

test('swapping reads the other order', () => {
  assert.equal(swapOrder('04-07-2026', TODAY), '2026-04-07');
  assert.equal(swapOrder('28-11-2024', TODAY), null, 'month 28 does not exist');
  assert.equal(swapOrder('04-Jul-26', TODAY), null, 'only numeric dates swap');
});

test('a report dated before the patient was born is refused, not filed', () => {
  const c = detectConflicts({
    chosen: '1985-03-02',
    patientDob: '1992-03-09',
    patientName: 'Rahim Uddin',
    today: TODAY,
  });
  const conflict = find(c, 'date-before-birth');
  assert.ok(conflict);
  assert.equal(conflict!.severity, 'blocking');
  assert.ok(isBlocked(c));
  assert.ok(conflict!.message.includes('Rahim Uddin'));
  assert.ok(/correct the date|fix the date of birth/i.test(conflict!.message), 'must say what to do');
});

test('a future date is refused', () => {
  const c = detectConflicts({ chosen: '2027-01-01', today: TODAY });
  assert.equal(find(c, 'future-date')?.severity, 'blocking');
  assert.ok(isBlocked(c));
});

test('a date that is not a date is refused and stops further judgement', () => {
  const c = detectConflicts({ chosen: '2026-02-31', patientDob: '1992-03-09', today: TODAY });
  assert.equal(c.length, 1);
  assert.equal(c[0]?.kind, 'invalid-date');
  assert.ok(isBlocked(c));
});

test('a date of birth on the page that disagrees with the patient is raised', () => {
  // Two of the real documents carry different dates of birth for the same person.
  // Left alone this quietly disables the strongest guard against filing a report
  // under a birth date.
  const candidates = rankDateCandidates('Date of Birth 09-03-1992 Collected 28/11/2024', {
    today: TODAY,
  });
  const c = detectConflicts({
    chosen: '2024-11-28',
    candidates,
    patientDob: '1990-06-21',
    today: TODAY,
  });

  const conflict = find(c, 'dob-mismatch');
  assert.ok(conflict, 'must be raised');
  assert.equal(conflict!.severity, 'ask', 'the report itself may still be fine');
  assert.ok(conflict!.message.includes('09/03/1992'));
  assert.ok(conflict!.message.includes('21/06/1990'));
  assert.ok(!isBlocked(c), 'filing is allowed; the user is simply told');
});

test('a matching date of birth raises nothing', () => {
  const candidates = rankDateCandidates('Date of Birth 09-03-1992 Collected 28/11/2024', {
    today: TODAY,
  });
  const c = detectConflicts({
    chosen: '2024-11-28',
    candidates,
    patientDob: '1992-03-09',
    today: TODAY,
  });
  assert.equal(find(c, 'dob-mismatch'), undefined);
});

test('two nearly-tied candidates are flagged rather than silently resolved', () => {
  // "reported" and "requested date" are deliberately close in weight: both are
  // plausible readings of what a lab report is dated, and neither deserves to win
  // silently.
  const candidates = rankDateCandidates('Reported 01/12/2024 Requested Date 28/11/2024', {
    today: TODAY,
  });
  const c = detectConflicts({
    chosen: candidates[0]!.iso,
    candidates,
    today: TODAY,
  });
  const close = find(c, 'close-call');
  assert.ok(close, `expected a close call, scores were ${candidates.map((x) => x.score).join(', ')}`);
  assert.equal(close!.options?.length, 2);
});

test('a clear winner is not flagged as a close call', () => {
  const candidates = rankDateCandidates('Sample Collected 28/11/2024 printed on 01/12/2024', {
    today: TODAY,
  });
  const c = detectConflicts({ chosen: candidates[0]!.iso, candidates, today: TODAY });
  assert.equal(find(c, 'close-call'), undefined);
});

test('an empty date raises nothing — the field simply has to be filled', () => {
  assert.deepEqual(detectConflicts({ chosen: null, today: TODAY }), []);
});

test('a normal document raises nothing at all', () => {
  const candidates = rankDateCandidates('Sample Collected : 14/03/2026', { today: TODAY });
  const c = detectConflicts({
    chosen: '2026-03-14',
    chosenRaw: '14/03/2026',
    text: 'Sample Collected : 14/03/2026',
    candidates,
    patientDob: '1992-03-09',
    today: TODAY,
  });
  assert.deepEqual(c, [], `unexpected: ${JSON.stringify(c)}`);
});
