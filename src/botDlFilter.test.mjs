import test from 'node:test';
import assert from 'node:assert/strict';
import {
  dlStatusOf,
  countByDlStatus,
  filterByDlStatus,
  dlItemLabel,
  DL_STATUS_ORDER,
  DL_STATUS_LABEL,
} from './botDlFilter.ts';

const row = (id, dl, kind = 'image', dedup = 0, parent = null, sender = null) => ({
  id, dl, kind, dedup, parent, sender, label: `#${id}`, size: null, err: null,
});

test('下载队列状态压过一切其它标记', () => {
  assert.equal(dlStatusOf(row(1, 'busy')), 'busy');
  assert.equal(dlStatusOf(row(2, 'pending', 'image', 1)), 'pending');
  assert.equal(dlStatusOf(row(3, 'dead')), 'dead');
});

test('转发父行归已归档，内部跳过的重复数不算「重复跳过」', () => {
  assert.equal(dlStatusOf(row(4, 'ok', 'forward', 3)), 'archived');
  assert.equal(dlStatusOf(row(5, 'none', 'image')), 'archived', '转发明细行');
  assert.equal(dlStatusOf(row(6, 'none', 'text')), 'archived', '纯文本直发');
});

test('ok 行按 dedup 分成重复跳过与已下载', () => {
  assert.equal(dlStatusOf(row(7, 'ok')), 'ok');
  assert.equal(dlStatusOf(row(8, 'ok', 'video', 1)), 'dedup');
});

test('计数覆盖六个状态，未出现的状态为 0', () => {
  const counts = countByDlStatus([row(1, 'busy'), row(2, 'busy'), row(3, 'dead'), row(4, 'ok')]);
  assert.deepEqual(counts, { busy: 2, pending: 0, dead: 1, dedup: 0, ok: 1, archived: 0 });
  assert.equal(Object.keys(counts).length, DL_STATUS_ORDER.length);
  for (const s of DL_STATUS_ORDER) assert.ok(DL_STATUS_LABEL[s], `${s} 缺标签`);
});

test('空列表不炸，筛全部返回副本', () => {
  assert.equal(countByDlStatus([]).ok, 0);
  const items = [row(1, 'ok')];
  const all = filterByDlStatus(items, 'all');
  assert.notEqual(all, items, 'all 也走 slice，调用方改动不会污染快照');
  assert.deepEqual(filterByDlStatus(items, 'dedup'), []);
  assert.deepEqual(filterByDlStatus(items, 'ok').map(i => i.id), [1]);
});

test('转发明细行带发送者前缀，顶层行不带', () => {
  assert.equal(dlItemLabel(row(1, 'ok')), '#1');
  assert.equal(dlItemLabel(row(2, 'ok', 'image', 0, 7, '小明')), '小明：#2');
  assert.equal(dlItemLabel(row(3, 'ok', 'image', 0, 7, null)), '#3', '明细行缺昵称时不加前缀');
});