import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { ThumbnailProgressPayload } from "../api";
import type { Video } from "../types";
import type { Notify } from "./useToasts";

/** 封面生成：订阅 thumbnail-progress 事件 + 一键为全部缺封面的视频生成 */
export function useThumbnailGeneration(videos: Video[], loadVideos: () => Promise<void>, notify: Notify) {
  const [thumbProgress, setThumbProgress] = useState<{ processed: number; total: number } | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<ThumbnailProgressPayload>("thumbnail-progress", (event) => {
      const { processed, total, done } = event.payload;
      setThumbProgress(done ? null : { processed, total });
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 事件监听失败不影响手动刷新 */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const generateAll = useCallback(async () => {
    const pending = videos.filter(v => !v.thumbnail_path).map(v => v.id);
    if (pending.length === 0) {
      notify("所有视频都已有封面。");
      return;
    }
    setThumbProgress({ processed: 0, total: pending.length });
    try {
      const generated = await api.generateThumbnails(pending);
      await loadVideos();
      notify(generated > 0
        ? `已生成 ${generated} 个封面。`
        : "没有生成新封面：可能缺少 ffmpeg，或视频抽帧失败。");
    } catch (e) {
      notify(`生成封面失败：${String(e)}`, "error");
    } finally {
      setThumbProgress(null);
    }
  }, [videos, loadVideos, notify]);

  return { thumbProgress, generating: thumbProgress !== null, generateAll };
}
