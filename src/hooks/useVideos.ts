import { useState, useEffect, useCallback } from "react";
import { api } from "../api";
import type { RecentlyPlayed, Video } from "../types";
import { loadScanRoots, migrateLegacyScanRoots } from "../scanRootStore";

/** 自动重扫的目录级进度 */
export interface RescanStatus {
  /** 当前是第几个目录（从 1 开始） */
  current: number;
  /** 待重扫目录总数 */
  total: number;
  dir: string;
}

export function useVideos() {
  const [videos, setVideos] = useState<Video[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progressMap, setProgressMap] = useState<Record<string, number | null>>({});
  const [recentlyPlayed, setRecentlyPlayed] = useState<RecentlyPlayed[]>([]);
  const [rescanStatus, setRescanStatus] = useState<RescanStatus | null>(null);

  const refresh = useCallback(async () => {
    const [result, recent] = await Promise.all([
      api.getVideosWithProgress(),
      api.getRecentlyPlayed(),
    ]);
    setVideos(result.map(r => r.video));
    setProgressMap(Object.fromEntries(result.map(r => [r.video.id, r.position])));
    setRecentlyPlayed(recent);
  }, []);

  /** 执行一次目录扫描并刷新列表；返回错误信息（null = 成功）。loading 由调用方管理 */
  const runScan = useCallback(async (dir: string): Promise<string | null> => {
    let scanError: string | null = null;
    try {
      await api.scanDirectory(dir);
    } catch (e) {
      scanError = `扫描失败，未完成更新：${String(e)}`;
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

  /** 打开应用时自动重新扫描所有已记录的目录；个别目录失败只汇总提示，不影响其余目录 */
  const rescanAll = useCallback(async () => {
    const roots = await loadScanRoots("video");
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
      setError(`部分目录自动扫描失败：${failed.join("、")}（磁盘可能未连接；扫描失败的目录不会被清理）`);
    }
  }, [runScan]);

  const saveProgress = useCallback(async (videoId: string, position: number) => {
    try {
      await api.saveProgress(videoId, position);
      setProgressMap(prev => ({ ...prev, [videoId]: position }));
    } catch (e) {
      setError(`播放进度保存失败：${String(e)}`);
      throw e;
    }
    try {
      setRecentlyPlayed(await api.getRecentlyPlayed());
    } catch (e) {
      setError(`进度已保存，但播放记录刷新失败：${String(e)}`);
    }
  }, []);

  const loadVideos = useCallback(async () => {
    setLoading(true);
    try {
      await refresh();
    } catch (e) {
      setError(`读取视频库失败：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [refresh]);

  /** 删除后就地剔除本地清单：整库重拉一次要过桥几十 MB JSON，还要重建目录树 */
  const dropLocally = useCallback((ids: Iterable<string>) => {
    const gone = new Set(ids);
    if (gone.size === 0) return;
    setVideos(prev => prev.filter(video => !gone.has(video.id)));
    setProgressMap(prev => {
      const next = { ...prev };
      for (const id of gone) delete next[id];
      return next;
    });
    setRecentlyPlayed(prev => prev.filter(entry => !gone.has(entry.video.id)));
  }, []);

  /** 移动后就地套用后端返回的新路径。一次调用改多条：逐张 setVideos 会把整表复制 N 遍 */
  const retargetLocally = useCallback((updates: Array<[videoId: string, newPath: string]>) => {
    if (updates.length === 0) return;
    const byId = new Map(updates);
    const patch = (video: Video): Video => {
      const next = byId.get(video.id);
      if (next === undefined) return video;
      return { ...video, path: next, filename: next.split(/[\\/]/).pop() ?? video.filename };
    };
    setVideos(prev => prev.map(patch));
    setRecentlyPlayed(prev => prev.map(entry => ({ ...entry, video: patch(entry.video) })));
  }, []);

  useEffect(() => {
    void (async () => {
      await migrateLegacyScanRoots();
      await loadVideos();
      await rescanAll();
    })();
  }, [loadVideos, rescanAll]);

  const clearError = useCallback(() => setError(null), []);
  return { videos, progressMap, recentlyPlayed, loading, error, rescanStatus, clearError, scanDirectory, saveProgress, loadVideos, dropLocally, retargetLocally };
}
