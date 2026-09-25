import test from 'node:test';
import assert from 'node:assert/strict';
import {
  extrasOfGroups, groupsMatching, hashDistance, liveGroups, pickKeep, selectGroupExtras,
} from './similarGroups.ts';

test('hashDistance counts differing bits across both halves of the 64-bit hash', () => {
  assert.equal(hashDistance([0, 0], [0, 0]), 0);
  assert.equal(hashDistance([0b0101, 0], [0, 0]), 2);
  assert.equal(hashDistance([0xffff_ffff, 0], [0, 0]), 32);
  assert.equal(hashDistance([0xffff_ffff, 0xffff_ffff], [0, 0]), 64);
  // 补算失败的张没有指纹：给 null，让调用方把这块标记省掉而不是显示成"距 0"
  assert.equal(hashDistance(undefined, [0, 0]), null);
  assert.equal(hashDistance([0, 0], undefined), null);
});

/** 组内顺序即后端顺序（按入库时间升序），所以首张就是"最早入库" */
const imageById = new Map([
  ['a', { id: 'a', file_size: 500, width: 800, height: 600 }],
  ['b', { id: 'b', file_size: 900, width: 1000, height: 800 }],
  ['c', { id: 'c', file_size: 100, width: 2000, height: 1500 }],
  ['d', { id: 'd', file_size: 700, width: null, height: null }],
  ['e', { id: 'e', file_size: 700, width: 640, height: 360 }],
]);
const ids = ['a', 'b', 'c', 'd', 'e'];

test('pickKeep keeps the first added image under the earliest rule', () => {
  assert.equal(pickKeep(['a', 'b', 'c'], imageById, 'earliest'), 'a');
});

test('pickKeep chooses the largest pixels and the largest file', () => {
  assert.equal(pickKeep(ids, imageById, 'highest'), 'c');
  assert.equal(pickKeep(ids, imageById, 'largest'), 'b');
});

test('pickKeep ignores members with missing metadata instead of picking them', () => {
  // d 没有宽高：按分辨率选主时不能因为它排得靠前就当主
  assert.equal(pickKeep(['d', 'e'], imageById, 'highest'), 'e');
  assert.equal(pickKeep(['d', 'e'], imageById, 'largest'), 'd');
  assert.equal(pickKeep(['d', 'e'], new Map(), 'largest'), 'd');
});

test('liveGroups prunes members that left the library and drops groups below two', () => {
  const groups = [['a', 'b', 'gone'], ['c', 'missing'], ['d', 'e']];
  assert.deepEqual(liveGroups(groups, imageById), [
    { ids: ['a', 'b'], keep: 'a', far: [] },
    { ids: ['d', 'e'], keep: 'd', far: [] },
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

test('liveGroups applies the keep rule to every group at once', () => {
  const groups = [['a', 'b'], ['c', 'e']];
  assert.deepEqual(
    liveGroups(groups, imageById, undefined, 'highest').map(g => g.keep),
    ['b', 'c'],
  );
  assert.deepEqual(
    liveGroups(groups, imageById, undefined, 'largest').map(g => g.keep),
    ['b', 'e'],
  );
});

test('liveGroups lets a manual keeper outrank the rule', () => {
  // 规则会选 c，但人已经点过「留」b
  const groups = [['a', 'b', 'c']];
  assert.equal(liveGroups(groups, imageById, new Set(['b']), 'highest')[0].keep, 'b');
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

test('远亲只列出来看，不参与自动勾选', () => {
  const groups = liveGroups([['a', 'b', 'c']], imageById, undefined, 'earliest', new Set(['c']));
  assert.deepEqual(groups[0].far, ['c']);
  assert.deepEqual(extrasOfGroups(groups), ['b']);
  // 换选主也不该把远亲勾上：它本来就比阈值松一档
  assert.deepEqual([...selectGroupExtras([], groups[0], 'a')].sort(), ['b']);
});

test('默认保留张不挑远亲，整组都是远亲时才退回全表', () => {
  // c 的文件最大，但它是远亲：该留 a 或 b，而不是留一张"看着不太像"的
  const withSkeleton = liveGroups([['a', 'b', 'c']], imageById, undefined, 'largest', new Set(['c']));
  assert.equal(withSkeleton[0].keep, 'b');
  // 整组都是挂来的远亲：主照旧得有一个（否则这张组会被自动勾空），但一张都不勾
  const allFar = liveGroups([['c', 'd']], imageById, undefined, 'largest', new Set(['c', 'd']));
  assert.equal(allFar[0].keep, 'd');
  assert.deepEqual(extrasOfGroups(allFar), []);
});
