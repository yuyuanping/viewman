import test from 'node:test';
import assert from 'node:assert/strict';
import { pruneGroupsToLive, restoredResultStale } from './detectionCache.ts';

test('restoredResultStale stays quiet for a result this session computed itself', () => {
  // null = 这一趟现跑的，不该挂"上次存下的"那块牌子
  assert.equal(restoredResultStale(null, 0), false);
  assert.equal(restoredResultStale(null, 183_000), false);
});

test('restoredResultStale only nags once the library has grown', () => {
  assert.equal(restoredResultStale(100, 100), false);
  assert.equal(restoredResultStale(100, 101), true);
  // 库还没加载出来（0 条）不能误报，否则一开面板就看到"过期"
  assert.equal(restoredResultStale(100, 0), false);
});

test('restoredResultStale shuts up when the library only shrank', () => {
  // 少掉的条目是用户自己删的：分组当场被裁干净，再提示"过期"只是噪声
  assert.equal(restoredResultStale(100, 87), false);
});

test('pruneGroupsToLive drops vanished members and single-member groups', () => {
  const alive = new Set(['a', 'b', 'c']);
  const groups = [['a', 'b'], ['a', 'gone'], ['gone', 'also-gone'], ['c', 'a', 'b']];
  assert.deepEqual(pruneGroupsToLive(groups, id => alive.has(id)), [['a', 'b'], ['c', 'a', 'b']]);
});

test('pruneGroupsToLive keeps the surviving groups in their original order', () => {
  const alive = new Set(['x', 'y', 'z']);
  const groups = [['z', 'x'], ['y', 'z'], ['x', 'y'], ['gone', 'z']];
  assert.deepEqual(pruneGroupsToLive(groups, id => alive.has(id)), [['z', 'x'], ['y', 'z'], ['x', 'y']]);
});

test('pruneGroupsToLive never mutates the cached groups', () => {
  const groups = [['a', 'gone'], ['b', 'c']];
  pruneGroupsToLive(groups, id => id !== 'gone');
  assert.deepEqual(groups, [['a', 'gone'], ['b', 'c']]);
});
