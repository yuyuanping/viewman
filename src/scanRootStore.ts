import { api } from "./api";
import { LEGACY_SCAN_ROOTS_KEY, dedupeKey, parseScanRoots } from "./scanRoots";

/** 从数据库读取已记录的扫描目录；读取失败按空处理（只影响自动重扫，不影响手动功能） */
export async function loadScanRoots(): Promise<string[]> {
  try {
    return parseScanRoots(await api.loadScanRoots());
  } catch {
    return [];
  }
}

/** 追加记录一个扫描目录（去重后整体写回）；写库失败不影响本次扫描结果 */
export async function rememberScanRoot(dir: string): Promise<void> {
  try {
    const roots = await loadScanRoots();
    const key = dedupeKey(dir);
    if (roots.some(root => dedupeKey(root) === key)) return;
    await api.saveScanRoots([...roots, dir]);
  } catch {
    // 存储失败不影响本次扫描
  }
}

/** 把旧版 localStorage 里的记录迁入数据库（仅一次；数据库已有记录或无旧记录则不动） */
export async function migrateLegacyScanRoots(): Promise<void> {
  try {
    const legacy = parseScanRoots(JSON.parse(localStorage.getItem(LEGACY_SCAN_ROOTS_KEY) ?? "null"));
    if (legacy.length === 0) return;
    const existing = await loadScanRoots();
    if (existing.length === 0) {
      await api.saveScanRoots(legacy);
    }
    localStorage.removeItem(LEGACY_SCAN_ROOTS_KEY);
  } catch {
    // 迁移失败下次启动重试
  }
}
