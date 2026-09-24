import test from 'node:test';
import assert from 'node:assert/strict';
import { buildDirTree } from './dirTree.ts';

const paths = [
  { path: 'D:\\pics\\a\\1.jpg' },
  { path: 'D:\\pics\\a\\2.jpg' },
  { path: 'D:/pics/b/3.jpg' },
  { path: 'E:\\other\\deep\\nested\\4.jpg' },
  { path: 'no-dir-name.jpg' },
];

test('buildDirTree nests directories and tallies descendant counts', () => {
  const root = buildDirTree(paths, '所有图片');
  assert.equal(root.itemCount, paths.length);
  assert.deepEqual(root.children.map(c => c.name), ['D:', 'E:']);

  const d = root.children[0];
  assert.equal(d.itemCount, 3);
  const pics = d.children.find(c => c.name === 'pics');
  assert.equal(pics.path, 'D:\\pics');
  assert.equal(pics.itemCount, 3);
  assert.deepEqual(pics.children.map(c => c.name), ['a', 'b']);
  assert.equal(pics.children[0].itemCount, 2);
  assert.equal(pics.children[1].itemCount, 1);
});

test('buildDirTree merges a directory shared by slash and backslash paths', () => {
  const tree = buildDirTree([{ path: 'D:/x/1.jpg' }, { path: 'D:\\x\\2.jpg' }], '所有图片');
  const x = tree.children[0].children[0];
  assert.equal(x.children.length, 0, 'x 是叶子节点');
  assert.equal(x.itemCount, 2);
});

test('buildDirTree skips entries without a directory separator', () => {
  const tree = buildDirTree([{ path: 'loose.jpg' }, { path: 'D:\\k\\1.jpg' }], '所有图片');
  assert.equal(tree.itemCount, 2);
  assert.equal(tree.children.length, 1);
  assert.equal(tree.children[0].children[0].name, 'k');
});

test('buildDirTree reuses one node per path under wide fan-out', () => {
  const many = Array.from({ length: 5000 }, (_, i) => ({ path: `D:\\wide\\${i % 50}\\${i}.jpg` }));
  const tree = buildDirTree(many, '所有图片');
  const wide = tree.children[0].children[0];
  assert.equal(wide.children.length, 50);
  assert.equal(wide.children.reduce((n, c) => n + c.itemCount, 0), 5000);
});
