/** 旧版把清单存在 localStorage 的键名，仅用于一次性迁移 */
export const LEGACY_SCAN_ROOTS_KEY = "viewman.scanRoots";

/** 规范化用于比较的键：小写 + 去掉结尾分隔符（Windows 路径大小写不敏感） */
export function dedupeKey(dir: string): string {
  return dir.toLowerCase().replace(/[\\/]+$/, "");
}

/** path 是否位于 dir 之内（不含 dir 自身）：大小写与 \ / 分隔符都不敏感 */
export function isUnderDir(path: string, dir: string): boolean {
  const parent = dedupeKey(dir);
  if (!parent) return false;
  return path.toLowerCase().replace(/\//g, "\\").startsWith(parent + "\\");
}

/** 统计挂在某目录下的条目数，用于"移除目录"前告知影响范围 */
export function countUnderDir(items: { path: string }[], dir: string): number {
  return items.reduce((n, item) => (isUnderDir(item.path, dir) ? n + 1 : n), 0);
}

/**
 * 一次遍历统计多个扫描根。逐个根调 countUnderDir 会把同一条路径
 * toLowerCase + 替换分隔符重复做 N 遍，19 万条 × 3 根实测 240ms/次渲染。
 * 返回值与入参 dirs 一一对应。
 */
export function countUnderDirs(items: { path: string }[], dirs: string[]): number[] {
  const counts = dirs.map(() => 0);
  if (dirs.length === 0) return counts;
  const parents = dirs.map(dir => dedupeKey(dir) + "\\");
  for (const item of items) {
    const path = item.path.toLowerCase().replace(/\//g, "\\");
    for (let i = 0; i < parents.length; i++) {
      if (path.startsWith(parents[i])) counts[i] += 1;
    }
  }
  return counts;
}

/** 平表（父目录 → 直接文件数）挂在 dir 下的计数：目录自身直接挂的 + 子树里的 */
function countUnderDirInCounts(dirCounts: Array<[string, number]>, dir: string): number {
  const parent = dedupeKey(dir);
  if (!parent) return 0;
  let n = 0;
  for (const [sub, count] of dirCounts) {
    const key = sub.toLowerCase().replace(/\//g, "\\");
    if (key === parent || key.startsWith(parent + "\\")) n += count;
  }
  return n;
}

/** countUnderDirs 的平表版：图片库不再整表下发，按目录计数聚合 */
export function countUnderDirsFromCounts(dirCounts: Array<[string, number]>, dirs: string[]): number[] {
  return dirs.map(dir => countUnderDirInCounts(dirCounts, dir));
}

/** countUnderDir 的平表版：移除扫描根前的影响范围提示 */
export function countUnderDirFromCounts(dirCounts: Array<[string, number]>, dir: string): number {
  return countUnderDirInCounts(dirCounts, dir);
}

/** 解析并规范化扫描目录清单：去重（大小写不敏感，Windows 路径）、剔除非字符串与空值 */
export function parseScanRoots(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const roots: string[] = [];
  for (const item of raw) {
    if (typeof item !== "string") continue;
    const dir = item.trim();
    if (!dir) continue;
    const key = dedupeKey(dir);
    if (seen.has(key)) continue;
    seen.add(key);
    roots.push(dir);
  }
  return roots;
}
