import test from 'node:test';
import assert from 'node:assert/strict';
import { gridWindowOf, rowHeightOf } from './hooks/useGridWindow.ts';

const card = (offsetTop, offsetHeight = 232) => ({ offsetTop, offsetHeight });
const pad = (offsetTop, offsetHeight) => ({ offsetTop, offsetHeight, dataset: { pad: 'top' } });

test('rowHeightOf anchors on the first real card and reads the next row', () => {
  // 4 列：前三张与第四张同排（offsetTop 相等，差 0 不是行高），第五张才在下一行
  const kids = [card(0), card(0), card(0), card(0), card(250), card(250)];
  assert.equal(rowHeightOf(kids, 18), 250);
});

test('rowHeightOf skips pad spacers on both ends', () => {
  // 回归：pad 撑高块曾被当成第一张卡，量到的"行高"其实是 padTop 高度
  const kids = [pad(0, 5000), card(5000), card(5000), card(5250)];
  assert.equal(rowHeightOf(kids, 18), 250);
  const trailingPad = [...[card(0), card(0), card(250)], pad(250, 9000)];
  assert.equal(rowHeightOf(trailingPad, 18), 250);
});

test('rowHeightOf falls back to card height for a single row', () => {
  assert.equal(rowHeightOf([card(0, 232)], 18), 250);
  assert.equal(rowHeightOf([], 18), 0);
});

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
