import test from 'node:test';
import assert from 'node:assert/strict';
import { rangeBetween, unionRange } from './rangeSelect.ts';

const items = ['a', 'b', 'c', 'd', 'e'].map(id => ({ id }));
const ids = list => list.map(i => i.id);

test('rangeBetween selects the inclusive span forward', () => {
  assert.deepEqual(ids(rangeBetween(items, 1, 3)), ['b', 'c', 'd']);
});

test('rangeBetween works backwards too', () => {
  assert.deepEqual(ids(rangeBetween(items, 3, 1)), ['b', 'c', 'd']);
});

test('rangeBetween on a single item yields that item', () => {
  assert.deepEqual(ids(rangeBetween(items, 2, 2)), ['c']);
});

test('rangeBetween without an anchor falls back to the clicked item', () => {
  assert.deepEqual(ids(rangeBetween(items, -1, 3)), ['d']);
});

test('rangeBetween ignores an out-of-range click', () => {
  assert.deepEqual(rangeBetween(items, 0, 9), []);
});

test('unionRange adds to the existing selection without dropping anything', () => {
  const next = unionRange(['z'], rangeBetween(items, 1, 2));
  assert.deepEqual([...next].sort(), ['b', 'c', 'z']);
});
