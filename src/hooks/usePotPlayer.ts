import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PotPlayerStatus, Video } from "../types";

const POLL_INTERVAL_MS = 10000;

interface Session {
  videoId: string;
  videoPath: string;
  startPos: number;
  startedAt: number;
  lastReal: number | null;
}

function estimatedPosition(session: Session): number {
  return session.startPos + (Date.now() - session.startedAt) / 1000;
}

export function usePotPlayer(saveProgress: (videoId: string, position: number) => Promise<void>) {
  const [error, setError] = useState<string | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const sessionRef = useRef<Session | null>(null);

  const stop = useCallback(() => {
    if (timerRef.current !== null) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const finish = useCallback((session: Session, finalPos: number | null) => {
    stop();
    sessionRef.current = null;
    if (finalPos !== null && finalPos > 0) {
      saveProgress(session.videoId, finalPos);
    }
  }, [stop, saveProgress]);

  const poll = useCallback(async () => {
    const session = sessionRef.current;
    if (!session) return;
    try {
      const status = await invoke<PotPlayerStatus>("potplayer_status", { videoPath: session.videoPath });
      if (status.position !== null) {
        session.lastReal = status.position;
      }
      if (status.running) {
        if (status.position !== null) {
          await saveProgress(session.videoId, status.position);
        }
      } else {
        // PotPlayer 窗口已关闭：优先用真实位置（含 ini 记忆值），否则按播放时长估算一次后停止
        finish(session, status.position ?? session.lastReal ?? estimatedPosition(session));
      }
    } catch {
      // 单次查询失败时保持轮询，等待下次结果
    }
  }, [saveProgress, finish]);

  const launch = useCallback(async (video: Video, seek: number | null) => {
    stop();
    const startPos = seek ?? 0;
    sessionRef.current = {
      videoId: video.id,
      videoPath: video.path,
      startPos,
      startedAt: Date.now(),
      lastReal: null,
    };
    try {
      await invoke("launch_potplayer", { videoPath: video.path, seek });
      setError(null);
      await saveProgress(video.id, startPos);
      timerRef.current = setInterval(poll, POLL_INTERVAL_MS);
    } catch (e) {
      sessionRef.current = null;
      setError(String(e));
    }
  }, [stop, poll, saveProgress]);

  useEffect(() => {
    return () => {
      const session = sessionRef.current;
      if (session) {
        const finalPos = session.lastReal ?? estimatedPosition(session);
        if (finalPos > 0) {
          void saveProgress(session.videoId, finalPos);
        }
      }
      stop();
    };
  }, [saveProgress, stop]);

  const clearError = useCallback(() => setError(null), []);

  return { launch, error, clearError };
}
