import test from 'node:test';
import assert from 'node:assert/strict';
import { gridWindowOf } from './hooks/useGridWindow.ts';

test('gridWindowOf slices by rows with overscan and pads both ends', () => {
  // 100 条、4 列、行高 170 → 共 25 行
  // 顶部：scrollTop=0 → lastRow = ceil(500/170) = 3，endRow = 3+1+4 = 8
  let w = gridWindowOf(100, 4, 170, 0, 500, 4);
  assert.equal(w.start, 0);
  assert.equal(w.end, 32);
  assert.equal(w.padTop, 0);
  assert.equal(w.padBottom, (25 - 8) * 170);

  // 中部：scrollTop=1700 → firstRow=10 → startRow=6；lastRow=13 → endRow=18
  w = gridWindowOf(100, 4, 170, 1700, 500, 4);
  assert.equal(w.start, 24);
  assert.equal(w.end, 72);
  assert.equal(w.padTop, 1020);
  assert.equal(w.padBottom, 1190);

  // 滚到底：残留的大 scrollTop 被夹到末行，末条目下标被 total 夹住，无尾部空白
  w = gridWindowOf(100, 4, 170, 100000, 500, 4);
  assert.equal(w.start, 80);
  assert.equal(w.end, 100);
  assert.equal(w.padBottom, 0);

  // 未就绪（rowH=0）：退回前 60 条
  w = gridWindowOf(100, 4, 0, 0, 500, 4);
  assert.deepEqual([w.start, w.end], [0, 60]);
});

test('gridWindowOf renders all rows when list is small', () => {
  // 8 条、4 列、2 行：窗口覆盖全部，无空白
  const w = gridWindowOf(8, 4, 170, 0, 500, 4);
  assert.equal(w.start, 0);
  assert.equal(w.end, 8);
  assert.equal(w.padTop, 0);
  assert.equal(w.padBottom, 0);
});

test('gridWindowOf yields a valid window when the list shrinks under a stale scrollTop', () => {
  // 回归（切目录卡死）：残留 scrollTop=100000 + 列表只剩 10 条 →
  // 可见行夹回末行，窗口仍 start<=end 且覆盖全部条目，不产生空窗口
  const w = gridWindowOf(10, 4, 170, 100000, 500, 4);
  assert.equal(w.start, 0);
  assert.equal(w.end, 10);
  assert.equal(w.padTop, 0);
  assert.equal(w.padBottom, 0);
});
