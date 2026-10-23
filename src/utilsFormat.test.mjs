import test from 'node:test';
import assert from 'node:assert/strict';
import { formatBytes, formatDlSize, formatFileSize } from './utils.ts';

const KB = 1024;
const MB = KB * 1024;

test('formatBytes floors to one decimal and trims a trailing .0', () => {
  assert.equal(formatBytes(KB), '1 KB');
  assert.equal(formatBytes(1536), '1.5 KB');
  assert.equal(formatBytes(123456789), '117.7 MB');
});

test('formatBytes placeholder is caller supplied', () => {
  assert.equal(formatBytes(null), '-');
  assert.equal(formatBytes(0), '-');
  assert.equal(formatBytes(NaN), '-');
  assert.equal(formatBytes(0, '0 B'), '0 B');
  assert.equal(formatFileSize(0), '0 B');
});

test('formatDlSize keeps the QQ panel rounding (whole KB shows as "1.0 KB")', () => {
  // 与 formatBytes 分开的理由就这几条：下载量数字不能因为统一实现而变样
  assert.equal(formatDlSize(KB), '1.0 KB');
  assert.equal(formatDlSize(1536), '1.5 KB');
  assert.equal(formatDlSize(100 * MB), '100 MB');
  assert.equal(formatDlSize(1023), '1023 B');
  assert.notEqual(formatDlSize(KB), formatBytes(KB));
});

test('formatDlSize placeholder', () => {
  assert.equal(formatDlSize(null), '-');
  assert.equal(formatDlSize(0), '-');
  assert.equal(formatDlSize(-1), '-');
});
