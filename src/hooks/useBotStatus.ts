import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { BotStatus } from "../types";
import { dlSnapshotChanged } from "../botDlSnapshot";

/** 全量轮询间隔：进程/端口探测（PowerShell 冷启动约 1 秒）不宜更快 */
const POLL_MS = 10000;
/** 轻量快照间隔：下载活跃期只读 bot.db（毫秒级），列表近实时 */
const FAST_MS = 2000;

/**
 * 常驻轮询 QQ Bot 状态。挂在 App 上而非面板里，侧栏 tab 徽标在任何标签页下都保持实时，
 * 面板切进来时直接读缓存，不用等第一次请求。
 *
 * 下载列表要近实时，但全量状态里的进程探测（PowerShell）与端口探测（netstat）扛不住
 * 高频，所以分两档：有媒体在排队/下载（pending+busy>0）时每 2 秒拉一次轻量快照
 * （只读 bot.db 的积压计数、汇总与当日列表），否则回到 10 秒全量轮询。
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

  // 下载活跃期（有 pending/busy 媒体）切 2 秒轻量快照；快照失败静默，
  // 下一个全量轮询会带出 db_note/错误。依赖 active：空闲↔活跃切换时重排定时器
  const active = status != null && status.dl.busy + status.dl.pending > 0;
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(async () => {
      try {
        const snap = await api.getBotDlSnapshot();
        if (!mounted.current) return;
        // 队列积压没变就不换引用：status 是 App 的 state，每 2 秒换新对象会把
        // 侧栏、工具栏、视频网格整棵树重渲染一遍（用户在别的标签页也躲不掉）
        setStatus((prev) =>
          prev && dlSnapshotChanged(prev, snap) ? { ...prev, dl: snap.dl, summary: snap.summary, today_items: snap.today_items } : prev,
        );
      } catch {
        // 轻量快照失败不打扰用户（全量轮询负责报错）
      }
    }, FAST_MS);
    return () => clearInterval(timer);
  }, [active]);

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
