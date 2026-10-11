import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { BotStatus } from "../types";

/** 轮询间隔：NapCat 冷启动与下载都不是秒级变化，10 秒足够且不打扰后端 */
const POLL_MS = 10000;

/**
 * 常驻轮询 QQ Bot 状态。挂在 App 上而非面板里，侧栏 tab 徽标在任何标签页下都保持实时，
 * 面板切进来时直接读缓存，不用等第一次请求。
 */
export function useBotStatus() {
  const [status, setStatus] = useState<BotStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const snap = await api.getBotStatus();
      if (!mounted.current) return;
      setStatus(snap);
      setError(null);
    } catch (e) {
      if (!mounted.current) return;
      // 后端没起来时 get_bot_status 也会失败，此时保留上一份快照，只提示错误
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => {
      mounted.current = false;
      clearInterval(timer);
    };
  }, [refresh]);

  return { status, error, refresh };
}
