import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { BotStatus } from "../types";
import { dlSnapshotChanged } from "../botDlSnapshot";
import { useTauriEvent } from "./useTauriEvent";
/** 全量轮询间隔：进程/端口探测（PowerShell 冷启动约 1 秒）不宜更快 */
const POLL_MS = 10000;
/** 事件防抖：bot 落库很碎（明细行预登记/进度/终态连着来），200ms 窗口合并成一次拉取 */
const EVENT_DEBOUNCE_MS = 200;
/**
* 常驻轮询 QQ Bot 状态。挂在 App 上而非面板里，侧栏 tab 徽标在任何标签页下都保持实时，
* 面板切进来时直接读缓存，不用等第一次请求。
*
* 下载列表的实时性走推送：bot 每次落库，Rust 侧监视 bot.db 防抖后发 bot-dl-changed，
* 这里合并 200ms 再拉一次轻量快照（只读 bot.db，毫秒级）。10 秒全量轮询保留作兜底，
* 负责进程/端口状态与 db_note——这些探测撑不起高频，也不需要。
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
  // bot-dl-changed 推送 -> 200ms 防抖合并 -> 轻量快照。窗口内的连续事件只排一次
  // 拉取（不延长窗口）；status 是 App 的 state，只有快照数字真变了才换引用，
  // 免得侧栏、工具栏、视频网格整棵树跟着空转重渲染
  const debounceRef = useRef<number | null>(null);
  useTauriEvent("bot-dl-changed", () => {
    if (!mounted.current || debounceRef.current != null) return;
    debounceRef.current = window.setTimeout(async () => {
      debounceRef.current = null;
      try {
        const snap = await api.getBotDlSnapshot();
        if (!mounted.current) return;
        setStatus((prev) =>
          prev && dlSnapshotChanged(prev, snap)
            ? { ...prev, dl: snap.dl, summary: snap.summary, today_items: snap.today_items }
            : prev,
        );
      } catch {
        // 轻量快照失败不打扰用户（全量轮询负责报错）
      }
    }, EVENT_DEBOUNCE_MS);
  });
  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => {
      mounted.current = false;
      clearInterval(timer);
      if (debounceRef.current != null) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
    };
  }, [refresh]);
  return { status, error, refresh };
}
