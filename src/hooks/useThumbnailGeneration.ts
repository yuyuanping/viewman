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
  /** 启动续跑：后端登记孤儿封面并接着上次中断的批次，返回是否做了恢复。必须传稳定引用（api 上的方法） */
  resume: () => Promise<boolean>;
  event: string;
  /** 提示文案里的媒体名称 */
  unit: string;
}

/** 封面生成：订阅进度事件 + 一键为全部缺封面的条目生成 */
export function useThumbnailGeneration<T extends ThumbnailItem>(
  items: T[],
  reload: () => Promise<void>,
  notify: Notify,
  { generate, resume, event, unit }: ThumbnailOptions,
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

  // 启动续跑：关程序时没跑完的批次，下次打开接着生成。进度走同一个事件通道，
  // 跑完刷新一次列表；失败只静默放弃，不打扰启动流程
  useEffect(() => {
    let cancelled = false;
    resume().then(didResume => {
      if (cancelled || !didResume) return;
      notify(`检测到上次未完成的${unit}封面生成，已自动继续。`);
      return reload();
    }).catch(() => { /* 续跑失败不影响启动 */ });
    return () => { cancelled = true; };
  }, [resume, reload, notify, unit]);

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
