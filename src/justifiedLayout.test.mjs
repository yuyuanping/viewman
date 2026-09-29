import test from 'node:test';
import assert from 'node:assert/strict';
import {
  justifiedRows,
  justifiedWindow,
  cardAspect,
  CARD_TEXT_H,
} from './justifiedLayout.ts';

const W = 1000;
const TARGET = 200;
const GAP = 12;

/** 非末行应恰好铺满容器宽：imageH*Σaspect + gap*(count-1) === W（比例按布局同款夹取） */
function rowWidth(row, aspects) {
  return row.imageH * aspects.slice(row.start, row.start + row.count).reduce((a, b) => a + cardAspect(b), 0)
    + GAP * (row.count - 1);
}

test('full rows fill the container width exactly and stay near the target height', () => {
  // 一堆常规横竖图
  const aspects = [1.5, 1.5, 0.75, 1.33, 1.33, 1.33, 1.0, 1.5, 0.8, 1.6, 1.6, 1.6, 1.6];
  const { rows } = justifiedRows(aspects, W, TARGET, GAP, CARD_TEXT_H);
  assert.ok(rows.length > 1, 'should span multiple rows');
  for (const row of rows.slice(0, -1)) {
    assert.ok(Math.abs(rowWidth(row, aspects) - W) < 1e-6, `row ${row.start} fills width`);
    assert.ok(row.imageH >= TARGET, 'non-last rows are at least the target height');
    assert.ok(row.imageH <= TARGET * 2, 'row height is capped');
  }
});

test('covers every item once, in order, without gaps', () => {
  const aspects = Array.from({ length: 97 }, (_, i) => 0.6 + (i % 7) * 0.31);
  const { rows, totalHeight } = justifiedRows(aspects, W, TARGET, GAP, CARD_TEXT_H);
  let expected = 0;
  for (const row of rows) {
    assert.equal(row.start, expected);
    expected += row.count;
  }
  assert.equal(expected, aspects.length);
  // 总高 = 末行 bottom（不含行距尾巴）
  const last = rows[rows.length - 1];
  assert.equal(totalHeight, last.bottom);
});

test('last row is not stretched', () => {
  const aspects = [1.5, 1.5, 1.5];
  const { rows } = justifiedRows(aspects, W, TARGET, GAP, CARD_TEXT_H);
  assert.equal(rows.length, 1);
  assert.ok(rows[0].imageH <= TARGET, 'lone row keeps the target height');
});

test('extreme aspects are clamped so rows stay within the height cap', () => {
  const cases = [[20], [0.05], [20, 0.05], [20, 20, 0.05]];
  for (const aspects of cases) {
    const { rows, totalHeight } = justifiedRows(aspects, W, TARGET, GAP, CARD_TEXT_H);
    assert.equal(rows.reduce((n, r) => n + r.count, 0), aspects.length);
    for (const row of rows) {
      assert.ok(row.imageH <= TARGET * 2, 'panorama/tall-image rows are height-capped');
      assert.ok(rowWidth(row, aspects) <= W + 1e-6, 'row never overflows the container');
    }
    const last = rows[rows.length - 1];
    assert.equal(totalHeight, last.bottom);
  }
});

test('empty input and unmeasured container yield an empty layout', () => {
  assert.deepEqual(justifiedRows([], W, TARGET, GAP, CARD_TEXT_H), { rows: [], totalHeight: 0 });
  assert.deepEqual(justifiedRows([1, 2], 0, TARGET, GAP, CARD_TEXT_H), { rows: [], totalHeight: 0 });
});

test('justifiedWindow returns rows overlapping the viewport plus overscan', () => {
  const aspects = Array.from({ length: 500 }, (_, i) => 1 + (i % 5) * 0.2);
  const { rows } = justifiedRows(aspects, W, TARGET, GAP, CARD_TEXT_H);
  // 滚到中间某行：窗口应包含该行且留出 overscan 余量
  const mid = Math.floor(rows.length / 2);
  const scrollTop = rows[mid].top + 10;
  const [start, end] = justifiedWindow(rows, scrollTop, 400, 300);
  assert.ok(start <= mid, 'window starts above the viewport');
  assert.ok(end > mid, 'window covers the viewport');
  assert.ok(rows[start].bottom > scrollTop - 300, 'no fully out-of-range rows at the start');
  if (end < rows.length) assert.ok(rows[end].top >= scrollTop + 400 + 300 - 1e-6, 'no fully out-of-range rows at the end');
});

test('justifiedWindow clamps to valid range at any scroll position', () => {
  const aspects = Array.from({ length: 50 }, () => 1.4);
  const { rows } = justifiedRows(aspects, W, TARGET, GAP, CARD_TEXT_H);
  for (const scrollTop of [-500, 0, 1e6]) {
    const [start, end] = justifiedWindow(rows, scrollTop, 400, 300);
    assert.ok(start >= 0 && end <= rows.length && start <= end);
  }
  assert.equal(justifiedWindow([], 100, 400, 300)[0], 0);
});
