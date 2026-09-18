/** 旧版把清单存在 localStorage 的键名，仅用于一次性迁移 */
export const LEGACY_SCAN_ROOTS_KEY = "viewman.scanRoots";

/** 规范化用于比较的键：小写 + 去掉结尾分隔符（Windows 路径大小写不敏感） */
export function dedupeKey(dir: string): string {
  return dir.toLowerCase().replace(/[\\/]+$/, "");
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
