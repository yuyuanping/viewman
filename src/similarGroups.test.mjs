import test from 'node:test';
import assert from 'node:assert/strict';
import { liveGroups, extrasOfGroups, selectGroupExtras } from './similarGroups.ts';

const alive = new Set(['a', 'b', 'c', 'd', 'e']);

test('liveGroups prunes members that left the library and drops groups below two', () => {
  const groups = [['a', 'b', 'gone'], ['c', 'missing'], ['d', 'e']];
  assert.deepEqual(liveGroups(groups, alive, {}), [
    { at: 0, ids: ['a', 'b'], keep: 'a' },
    { at: 2, ids: ['d', 'e'], keep: 'd' },
  ]);
});

test('liveGroups puts the biggest group first', () => {
  const groups = [['a', 'b'], ['c', 'd', 'e']];
  assert.deepEqual(liveGroups(groups, alive, {}).map(g => g.at), [1, 0]);
});

test('liveGroups keeps the chosen keeper and falls back when it is gone', () => {
  assert.deepEqual(liveGroups([['a', 'b', 'c']], alive, { 0: 'c' })[0].keep, 'c');
  assert.deepEqual(liveGroups([['a', 'b', 'c']], alive, { 0: 'gone' })[0].keep, 'a');
});

test('extrasOfGroups excludes each group keeper', () => {
  const groups = liveGroups([['a', 'b', 'c'], ['d', 'e']], alive, { 0: 'b' });
  assert.deepEqual(extrasOfGroups(groups), ['a', 'c', 'e']);
});

test('selectGroupExtras flips one group and leaves the others alone', () => {
  const groups = liveGroups([['a', 'b', 'c'], ['d', 'e']], alive, {});
  const target = groups.find(group => group.at === 0);
  assert.deepEqual([...selectGroupExtras(['c', 'd'], target, 'a')].sort(), ['b', 'c', 'd']);
});
