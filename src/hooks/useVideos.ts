import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Video } from "../types";

export function useVideos() {
  const [videos, setVideos] = useState<Video[]>([]);
  const [loading, setLoading] = useState(false);
  const [progressMap, setProgressMap] = useState<Record<string, number | null>>({});

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

  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    try {
      await invoke<Video[]>("scan_directory", { dir });
      await loadVideos();
    } catch (e) {
      console.error("Failed to scan directory:", e);
    } finally {
      setLoading(false);
    }
  }, [loadVideos]);

  const saveProgress = useCallback(async (videoId: string, position: number) => {
    try {
      await invoke("save_progress", { videoId, position });
      setProgressMap(prev => ({ ...prev, [videoId]: position }));
    } catch (e) {
      console.error("Failed to save progress:", e);
    }
  }, []);

  useEffect(() => {
    loadVideos();
  }, [loadVideos]);

  return { videos, progressMap, loading, scanDirectory, saveProgress, loadVideos };
}
