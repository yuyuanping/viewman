import test from 'node:test';
import assert from 'node:assert/strict';
import { parseScanRoots, isUnderDir, countUnderDir, countUnderDirs } from './scanRoots.ts';

test('scan roots dedupe case-insensitively and drop invalid entries', () => {
  const roots = parseScanRoots(['D:\\视频', 'd:\\视频\\', 'E:\\media', 42, null, '', 'D:\\视频 ']);
  assert.deepEqual(roots, ['D:\\视频', 'E:\\media']);
});

test('non-array input yields empty list', () => {
  assert.deepEqual(parseScanRoots(null), []);
  assert.deepEqual(parseScanRoots('D:\\x'), []);
  assert.deepEqual(parseScanRoots({ dir: 'D:\\x' }), []);
});

test('isUnderDir ignores case and separator style, and excludes the directory itself', () => {
  assert.equal(isUnderDir('D:\\pics\\a.jpg', 'D:\\pics'), true);
  assert.equal(isUnderDir('D:/pics/sub/a.jpg', 'd:\\pics\\'), true);
  assert.equal(isUnderDir('D:\\pics', 'D:\\pics'), false);
  assert.equal(isUnderDir('D:\\pics2\\a.jpg', 'D:\\pics'), false);
  assert.equal(isUnderDir('E:\\pics\\a.jpg', 'D:\\pics'), false);
  // 空前缀会把整盘误判成子目录
  assert.equal(isUnderDir('D:\\pics\\a.jpg', ''), false);
});

test('countUnderDir tallies only the entries hanging below the directory', () => {
  const items = [
    { path: 'D:\\pics\\a.jpg' },
    { path: 'D:\\pics\\sub\\b.jpg' },
    { path: 'D:\\pics2\\c.jpg' },
    { path: 'E:\\other\\d.jpg' },
  ];
  assert.equal(countUnderDir(items, 'D:\\pics'), 2);
  assert.equal(countUnderDir(items, 'D:\\nowhere'), 0);
});

test('countUnderDirs matches per-root counts and keeps input order', () => {
  const items = [
    { path: 'D:\\pics\\a.jpg' },
    { path: 'D:\\pics\\sub\\b.jpg' },
    { path: 'D:\\pics2\\c.jpg' },
    { path: 'E:\\other\\d.jpg' },
  ];
  const roots = ['E:\\other', 'd:\\pics\\', 'D:\\pics2', 'D:\\nowhere'];
  assert.deepEqual(countUnderDirs(items, roots), roots.map(dir => countUnderDir(items, dir)));
  assert.deepEqual(countUnderDirs(items, []), []);
});
