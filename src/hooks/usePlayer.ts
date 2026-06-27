import { useState, useCallback, useRef, useEffect } from "react";
import type { Video } from "../types";

export function usePlayer(saveProgress: (videoId: string, position: number) => Promise<void>) {
  const [currentVideo, setCurrentVideo] = useState<Video | null>(null);
  const [initialPosition, setInitialPosition] = useState(0);
  const autoSaveRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const openPlayer = useCallback((video: Video, position: number = 0) => {
    setCurrentVideo(video);
    setInitialPosition(position);
  }, []);

  const closePlayer = useCallback(() => {
    setCurrentVideo(null);
    setInitialPosition(0);
  }, []);

  useEffect(() => {
    if (currentVideo && autoSaveRef.current === null) {
      autoSaveRef.current = setInterval(() => {}, 15000);
    }
    if (!currentVideo && autoSaveRef.current) {
      clearInterval(autoSaveRef.current);
      autoSaveRef.current = null;
    }
    return () => {
      if (autoSaveRef.current) {
        clearInterval(autoSaveRef.current);
        autoSaveRef.current = null;
      }
    };
  }, [currentVideo]);

  return { currentVideo, initialPosition, openPlayer, closePlayer };
}
