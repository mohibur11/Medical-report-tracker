import { test } from 'node:test';
import assert from 'node:assert/strict';

import { fileEach, summarize, type BulkTarget } from '../src/lib/bulk.ts';

const ok = (id: string): BulkTarget => ({
  ready: true,
  skipReason: null,
  file: () => Promise.resolve(id),
});

const notReady = (reason: string): BulkTarget => ({
  ready: false,
  skipReason: reason,
  file: () => Promise.reject(new Error('must never be called')),
});

const table = (rows: Record<string, BulkTarget>) => (id: string) =>
  rows[id] ? { name: `${id}.jpg`, target: rows[id]! } : null;

test('every ready row is filed, in the order given', async () => {
  const out = await fileEach(['a', 'b', 'c'], table({ a: ok('doc-a'), b: ok('doc-b'), c: ok('doc-c') }));
  assert.deepEqual(out.filed, ['doc-a', 'doc-b', 'doc-c']);
  assert.deepEqual(out.skipped, []);
});

test('rows file one at a time, never overlapping', async () => {
  let inFlight = 0;
  let peak = 0;
  const slow = (id: string): BulkTarget => ({
    ready: true,
    skipReason: null,
    file: async () => {
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await Promise.resolve();
      inFlight -= 1;
      return id;
    },
  });

  await fileEach(['a', 'b', 'c'], table({ a: slow('1'), b: slow('2'), c: slow('3') }));
  assert.equal(peak, 1, 'each commit reserves a suffix and moves a file — they must not race');
});

test('a row that is not ready is named, not guessed at', async () => {
  const out = await fileEach(
    ['a', 'b'],
    table({ a: notReady('no patient chosen'), b: ok('doc-b') }),
  );
  assert.deepEqual(out.filed, ['doc-b']);
  assert.deepEqual(out.skipped, [{ name: 'a.jpg', reason: 'no patient chosen' }]);
});

test('one failure does not abandon the rest of the batch', async () => {
  const boom: BulkTarget = {
    ready: true,
    skipReason: null,
    file: () => Promise.reject(new Error('disk full')),
  };
  const out = await fileEach(['a', 'b', 'c'], table({ a: ok('doc-a'), b: boom, c: ok('doc-c') }));
  assert.deepEqual(out.filed, ['doc-a', 'doc-c']);
  assert.equal(out.skipped.length, 1);
  assert.match(out.skipped[0]!.reason, /disk full/);
});

test('a row that refused without throwing still counts as left behind', async () => {
  const refused: BulkTarget = { ready: true, skipReason: null, file: () => Promise.resolve(null) };
  const out = await fileEach(['a'], table({ a: refused }));
  assert.deepEqual(out.filed, []);
  assert.equal(out.skipped[0]!.reason, 'could not be filed');
});

test('a row that vanished mid-batch is passed over silently', async () => {
  const out = await fileEach(['a', 'gone'], table({ a: ok('doc-a') }));
  assert.deepEqual(out.filed, ['doc-a']);
  assert.deepEqual(out.skipped, [], 'it was already filed or removed — not a failure');
});

test('progress counts only rows that are actually filed', async () => {
  const seen: Array<[number, number]> = [];
  await fileEach(
    ['a', 'b'],
    table({ a: notReady('no date'), b: ok('doc-b') }),
    (done, total) => seen.push([done, total]),
  );
  assert.deepEqual(seen, [[2, 2]]);
});

test('the summary names what was left, and counts the rest', () => {
  assert.equal(summarize({ filed: ['x', 'y'], skipped: [] }), 'Filed 2.');

  const many = summarize({
    filed: ['x'],
    skipped: [
      { name: 'a.jpg', reason: 'no date' },
      { name: 'b.jpg', reason: 'no patient chosen' },
      { name: 'c.jpg', reason: 'no title' },
      { name: 'd.jpg', reason: 'no date' },
    ],
  });
  assert.match(many, /^Filed 1, left 4: a\.jpg — no date/);
  assert.match(many, /and 1 more$/);
});
