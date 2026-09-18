import test from 'node:test';
import assert from 'node:assert/strict';
import {
  filterVideos,
  selectedDirectoryLabel,
  sortVideos,
  filterByWatchState,
  watchStateOf,
  FINISHED_RATIO,
} from './libraryFilter.ts';
const videos = [
  { id: '1', path: 'D:/media/alpha.mp4', filename: 'alpha.mp4' },
  { id: '2', path: 'D:/media/Beta.MKV', filename: 'Beta.MKV' },
  { id: '3', path: 'E:/other/gamma.mp4', filename: 'gamma.mp4' },
];
test('filename search ignores case and clearing restores results', () => {
  assert.deepEqual(filterVideos(videos, null, 'BETA').map(v => v.id), ['2']);
  assert.equal(filterVideos(videos, null, '').length, 3);
  assert.deepEqual(filterVideos([], null, ''), []);
});
test('directory and search filters compose without matching sibling prefixes', () => {
  assert.deepEqual(filterVideos(videos, 'd:/media', '').map(v => v.id), ['1', '2']);
  assert.deepEqual(filterVideos(videos, 'D:/media', 'gamma'), []);
  assert.deepEqual(filterVideos(videos, 'D:/med', ''), []);
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

test('sortVideos sorts by each field without mutating the input', () => {
  const original = [...sortable];
  assert.deepEqual(sortVideos(sortable, 'file_size', 'asc').map(v => v.id), ['c', 'b', 'a']);
  assert.deepEqual(sortVideos(sortable, 'created_at', 'desc').map(v => v.id), ['c', 'b', 'a']);
  assert.deepEqual(sortable, original);
});

test('sortVideos keeps unknown duration at the end in both directions', () => {
  assert.deepEqual(sortVideos(sortable, 'duration', 'asc').map(v => v.id), ['c', 'b', 'a']);
  assert.deepEqual(sortVideos(sortable, 'duration', 'desc').map(v => v.id), ['b', 'c', 'a']);
});

test('filename sorting is numeric-aware so gamma2 precedes gamma10', () => {
  const names = [
    { id: '2', filename: 'ep2.mp4', duration: 1, file_size: 1, created_at: '' },
    { id: '10', filename: 'ep10.mp4', duration: 1, file_size: 1, created_at: '' },
  ];
  assert.deepEqual(sortVideos(names, 'filename', 'asc').map(v => v.id), ['2', '10']);
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
