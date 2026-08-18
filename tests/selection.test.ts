import { test } from 'node:test';
import assert from 'node:assert/strict';

import { EMPTY, allSelected, click, ordered, prune, toggleAll } from '../src/lib/selection.ts';

const ROWS = ['a', 'b', 'c', 'd', 'e'];

const sel = (...ids: string[]) => {
  let s = EMPTY;
  for (const id of ids) s = click(s, ROWS, id, false);
  return s;
};

test('a plain click toggles one row and moves the anchor', () => {
  const one = click(EMPTY, ROWS, 'b', false);
  assert.deepEqual([...one.ids], ['b']);
  assert.equal(one.anchor, 'b');

  const none = click(one, ROWS, 'b', false);
  assert.equal(none.ids.size, 0);
});

test('shift extends from the anchor, inclusive, in either direction', () => {
  const down = click(sel('b'), ROWS, 'd', true);
  assert.deepEqual(ordered(down, ROWS), ['b', 'c', 'd']);

  const up = click(sel('d'), ROWS, 'b', true);
  assert.deepEqual(ordered(up, ROWS), ['b', 'c', 'd']);
});

test('shift only ever adds — dragging a range back does not deselect', () => {
  const wide = click(sel('a'), ROWS, 'e', true);
  const back = click(wide, ROWS, 'b', true);
  // The anchor never moved, so 'a'..'e' stays selected.
  assert.deepEqual(ordered(back, ROWS), ROWS);
});

test('shift with no anchor behaves like a plain click', () => {
  const s = click(EMPTY, ROWS, 'c', true);
  assert.deepEqual([...s.ids], ['c']);
});

test('select all, then all again, clears', () => {
  const all = toggleAll(EMPTY, ROWS);
  assert.equal(allSelected(all, ROWS), true);
  assert.equal(toggleAll(all, ROWS).ids.size, 0);
});

test('nothing on screen is not "all selected"', () => {
  assert.equal(allSelected(EMPTY, []), false);
});

test('filed rows drop out of the selection', () => {
  // 'b' was just committed and is gone from the queue.
  const after = prune(sel('a', 'b', 'c'), ['a', 'c']);
  assert.deepEqual(ordered(after, ['a', 'c']), ['a', 'c']);
  assert.equal(after.ids.has('b'), false);
});

test('an anchor that was filed is forgotten, not left dangling', () => {
  const after = prune(sel('a', 'b'), ['a']);
  assert.equal(after.anchor, null);
});

test('pruning nothing keeps the same object', () => {
  const before = sel('a', 'b');
  assert.equal(prune(before, ROWS), before);
});

test('selected ids come back in screen order, not click order', () => {
  assert.deepEqual(ordered(sel('d', 'a', 'c'), ROWS), ['a', 'c', 'd']);
});
