import test from 'node:test';
import assert from 'node:assert/strict';
import {
  filterMedia,
  selectedDirectoryLabel,
  sortMedia,
  filterByWatchState,
  filterByMedia,
  watchStateOf,
  FINISHED_RATIO,
} from './libraryFilter.ts';
const videos = [
  { id: '1', path: 'D:/media/alpha.mp4', filename: 'alpha.mp4' },
  { id: '2', path: 'D:/media/Beta.MKV', filename: 'Beta.MKV' },
  { id: '3', path: 'E:/other/gamma.mp4', filename: 'gamma.mp4' },
];
test('filename search ignores case and clearing restores results', () => {
  assert.deepEqual(filterMedia(videos, null, 'BETA').map(v => v.id), ['2']);
  assert.equal(filterMedia(videos, null, '').length, 3);
  assert.deepEqual(filterMedia([], null, ''), []);
});
test('directory and search filters compose without matching sibling prefixes', () => {
  assert.deepEqual(filterMedia(videos, 'd:/media', '').map(v => v.id), ['1', '2']);
  assert.deepEqual(filterMedia(videos, 'D:/media', 'gamma'), []);
  assert.deepEqual(filterMedia(videos, 'D:/med', ''), []);
});
test('selected directory label supports root and child folders', () => {
  assert.equal(selectedDirectoryLabel('D:/media/sub'), 'sub');
  assert.equal(selectedDirectoryLabel(null), '所有视频');
});

const sortable = [
  { id: 'b', filename: 'beta.mp4', duration: 200, file_size: 500, created_at: '2026-02-01T00:00:00' },
  { id: 'a', filename: 'Alpha.mp4', duration: null, file_size: 900, created_at: '2026-01-01T00:00:00' },
  { id: 'c', filename: 'gamma10.mp4', duration: 100, file_size: 100, created_at: '2026-03-01T00:00:00' },
];

test('sortMedia sorts by each field without mutating the input', () => {
  const original = [...sortable];
  assert.deepEqual(sortMedia(sortable, 'file_size', 'asc').map(v => v.id), ['c', 'b', 'a']);
  assert.deepEqual(sortMedia(sortable, 'created_at', 'desc').map(v => v.id), ['c', 'b', 'a']);
  assert.deepEqual(sortable, original);
});

test('sortMedia keeps unknown duration at the end in both directions', () => {
  assert.deepEqual(sortMedia(sortable, 'duration', 'asc').map(v => v.id), ['c', 'b', 'a']);
  assert.deepEqual(sortMedia(sortable, 'duration', 'desc').map(v => v.id), ['b', 'c', 'a']);
});

test('image entries carry no duration and still filter and sort by their own fields', () => {
  const images = [
    { id: '2', path: 'D:/pics/b.png', filename: 'b.png', file_size: 30, created_at: '2026-02-01T00:00:00' },
    { id: '1', path: 'D:/pics/a.png', filename: 'a.png', file_size: 10, created_at: '2026-01-01T00:00:00' },
    { id: '3', path: 'E:/else/c.png', filename: 'c.png', file_size: 20, created_at: '2026-03-01T00:00:00' },
  ];
  assert.deepEqual(filterMedia(images, 'd:/pics', '').map(v => v.id), ['2', '1']);
  assert.deepEqual(sortMedia(images, 'file_size', 'asc').map(v => v.id), ['1', '3', '2']);
  assert.equal(selectedDirectoryLabel(null, '所有图片'), '所有图片');
});

test('filename sorting is numeric-aware so gamma2 precedes gamma10', () => {
  const names = [
    { id: '2', filename: 'ep2.mp4', duration: 1, file_size: 1, created_at: '' },
    { id: '10', filename: 'ep10.mp4', duration: 1, file_size: 1, created_at: '' },
  ];
  assert.deepEqual(sortMedia(names, 'filename', 'asc').map(v => v.id), ['2', '10']);
});

test('watch state derives from progress and duration', () => {
  assert.equal(watchStateOf({ duration: 100 }, 0), 'unwatched');
  assert.equal(watchStateOf({ duration: 100 }, null), 'unwatched');
  assert.equal(watchStateOf({ duration: 100 }, 30), 'in_progress');
  assert.equal(watchStateOf({ duration: 100 }, 100 * FINISHED_RATIO), 'finished');
  assert.equal(watchStateOf({ duration: null }, 42), 'in_progress');
});

test('filterByWatchState narrows the list and passes everything through for all', () => {
  const videos = [
    { id: 'a', duration: 100 },
    { id: 'b', duration: 100 },
    { id: 'c', duration: 100 },
  ];
  const progress = { a: null, b: 99, c: 10 };
  const resolveId = v => v.id;
  assert.equal(filterByWatchState(videos, progress, resolveId, 'all').length, 3);
  assert.deepEqual(filterByWatchState(videos, progress, resolveId, 'unwatched').map(v => v.id), ['a']);
  assert.deepEqual(filterByWatchState(videos, progress, resolveId, 'finished').map(v => v.id), ['b']);
  assert.deepEqual(filterByWatchState(videos, progress, resolveId, 'in_progress').map(v => v.id), ['c']);
});

test('filterByMedia applies size, duration and height floors', () => {
  const items = [
    { id: 'big', duration: 3600, file_size: 5 * 1024 ** 3, height: 2160 },   // 1h, 5GB, 4K
    { id: 'mid', duration: 1200, file_size: 900 * 1024 ** 2, height: 1080 },   // 20min, 900MB, 1080p
    { id: 'small', duration: 60, file_size: 20 * 1024 ** 2, height: 480 },     // 1min, 20MB, 480p
    { id: 'unknown', duration: null, file_size: 100, height: null },          // 探测失败
  ];
  // 无条件：全过
  assert.equal(filterByMedia(items, null).length, 4);
  // 只限大小
  assert.deepEqual(filterByMedia(items, { minSize: 1024 ** 3 }).map(v => v.id), ['big']);
  // 只限时长：时长未知直接排除
  assert.deepEqual(filterByMedia(items, { minDuration: 1800 }).map(v => v.id), ['big']);
  // 只限分辨率：height 未知排除
  assert.deepEqual(filterByMedia(items, { minHeight: 1080 }).map(v => v.id), ['big', 'mid']);
  // 组合
  assert.deepEqual(filterByMedia(items, { minDuration: 600, minSize: 100 * 1024 ** 2, minHeight: 720 }).map(v => v.id), ['big', 'mid']);
});

test('sortMedia by modified_at puts entries without mtime at the end', () => {
  const items = [
    { id: 'no-mtime', filename: 'a.png', file_size: 1, created_at: '', modified_at: null },
    { id: 'new', filename: 'b.png', file_size: 1, created_at: '', modified_at: '2026-03-01T00:00:00' },
    { id: 'old', filename: 'c.png', file_size: 1, created_at: '', modified_at: '2026-01-01T00:00:00' },
  ];
  assert.deepEqual(sortMedia(items, 'modified_at', 'asc').map(v => v.id), ['old', 'new', 'no-mtime']);
  // 切换方向缺失值仍在末尾
  assert.deepEqual(sortMedia(items, 'modified_at', 'desc').map(v => v.id), ['new', 'old', 'no-mtime']);
});
