import test from 'node:test';
import assert from 'node:assert/strict';
import {
  extrasOfGroups, hashDistance, liveGroups, mergeSimilarGroups, pickKeep, selectGroupExtras,
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
    { at: 0, ids: ['a', 'b'], keep: 'a' },
    { at: 2, ids: ['d', 'e'], keep: 'd' },
  ]);
});

test('liveGroups puts the biggest group first', () => {
  const groups = [['a', 'b'], ['c', 'd', 'e']];
  assert.deepEqual(liveGroups(groups, imageById).map(g => g.at), [1, 0]);
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

test('mergeSimilarGroups grows an existing group without duplicating members', () => {
  const merged = mergeSimilarGroups([['a', 'b']], [['b', 'a', 'c']]);
  assert.deepEqual(merged, [['a', 'b', 'c']]);
});

test('mergeSimilarGroups joins the groups a bridge links into one', () => {
  // 后端先推了两条独立组，之后一张桥接图把它们连成一组，整组成员再推一次
  const merged = mergeSimilarGroups([['a', 'b'], ['c', 'd']], [['b', 'c', 'e']]);
  // 新组员接在身后，原有那条的下标不变：默认保留项仍是组内第一张
  assert.deepEqual(merged, [['a', 'b', 'c', 'e', 'd']]);
});

test('mergeSimilarGroups appends genuinely new groups and keeps order', () => {
  const merged = mergeSimilarGroups([['a', 'b']], [['c', 'd'], ['a', 'b']]);
  assert.deepEqual(merged, [['a', 'b'], ['c', 'd']]);
});

test('mergeSimilarGroups does not touch the arrays it was given', () => {
  const existing = [['a', 'b']];
  mergeSimilarGroups(existing, [['b', 'c']]);
  assert.deepEqual(existing, [['a', 'b']]);
});

test('extrasOfGroups excludes each group keeper', () => {
  const groups = liveGroups([['a', 'b', 'c'], ['d', 'e']], imageById, new Set(['b']));
  assert.deepEqual(extrasOfGroups(groups), ['a', 'c', 'e']);
});

test('selectGroupExtras flips one group and leaves the others alone', () => {
  const groups = liveGroups([['a', 'b', 'c'], ['d', 'e']], imageById);
  const target = groups.find(group => group.at === 0);
  assert.deepEqual([...selectGroupExtras(['c', 'd'], target, 'a')].sort(), ['b', 'c', 'd']);
});
