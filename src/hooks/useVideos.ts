import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Video, RecentlyPlayed } from "../types";

export function useVideos() {
  const [videos, setVideos] = useState<Video[]>([]);
  const [loading, setLoading] = useState(false);
  const [progressMap, setProgressMap] = useState<Record<string, number | null>>({});
  const [recentlyPlayed, setRecentlyPlayed] = useState<RecentlyPlayed[]>([]);

  const loadVideos = useCallback(async () => {
    setLoading(true);
    try {
      const result = await invoke<{ video: Video; position: number | null }[]>("get_videos_with_progress");
      setVideos(result.map(r => r.video));
      const pmap: Record<string, number | null> = {};
      result.forEach(r => { pmap[r.video.id] = r.position; });
      setProgressMap(pmap);
    } catch (e) {
      console.error("Failed to load videos:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  const loadRecentlyPlayed = useCallback(async () => {
    try {
      const result = await invoke<RecentlyPlayed[]>("get_recently_played");
      setRecentlyPlayed(result);
    } catch (e) {
      console.error("Failed to load recently played:", e);
    }
  }, []);

  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    try {
      await invoke<Video[]>("scan_directory", { dir });
      await loadVideos();
      await loadRecentlyPlayed();
    } catch (e) {
      console.error("Failed to scan directory:", e);
    } finally {
      setLoading(false);
    }
  }, [loadVideos, loadRecentlyPlayed]);

  const saveProgress = useCallback(async (videoId: string, position: number) => {
    try {
      await invoke("save_progress", { videoId, position });
      setProgressMap(prev => ({ ...prev, [videoId]: position }));
      loadRecentlyPlayed();
    } catch (e) {
      console.error("Failed to save progress:", e);
    }
  }, [loadRecentlyPlayed]);

  useEffect(() => {
    loadVideos();
    loadRecentlyPlayed();
  }, [loadVideos, loadRecentlyPlayed]);

  return { videos, progressMap, recentlyPlayed, loading, scanDirectory, saveProgress, loadVideos, loadRecentlyPlayed };
}
