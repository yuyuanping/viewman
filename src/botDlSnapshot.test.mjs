import test from 'node:test';
import assert from 'node:assert/strict';
import { dlSnapshotChanged } from './botDlSnapshot.ts';

const status = (over = {}) => ({
  dl: { pending: 1, busy: 1, dead: 2, dead_today: 0, ...(over.dl || {}) },
  summary: { ok: 10, ok_bytes: 1000, today: 3, today_bytes: 300, dedup_today: 1, ...(over.summary || {}) },
  today_items: over.items ?? [{ id: 1 }, { id: 2 }],
});

const snap = (over = {}) => {
  const s = status(over);
  return { dl: s.dl, summary: s.summary, today_items: s.today_items };
};

test('全部字段相同 → 不换引用（避免 2 秒一轮整棵 App 重渲染）', () => {
  assert.equal(dlSnapshotChanged(status(), snap()), false);
});

test('任何一个积压计数变了 → 要更新', () => {
  assert.equal(dlSnapshotChanged(status(), snap({ dl: { busy: 2 } })), true);
  assert.equal(dlSnapshotChanged(status(), snap({ dl: { dead_today: 1 } })), true);
});

test('今日汇总变了 → 要更新', () => {
  assert.equal(dlSnapshotChanged(status(), snap({ summary: { today: 4 } })), true);
  assert.equal(dlSnapshotChanged(status(), snap({ summary: { dedup_today: 2 } })), true);
});

test('当日条数变了 → 要更新（新行进来）', () => {
  assert.equal(dlSnapshotChanged(status(), snap({ items: [{ id: 1 }, { id: 2 }, { id: 3 }] })), true);
});

const dlItems = (bytes) => [
  { id: 1, dl: 'busy', dl_bytes: bytes, dl_total: 1000 },
  { id: 2, dl: 'ok', dl_bytes: 0, dl_total: null },
];

test('busy 行进度字节变了 → 要更新（下载中实时刷新靠它）', () => {
  assert.equal(
    dlSnapshotChanged(status({ items: dlItems(100) }), snap({ items: dlItems(250) })),
    true,
  );
});

test('busy 行进度字节没变 → 不换引用（同一毫秒快照不空转重渲）', () => {
  assert.equal(
    dlSnapshotChanged(status({ items: dlItems(100) }), snap({ items: dlItems(100) })),
    false,
  );
});

test('非 busy 行的 dl_bytes 不参与判变（终态行不可能变）', () => {
  // busy 行字节相同、pending 行字节不同 → 不换引用
  const items = (b) => [
    { id: 1, dl: 'busy', dl_bytes: 1, dl_total: 5 },
    { id: 2, dl: 'pending', dl_bytes: b, dl_total: null },
  ];
  assert.equal(
    dlSnapshotChanged(status({ items: items(0) }), snap({ items: items(77) })),
    false,
  );
});
