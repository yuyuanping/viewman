import { useState, useCallback } from "react";
import type { Video } from "../types";

export function usePlayer() {
  const [currentVideo, setCurrentVideo] = useState<Video | null>(null);
  const [initialPosition, setInitialPosition] = useState(0);

  const openPlayer = useCallback((video: Video, position: number = 0) => {
    setCurrentVideo(video);
    setInitialPosition(position);
  }, []);

  const closePlayer = useCallback(() => {
    setCurrentVideo(null);
    setInitialPosition(0);
  }, []);

  return { currentVideo, initialPosition, openPlayer, closePlayer };
}
