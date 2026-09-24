import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ThumbnailProgressPayload } from "../api";
import type { Notify } from "./useToasts";

interface ThumbnailItem {
  id: string;
  thumbnail_path?: string | null;
}

interface ThumbnailOptions {
  /** 后端批量生成命令，视频与图片各自一张表 */
  generate: (ids: string[]) => Promise<number>;
  event: string;
  /** 提示文案里的媒体名称 */
  unit: string;
}

/** 封面生成：订阅进度事件 + 一键为全部缺封面的条目生成 */
export function useThumbnailGeneration<T extends ThumbnailItem>(
  items: T[],
  reload: () => Promise<void>,
  notify: Notify,
  { generate, event, unit }: ThumbnailOptions,
) {
  const [thumbProgress, setThumbProgress] = useState<{ processed: number; total: number } | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<ThumbnailProgressPayload>(event, (received) => {
      const { processed, total, done } = received.payload;
      setThumbProgress(done ? null : { processed, total });
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 事件监听失败不影响手动刷新 */ });
    return () => { disposed = true; unlisten?.(); };
  }, [event]);

  const generateAll = useCallback(async () => {
    const pending = items.filter(item => !item.thumbnail_path).map(item => item.id);
    if (pending.length === 0) {
      notify(`所有${unit}都已有封面。`);
      return;
    }
    setThumbProgress({ processed: 0, total: pending.length });
    try {
      const generated = await generate(pending);
      await reload();
      notify(generated > 0
        ? `已生成 ${generated} 个封面。`
        : "没有生成新封面：可能缺少 ffmpeg，或抽帧失败。");
    } catch (e) {
      notify(`生成封面失败：${String(e)}`, "error");
    } finally {
      setThumbProgress(null);
    }
  }, [items, reload, notify, generate, unit]);

  return { thumbProgress, generating: thumbProgress !== null, generateAll };
}
