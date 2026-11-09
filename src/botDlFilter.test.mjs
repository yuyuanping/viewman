import test from 'node:test';
import assert from 'node:assert/strict';
import {
  dlStatusOf,
  countByDlStatus,
  filterByDlStatus,
  collapseDeadDuplicates,
  dlItemLabel,
  dlSpeedOf,
  dlForwardTaskText,
  forwardGroupStats,
  DL_STATUS_ORDER,
  DL_STATUS_LABEL,
} from './botDlFilter.ts';

const row = (id, dl, kind = 'image', dedup = 0, parent = null, sender = null) => ({
  id, dl, kind, dedup, parent, sender, label: `#${id}`, size: null, err: null,
});

test('下载队列状态压过一切其它标记；排队与在传合并为「下载中」', () => {
  assert.equal(dlStatusOf(row(1, 'busy')), 'downloading');
  assert.equal(dlStatusOf(row(2, 'pending', 'image', 1)), 'downloading', '排队也归下载中，dedup 不改变归类');
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

test('计数覆盖五个状态，未出现的状态为 0', () => {
  const counts = countByDlStatus([row(1, 'busy'), row(2, 'pending'), row(3, 'dead'), row(4, 'ok')]);
  assert.deepEqual(counts, { downloading: 2, dead: 1, dedup: 0, ok: 1, archived: 0 });
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

test('下载速度：8 秒窗口差分，未满 2s 无速度、2~8s 给至今平均，停滞 15 秒归零，收敛与重传重开窗口', () => {
  const busyRow = bytes => ({ id: 9001, dl: 'busy', dl_bytes: bytes });
  assert.equal(dlSpeedOf(busyRow(0), 1000), 0, '首见无速度');
  assert.equal(dlSpeedOf(busyRow(1_000_000), 1000 + 1_500), 0, '开跑不足 2s 无速度');
  assert.equal(dlSpeedOf(busyRow(4_000_000), 1000 + 5_000), 800_000, '窗口未满给至今平均：4MB/5s');
  assert.equal(dlSpeedOf(busyRow(12_000_000), 1000 + 8_000), 1_500_000, '窗口满：12MB/8s');
  assert.equal(dlSpeedOf(busyRow(12_000_000), 1000 + 8_000 + 16_000), 0, '停滞 15s 归零');
  assert.equal(dlSpeedOf(busyRow(0), 1000 + 8_000 + 16_000 + 1_000), 0, '进度清零=重传，重开窗口');
  assert.equal(dlSpeedOf({ id: 9001, dl: 'ok', dl_bytes: 99 }, 9e9), 0, '收敛清状态');
  assert.equal(dlSpeedOf(busyRow(5_000_000), 9e9 + 1_000), 0, '清状态后再 busy 从头起窗');
});

test('转发任务态：busy 壳显 解析中/下载中 N/M 个；排队壳与普通行走常规标签', () => {
  assert.equal(dlForwardTaskText({ kind: 'forward', dl: 'busy' }), '解析中', '明细未登记=展开中');
  assert.equal(dlForwardTaskText({ kind: 'forward', dl: 'busy' }, { total: 3, done: 1 }), '下载中 1/3 个');
  assert.equal(dlForwardTaskText({ kind: 'reply', dl: 'busy' }), '解析中', '引用壳取原文也是解析');
  assert.equal(dlForwardTaskText({ kind: 'forward', dl: 'pending' }, { total: 3, done: 0 }), null, '排队的壳走常规「下载中」');
  assert.equal(dlForwardTaskText({ kind: 'image', dl: 'busy' }, { total: 1, done: 0 }), null, '普通行不受影响');

  const stats = forwardGroupStats([
    { id: 1, parent: null, dl: 'busy', kind: 'forward', dedup: 0 },
    { id: 2, parent: 1, dl: 'ok', kind: 'image', dedup: 0 },
    { id: 3, parent: 1, dl: 'pending', kind: 'image', dedup: 0 },
    { id: 4, parent: 1, dl: 'none', kind: 'text', dedup: 0 },
    { id: 5, parent: 1, dl: 'ok', kind: 'image', dedup: 1 },
  ]);
  assert.deepEqual(stats.get(1), { total: 4, done: 3 }, 'ok/重复跳过/归档都算完成，pending 不算');
  assert.equal(stats.get(2), undefined, '明细不再聚合');
});

test('失败聚合：同内容副本合并成一行 ×N，代表行取最近转死的', () => {
  const dead = (id, name, dead_at, kind = 'video', parent = null) => ({
    id, dl: 'dead', kind, name, dead_at, parent, dedup: 0, label: name, err: 'boom', size: null, sender: null,
  });
  const items = [
    dead(1, 'a.mp4', 100, 'video', 10),
    dead(2, 'a.mp4', 200, 'video', 20),
    dead(3, 'a.mp4', 200, 'video', 30),
    { ...dead(9, 'a.mp4', 300), dl: 'ok' },  // 同名但已成功：不进死行组
    dead(4, 'b.jpg', 150, 'image', 40),      // 单份死行：不聚
    dead(5, null, 160),                      // 无名死行：没有内容判据，不聚
    dead(6, 'a.mp4', 50, 'image', 50),       // 同名不同类型：不聚
  ];
  const { rows, groups } = collapseDeadDuplicates(items);
  // 代表行 = 组内最近转死的（dead_at 同为 200 时取 id 小的）→ id 2；副本 1/3 隐藏
  assert.deepEqual(groups.get(2), [1, 2, 3]);
  assert.equal(groups.size, 1, '单份与无名行不成组');
  assert.deepEqual(rows.map(i => i.id), [2, 9, 4, 5, 6], '隐藏副本、其余行原序保留');
  // 计数与筛选吃聚合后的行：失败角标数 = 看得见的行数，不是在册副本数
  assert.equal(countByDlStatus(rows).dead, 4);
  assert.deepEqual(filterByDlStatus(rows, 'dead').map(i => i.id), [2, 4, 5, 6]);
});

test('失败聚合：两份同刻转死取 id 小的；空列表不炸', () => {
  const dead = (id, name, dead_at) => ({
    id, dl: 'dead', kind: 'file', name, dead_at, parent: null, dedup: 0, label: name, err: null, size: null, sender: null,
  });
  const { rows, groups } = collapseDeadDuplicates([dead(7, 'r.pdf', 500), dead(3, 'r.pdf', 500)]);
  assert.deepEqual(groups.get(3), [3, 7], '同刻取 id 小的当代表');
  assert.deepEqual(rows.map(i => i.id), [3]);
  assert.deepEqual(collapseDeadDuplicates([]), { rows: [], groups: new Map() });
});