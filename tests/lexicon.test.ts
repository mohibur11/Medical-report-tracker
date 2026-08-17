import { test } from 'node:test';
import assert from 'node:assert/strict';

import { describe, detectDocType, suggestTitles } from '../src/lib/extract/lexicon.ts';

const titles = (text: string) => suggestTitles(text).map((s) => s.title);

test('a radiology report is titled from its study', () => {
  const t = suggestTitles('DEPARTMENT OF RADIOLOGY & IMAGING USG of Thyroid glands (B & W-CD Machine) Right lobe');
  assert.equal(t[0]?.title, 'USG of Thyroid');
  assert.equal(t[0]?.category, 'Thyroid', 'one lookup should settle the tag too');
});

test('a longer phrase beats a code that also appears in the reference column', () => {
  // "tsh" turns up in the reference ranges of a thyroid panel; the panel's own
  // name is the better answer.
  const t = titles('THYROID PROFILE Test TSH 6.82 uIU/mL 0.35 - 4.94 FT4 0.88');
  assert.equal(t[0], 'Thyroid Profile');
});

test('short codes only match as whole words', () => {
  // Otherwise "alt" matches "alternate" and "ct" matches half the page.
  assert.ok(!titles('please consult an alternate provider').includes('SGPT (ALT)'));
  assert.ok(!titles('contact the department').includes('CT Scan'));
  assert.ok(titles('SGPT (ALT) 34 U/L').includes('SGPT (ALT)'));
});

test('common South Asian test names are recognised', () => {
  assert.equal(titles('COMPLETE BLOOD COUNT (CBC)')[0], 'Complete Blood Count');
  assert.equal(titles('S. Creatinine 1.1 mg/dL')[0], 'Serum Creatinine');
  assert.equal(titles('Urine R/E')[0], 'Urine R/E');
  assert.ok(titles('HbA1c 6.2 %').includes('HbA1c'));
  assert.ok(titles('Widal Test for enteric fever').includes('Widal Test'));
});

test('an unrecognised document suggests nothing rather than guessing', () => {
  assert.deepEqual(suggestTitles('a note about nothing in particular'), []);
});

test('a pathology report is recognised as a report', () => {
  const text =
    'PATHOLOGICAL REPORT HI-TECH LAB LAB NO: C-00-000000 Clinical History: thyroid nodule ' +
    'Gross Examination: Received are six smear slides Microscopic Examination: the smears show';
  assert.equal(detectDocType(text), 'report');
  assert.equal(titles(text)[0], 'Pathological Report');
});

test('a hospital receipt is recognised as an invoice, not a report', () => {
  const text =
    'Receipt OPD Original Date/Time 30 Nov 2024 Document No. Payment Type Credit Card ' +
    'Description Amount Discount Net Medication for Outpatient 825.00 PAID Baht Only';
  assert.equal(detectDocType(text), 'invoice');
  assert.equal(titles(text)[0], 'Receipt');
});

test('a receipt that mentions dispensed medicines is still an invoice', () => {
  // Real receipts carry "Rx DISPENSED" and list drugs, which would otherwise read
  // as a prescription.
  const text =
    'Receipt OPD Payment Type Credit Card Rx DISPENSED Medication for Outpatient ' +
    'Tab. Napa 500mg Amount Discount Net Total PAID Baht';
  assert.equal(detectDocType(text), 'invoice');
});

test('a prescription is detected by its dosing notation, not its words', () => {
  // A doctor's slip can be almost entirely unreadable, but the dosing survives.
  assert.equal(detectDocType('Rx Tab. Napa 500mg 1+0+1 after meal Cap. Omeprazole 20mg BD'), 'prescription');
  assert.equal(detectDocType('Syp. Ambrox 2 tsf TDS x 7 days'), 'prescription');
});

test('a prescription is not given a test name', () => {
  // It has drugs and a doctor, not a test, so matching a title would be noise.
  const text = 'Rx Tab. Napa 500mg 1+0+1 Cap. Omeprazole 20mg BD';
  assert.equal(detectDocType(text), 'prescription');
});

test('a visit slip is recognised', () => {
  const text = 'Visit Slip Queue Number 5474 Name Date Nationality Appointment Department';
  assert.equal(titles(text)[0], 'Visit Slip');
});

test('an allergy panel is recognised', () => {
  const text = 'Allergy Profile for Specific IgE testing Combined 36 allergens Antigen Concentration Class';
  assert.ok(titles(text).includes('Allergy Panel'));
});

test('a document with nothing to go on is other, not report', () => {
  assert.equal(detectDocType('hello'), 'other');
});

/**
 * The following came from running this against real scanned records. Each was a
 * wrong answer the lexicon gave before the kind was allowed to constrain the title.
 */

test('a receipt is not titled "Prescription" just because it prints a prescription number', () => {
  const text =
    'Receipt OPD Original Payment Type Credit Card Prescription No. O0000000000 ' +
    'Description Amount Discount Net Medication for Outpatient 825.00 PAID Baht Only';
  const d = describe(text);
  assert.equal(d.docType, 'invoice');
  assert.equal(d.titles[0]?.title, 'Receipt');
});

test('an itemised bill is not titled after a test it happens to list', () => {
  // A cashier sheet names every lab test on the bill; none of them is what the
  // document is.
  const text =
    'Details Cashier OPD Payment Type Amount Discount Net Amount Laboratory Investigation ' +
    'Thyroid Stimulating Hormone ( TSH ) 1,100.00 Thyroxine Free ( FT4 ) 1,005.00 Total PAID';
  const d = describe(text);
  assert.equal(d.docType, 'invoice');
  assert.equal(d.titles[0]?.title, 'Receipt');
});

test('a radiology report is a report, though it uses none of the usual lab words', () => {
  const text =
    'DEPARTMENT OF RADIOLOGY & IMAGING USG of Thyroid glands Right lobe of thyroid gland is ' +
    'normal in size. Echotexture is homogeneously uniform. Two hypoechoic nodules are seen. ' +
    'Impression: Right thyroid nodules.';
  const d = describe(text);
  assert.equal(d.docType, 'report');
  assert.equal(d.titles[0]?.title, 'USG of Thyroid');
  assert.equal(d.titles[0]?.category, 'Thyroid');
});

test('an immunology panel is a report', () => {
  const text =
    'Allergy Profile for Specific IgE testing Antigen Concentration Class ' +
    'Bermuda grass < 0.35 kU/l 0 Antibody detection titer';
  assert.equal(describe(text).docType, 'report');
});

test('a prescription still yields no title through describe()', () => {
  const d = describe('Rx Tab. Napa 500mg 1+0+1 Cap. Omeprazole 20mg BD');
  assert.equal(d.docType, 'prescription');
  assert.deepEqual(d.titles, []);
});
