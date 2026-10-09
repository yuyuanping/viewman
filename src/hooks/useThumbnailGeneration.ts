import { useCallback, useEffect, useState } from "react";
import type { ThumbnailProgressPayload } from "../api";
import { logger } from "../logger";
import { useTauriEvent } from "./useTauriEvent";
import type { Notify } from "./useToasts";

interface ThumbnailOptions {
  /** 后端批量生成命令，视频与图片各自一张表 */
  generate: (ids: string[]) => Promise<number>;
  /** 启动续跑：后端登记孤儿封面并接着上次中断的批次，返回是否做了恢复。必须传稳定引用（api 上的方法） */
  resume: () => Promise<boolean>;
  event: string;
  /** 提示文案里的媒体名称 */
  unit: string;
}

/**
 * 封面生成：订阅进度事件 + 一键为全部缺封面的条目生成。
 * 待办 id 由调用方给出——视频侧从全量清单里筛，图片侧直接问后端
 * （库不再整表下发，前端没有可筛的清单）。
 */
export function useThumbnailGeneration(
  getPendingIds: () => Promise<string[]>,
  reload: () => Promise<void>,
  notify: Notify,
  { generate, resume, event, unit }: ThumbnailOptions,
) {
  const [thumbProgress, setThumbProgress] = useState<{ processed: number; total: number } | null>(null);

  // 与 useScanProgress 同一条订阅通路：useTauriEvent 统一处理竞态与清理，不再手写 listen
  useTauriEvent<ThumbnailProgressPayload>(
    event,
    (payload) => setThumbProgress(payload.done ? null : { processed: payload.processed, total: payload.total }),
    (e) => logger.warn("thumbnail", `事件订阅失败 (${event}):`, e),
  );

  // 启动续跑：关程序时没跑完的批次，下次打开接着生成。进度走同一个事件通道，
  // 跑完刷新一次列表；失败记 warn 不打扰启动流程
  useEffect(() => {
    let cancelled = false;
    resume().then(didResume => {
      if (cancelled || !didResume) return;
      notify(`检测到上次未完成的${unit}封面生成，已自动继续。`);
      return reload();
    }).catch((e) => logger.warn("thumbnail", `续跑失败 (${unit}):`, e));
    return () => { cancelled = true; };
  }, [resume, reload, notify, unit]);

  const generateAll = useCallback(async () => {
    const pending = await getPendingIds();
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
  }, [getPendingIds, reload, notify, generate, unit]);

  return { thumbProgress, generating: thumbProgress !== null, generateAll };
}
