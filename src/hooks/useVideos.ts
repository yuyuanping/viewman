import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Video, RecentlyPlayed, VideoWithProgress } from "../types";
import { loadScanRoots, rememberScanRoot } from "../scanRoots";

export function useVideos() {
  const [videos, setVideos] = useState<Video[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progressMap, setProgressMap] = useState<Record<string, number | null>>({});
  const [recentlyPlayed, setRecentlyPlayed] = useState<RecentlyPlayed[]>([]);

  const refresh = useCallback(async () => {
    const [result, recent] = await Promise.all([
      invoke<VideoWithProgress[]>("get_videos_with_progress"),
      invoke<RecentlyPlayed[]>("get_recently_played"),
    ]);
    setVideos(result.map(r => r.video));
    setProgressMap(Object.fromEntries(result.map(r => [r.video.id, r.position])));
    setRecentlyPlayed(recent);
  }, []);

  /** 执行一次目录扫描并刷新列表；返回错误信息（null = 成功）。loading 由调用方管理 */
  const runScan = useCallback(async (dir: string): Promise<string | null> => {
    let scanError: string | null = null;
    try {
      await invoke<Video[]>("scan_directory", { dir });
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

  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    setError(null);
    try {
      const result = await runScan(dir);
      if (result !== null) {
        setError(result);
      } else {
        rememberScanRoot(dir);
      }
    } finally {
      setLoading(false);
    }
  }, [runScan]);

  /** 打开应用时自动重新扫描所有已记录的目录；个别目录失败只汇总提示，不影响其余目录 */
  const rescanAll = useCallback(async () => {
    const roots = loadScanRoots();
    if (roots.length === 0) return;
    setLoading(true);
    const failed: string[] = [];
    try {
      for (const root of roots) {
        if (await runScan(root) !== null) {
          failed.push(root);
        }
      }
    } finally {
      setLoading(false);
    }
    if (failed.length > 0) {
      setError(`部分目录自动扫描失败：${failed.join("、")}（磁盘可能未连接；扫描失败的目录不会被清理）`);
    }
  }, [runScan]);

  const saveProgress = useCallback(async (videoId: string, position: number) => {
    try {
      await invoke("save_progress", { videoId, position });
      setProgressMap(prev => ({ ...prev, [videoId]: position }));
    } catch (e) {
      setError(`播放进度保存失败：${String(e)}`);
      throw e;
    }
    try {
      setRecentlyPlayed(await invoke<RecentlyPlayed[]>("get_recently_played"));
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

  useEffect(() => {
    void (async () => {
      await loadVideos();
      await rescanAll();
    })();
  }, [loadVideos, rescanAll]);

  const clearError = useCallback(() => setError(null), []);
  return { videos, progressMap, recentlyPlayed, loading, error, clearError, scanDirectory, saveProgress, loadVideos };
}
