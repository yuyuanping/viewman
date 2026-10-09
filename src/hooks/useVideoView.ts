import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { VideoViewPayload } from "../api";

/** 服务端视频视图分页：目录/搜索下推 SQL + LIMIT/OFFSET，大库翻页不再整表过桥。
 * 排序仍在前端（中文 Collator），本 hook 只负责取页与总数；小库调用方可继续用全量。
 */
export const VIDEO_VIEW_PAGE = 2000;

export function useVideoView(dir: string | null, search: string) {
  const [page, setPage] = useState(0);
  const [view, setView] = useState<VideoViewPayload | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cancelled = useRef(false);

  const load = useCallback(async (nextPage: number) => {
    cancelled.current = false;
    setLoading(true);
    setError(null);
    try {
      const payload = await api.getVideoView(dir, search, VIDEO_VIEW_PAGE, nextPage * VIDEO_VIEW_PAGE);
      if (cancelled.current) return;
      setView(payload);
      setPage(nextPage);
    } catch (e) {
      if (!cancelled.current) setError(String(e));
    } finally {
      if (!cancelled.current) setLoading(false);
    }
  }, [dir, search]);

  useEffect(() => {
    void load(0);
    return () => {
      cancelled.current = true;
    };
  }, [load]);

  return { view, page, loading, error, reload: () => load(page), goto: load };
}
