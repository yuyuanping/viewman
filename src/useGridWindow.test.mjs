import test from 'node:test';
import assert from 'node:assert/strict';
import { columnsOf, gridWindowOf } from './hooks/useGridWindow.ts';

test('columnsOf matches CSS auto-fill math', () => {
  // n 列能放下 ⇔ n*minCol + (n-1)*gap <= W（与 CSS repeat(auto-fill, minmax()) 一致）
  // 宽 1000：5*200+4*20 = 1080 > 1000 → 4 列
  assert.equal(columnsOf(1000, 200, 20), 4);
  assert.equal(columnsOf(899, 200, 20), 4);
  // 边界：1079 塞不下 5 列，1080 恰好塞下
  assert.equal(columnsOf(1079, 200, 20), 4);
  assert.equal(columnsOf(1080, 200, 20), 5);
  // 图片网格：宽 900 → 6*150+5*14 = 970 > 900 → 5 列
  assert.equal(columnsOf(900, 150, 14), 5);
  // 容器宽非法时兜底 1 列
  assert.equal(columnsOf(0, 200, 20), 1);
});

test('gridWindowOf slices by rows with overscan and pads both ends', () => {
  // 100 条、4 列、卡高 150、gap 20 → 行高 170，共 25 行
  // 顶部：scrollTop=0 → lastRow = ceil(500/170) = 3，endRow = 3+1+4 = 8
  let w = gridWindowOf(100, 4, 150, 20, 0, 500, 4);
  assert.equal(w.start, 0);
  assert.equal(w.end, 8 * 4);
  assert.equal(w.padTop, 0);
  assert.equal(w.padBottom, (25 - 8) * 170);

  // 中部：scrollTop=1700 → firstRow=10 → startRow=6；lastRow = ceil(2200/170) = 13 → endRow = 18
  w = gridWindowOf(100, 4, 150, 20, 1700, 500, 4);
  assert.equal(w.start, 6 * 4);
  assert.equal(w.end, 18 * 4);
  assert.equal(w.padTop, 6 * 170);
  assert.equal(w.padBottom, (25 - 18) * 170);

  // 滚到底：末条目下标被 total 夹住，无尾部空白
  w = gridWindowOf(100, 4, 150, 20, 100000, 500, 4);
  assert.equal(w.end, 100);
  assert.equal(w.padBottom, 0);

  // 未就绪（cardH=0）：退回前 60 条
  w = gridWindowOf(100, 4, 0, 20, 0, 500, 4);
  assert.deepEqual([w.start, w.end], [0, 60]);
});

test('gridWindowOf renders all rows when list is small', () => {
  // 8 条、4 列、2 行：窗口覆盖全部，无空白
  const w = gridWindowOf(8, 4, 150, 20, 0, 500, 4);
  assert.equal(w.start, 0);
  assert.equal(w.end, 8);
  assert.equal(w.padTop, 0);
  assert.equal(w.padBottom, 0);
});
