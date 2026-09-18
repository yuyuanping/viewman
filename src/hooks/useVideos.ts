import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Video, RecentlyPlayed, VideoWithProgress } from "../types";

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

  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    setError(null);
    try {
      await invoke<Video[]>("scan_directory", { dir });
      try {
        await refresh();
      } catch (e) {
        setError(`扫描已保存，但刷新列表失败：${String(e)}。请重启应用刷新。`);
      }
    } catch (e) {
      setError(`扫描失败，未完成更新：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [refresh]);

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

  useEffect(() => { void loadVideos(); }, [loadVideos]);
  const clearError = useCallback(() => setError(null), []);
  return { videos, progressMap, recentlyPlayed, loading, error, clearError, scanDirectory, saveProgress, loadVideos };
}
