import { useState, useCallback } from "react";
import { api } from "../api";
import type { Image } from "../types";
import type { RescanStatus } from "./useVideos";
import { loadScanRoots } from "../scanRootStore";

/** 图片库数据层：与视频库各一份清单、各一张表，互不收录对方的文件 */
export function useImages() {
  const [images, setImages] = useState<Image[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rescanStatus, setRescanStatus] = useState<RescanStatus | null>(null);

  const refresh = useCallback(async () => {
    setImages(await api.getImages());
  }, []);

  /** 执行一次图片目录扫描并刷新列表；返回错误信息（null = 成功）。loading 由调用方管理 */
  const runScan = useCallback(async (dir: string): Promise<string | null> => {
    let scanError: string | null = null;
    try {
      await api.scanImageDirectory(dir);
    } catch (e) {
      scanError = `图片扫描失败，未完成更新：${String(e)}`;
    }
    if (scanError === null) {
      try {
        await refresh();
      } catch (e) {
        scanError = `扫描已保存，但刷新列表失败：${String(e)}。请重启应用刷新。`;
      }
    }
    return scanError;
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

  /** 删除后就地剔除本地清单：整库有 20 万条，重拉一次要过桥 70MB JSON 再重建目录树 */
  const dropLocally = useCallback((ids: Iterable<string>) => {
    const gone = new Set(ids);
    if (gone.size === 0) return;
    setImages(prev => prev.filter(image => !gone.has(image.id)));
  }, []);

  /** 移动后就地套用新路径。一次调用改多条：逐张 setImages 会把整表复制 N 遍 */
  const retargetLocally = useCallback((updates: Array<[imageId: string, newPath: string]>) => {
    if (updates.length === 0) return;
    const byId = new Map(updates);
    setImages(prev => prev.map(image => {
      const next = byId.get(image.id);
      if (next === undefined) return image;
      return { ...image, path: next, filename: next.split(/[\\/]/).pop() ?? image.filename };
    }));
  }, []);

  const initialRun = useCallback(async () => {
    await loadImages();
    await rescanAll();
  }, [loadImages, rescanAll]);

  const clearError = useCallback(() => setError(null), []);

  return {
    images, loading, error, rescanStatus, clearError,
    scanDirectory, loadImages, initialRun, dropLocally, retargetLocally,
  };
}
