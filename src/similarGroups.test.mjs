import test from 'node:test';
import assert from 'node:assert/strict';
import {
  extrasOfGroups, groupsMatching, liveGroups, selectGroupExtras,
} from './similarGroups.ts';

/** 组内顺序即后端顺序（按入库时间升序），所以首张就是"最早入库" */
const imageById = new Map([
  ['a', { id: 'a', file_size: 500, width: 800, height: 600 }],
  ['b', { id: 'b', file_size: 900, width: 1000, height: 800 }],
  ['c', { id: 'c', file_size: 100, width: 2000, height: 1500 }],
  ['d', { id: 'd', file_size: 700, width: null, height: null }],
  ['e', { id: 'e', file_size: 700, width: 640, height: 360 }],
]);

test('liveGroups prunes members that left the library and drops groups below two', () => {
  const groups = [['a', 'b', 'gone'], ['c', 'missing'], ['d', 'e']];
  assert.deepEqual(liveGroups(groups, imageById), [
    { ids: ['a', 'b'], keep: 'a' },
    { ids: ['d', 'e'], keep: 'd' },
  ]);
});

test('liveGroups puts the biggest group first', () => {
  const groups = [['a', 'b'], ['c', 'd', 'e']];
  assert.deepEqual(liveGroups(groups, imageById).map(g => g.keep), ['c', 'a']);
});

test('liveGroups honors the chosen keeper and falls back when it is gone', () => {
  assert.equal(liveGroups([['a', 'b', 'c']], imageById, new Set(['c']))[0].keep, 'c');
  assert.equal(liveGroups([['a', 'b', 'c']], imageById, new Set(['gone']))[0].keep, 'a');
});

test('extrasOfGroups excludes each group keeper', () => {
  const groups = liveGroups([['a', 'b', 'c'], ['d', 'e']], imageById, new Set(['b']));
  assert.deepEqual(extrasOfGroups(groups), ['a', 'c', 'e']);
});

test('selectGroupExtras flips one group and leaves the others alone', () => {
  const groups = liveGroups([['a', 'b', 'c'], ['d', 'e']], imageById);
  const target = groups.find(group => group.keep === 'a');
  assert.deepEqual([...selectGroupExtras(['c', 'd'], target, 'a')].sort(), ['b', 'c', 'd']);
});

test('groupsMatching finds a group by filename or path', () => {
  const byId = new Map([
    ['a', { id: 'a', filename: '175ffbbe.png', path: 'D:\\QQBot_Data\\group\\images\\175ffbbe.png', file_size: 1, width: 1, height: 1 }],
    ['b', { id: 'b', filename: 'pet.png', path: 'E:\\精选\\pet.png', file_size: 1, width: 1, height: 1 }],
    ['c', { id: 'c', filename: 'PET-COPY.JPG', path: 'E:\\精选\\PET-COPY.JPG', file_size: 1, width: 1, height: 1 }],
  ]);
  const groups = liveGroups([['b', 'c'], ['a', 'b']], byId);
  // 命中筛选不能改动组的保留张：面板拿它当 React key 和「留」的入参，串了就翻不到那组
  assert.deepEqual(groupsMatching(groups, '175FFBBE', byId).map(group => group.keep), ['a']);
  assert.deepEqual(groupsMatching(groups, '精选', byId).map(group => group.keep), ['b', 'a']);
  assert.deepEqual(groupsMatching(groups, '  ', byId), groups);
  assert.deepEqual(groupsMatching(groups, 'nomatch', byId), []);
});
