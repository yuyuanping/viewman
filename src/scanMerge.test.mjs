import test from 'node:test';
import assert from 'node:assert/strict';
import { mergeById } from './scanMerge.ts';

const item = (id, name = id) => ({ id, name });

test('mergeById overwrites by id in place and appends new items once', () => {
  const current = [item('a'), item('b'), item('c')];
  const merged = mergeById(current, [item('b', 'b2'), item('d')]);
  assert.deepEqual(merged.map(i => i.name), ['a', 'b2', 'c', 'd']);
  assert.equal(merged.filter(i => i.id === 'd').length, 1);
});

test('mergeById keeps the original array when there is nothing to merge', () => {
  const current = [item('a')];
  assert.equal(mergeById(current, []), current);
});

test('mergeById leaves the original list untouched', () => {
  const current = [item('a')];
  mergeById(current, [item('b')]);
  assert.deepEqual(current.map(i => i.id), ['a']);
});
