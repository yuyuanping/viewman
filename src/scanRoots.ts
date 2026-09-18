const SCAN_ROOTS_KEY = "viewman.scanRoots";

/** 规范化用于比较的键：小写 + 去掉结尾分隔符（Windows 路径大小写不敏感） */
function dedupeKey(dir: string): string {
  return dir.toLowerCase().replace(/[\\/]+$/, "");
}

/** 解析并规范化已保存的扫描目录：去重（大小写不敏感，Windows 路径）、剔除非字符串与空值 */
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

export function loadScanRoots(): string[] {
  try {
    return parseScanRoots(JSON.parse(localStorage.getItem(SCAN_ROOTS_KEY) ?? "null"));
  } catch {
    return [];
  }
}

export function rememberScanRoot(dir: string): void {
  const key = dedupeKey(dir);
  if (loadScanRoots().some(root => dedupeKey(root) === key)) return;
  try {
    localStorage.setItem(SCAN_ROOTS_KEY, JSON.stringify([...loadScanRoots(), dir]));
  } catch {
    // 存储失败不影响本次扫描
  }
}
