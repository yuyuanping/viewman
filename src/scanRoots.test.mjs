import test from 'node:test';
import assert from 'node:assert/strict';
import { parseScanRoots } from './scanRoots.ts';

test('scan roots dedupe case-insensitively and drop invalid entries', () => {
  const roots = parseScanRoots(['D:\\视频', 'd:\\视频\\', 'E:\\media', 42, null, '', 'D:\\视频 ']);
  assert.deepEqual(roots, ['D:\\视频', 'E:\\media']);
});

test('non-array input yields empty list', () => {
  assert.deepEqual(parseScanRoots(null), []);
  assert.deepEqual(parseScanRoots('D:\\x'), []);
  assert.deepEqual(parseScanRoots({ dir: 'D:\\x' }), []);
});
