import { useState, useCallback } from "react";
import { api } from "../api";
import type { ImageStatsPayload } from "../api";
import type { RescanStatus } from "./useVideos";
import { loadScanRoots } from "../scanRootStore";

/**
 * 图片库数据层。库不再整表下发：前端只持有两样东西——
 * `libraryVersion`（库内容代次，ImageLibrary 靠它重拉当前视图）和
 * `imageStats`（侧栏目录树/计数徽章用的聚合）。目录与搜索过滤都在后端。
 */
export function useImages() {
  const [imageStats, setImageStats] = useState<ImageStatsPayload | null>(null);
  const [libraryVersion, setLibraryVersion] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rescanStatus, setRescanStatus] = useState<RescanStatus | null>(null);

  /** 库内容变了：统计重拉、代次前进让 ImageLibrary 重拉视图 */
  const refresh = useCallback(async () => {
    setLibraryVersion(v => v + 1);
    setImageStats(await api.getImageStats());
  }, []);

  /** 扫描完成后库已在后端更新，这里只负责让前端重新拉取；返回错误信息（null = 成功） */
  const runScan = useCallback(async (dir: string): Promise<string | null> => {
    try {
      await api.scanImageDirectory(dir);
      await refresh();
      return null;
    } catch (e) {
      return `图片扫描失败，未完成更新：${String(e)}`;
    }
  }, [refresh]);

  /** 扫描根由后端在能读到目录时即刻记下，扫到一半被打断，下次启动会自己接着扫 */
  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    setError(null);
    try {
      const result = await runScan(dir);
      if (result !== null) {
        setError(result);
      }
    } finally {
      setLoading(false);
    }
  }, [runScan]);

  /** 打开应用时自动重新扫描已记录的图片目录；个别目录失败只汇总提示 */
  const rescanAll = useCallback(async () => {
    const roots = await loadScanRoots("image");
    if (roots.length === 0) return;
    setLoading(true);
    const failed: string[] = [];
    try {
      for (let i = 0; i < roots.length; i++) {
        setRescanStatus({ current: i + 1, total: roots.length, dir: roots[i] });
        if (await runScan(roots[i]) !== null) {
          failed.push(roots[i]);
        }
      }
    } finally {
      setRescanStatus(null);
      setLoading(false);
    }
    if (failed.length > 0) {
      setError(`部分图片目录自动扫描失败：${failed.join("、")}（磁盘可能未连接；扫描失败的目录不会被清理）`);
    }
  }, [runScan]);

  const loadImages = useCallback(async () => {
    setLoading(true);
    try {
      await refresh();
    } catch (e) {
      setError(`读取图片库失败：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [refresh]);

  const initialRun = useCallback(async () => {
    await loadImages();
    await rescanAll();
  }, [loadImages, rescanAll]);

  const clearError = useCallback(() => setError(null), []);

  return {
    imageStats, libraryVersion, loading, error, rescanStatus, clearError,
    scanDirectory, loadImages, initialRun, refresh,
  };
}
