import { api } from "./api";
import type { MediaKind } from "./types";
import { LEGACY_SCAN_ROOTS_KEY, parseScanRoots } from "./scanRoots";

/** 读取指定库已记录的扫描目录；读取失败按空处理（只影响自动重扫，不影响手动功能） */
export async function loadScanRoots(kind: MediaKind): Promise<string[]> {
  try {
    return parseScanRoots(await api.loadScanRoots(kind));
  } catch {
    return [];
  }
}

/** 把旧版 localStorage 里的记录迁入数据库（仅一次；数据库已有记录或无旧记录则不动） */
export async function migrateLegacyScanRoots(): Promise<void> {
  try {
    const legacy = parseScanRoots(JSON.parse(localStorage.getItem(LEGACY_SCAN_ROOTS_KEY) ?? "null"));
    if (legacy.length === 0) return;
    const existing = await loadScanRoots("video");
    if (existing.length === 0) {
      await api.saveScanRoots("video", legacy);
    }
    localStorage.removeItem(LEGACY_SCAN_ROOTS_KEY);
  } catch {
    // 迁移失败下次启动重试
  }
}
