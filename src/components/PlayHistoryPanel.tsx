import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { RecentlyPlayed } from "../types";
import { formatDuration, formatTime } from "../utils";
import { FINISHED_RATIO } from "../libraryFilter";

interface PlayHistoryPanelProps {
  onClose: () => void;
  onPlay: (videoId: string, position: number) => void;
}

/** SQLite datetime('now') 产出 "YYYY-MM-DD HH:MM:SS"（UTC）→ 本地 Date */
function parseUtc(dateStr: string): Date {
  const iso = dateStr.replace(" ", "T");
  return new Date(iso.endsWith("Z") ? iso : `${iso}Z`);
}

/** 历史面板标题下的日期；今天/昨天友好化 */
function dayLabel(d: Date): string {
  const today = new Date();
  const diffDays = Math.floor((new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime()
    - new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()) / 86400000);
  if (diffDays === 0) return "今天";
  if (diffDays === 1) return "昨天";
  if (diffDays < 7) return `${diffDays} 天前`;
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** 完整播放历史：全表按天分组，条目带进度与看完状态，点击续播 */
export function PlayHistoryPanel({ onClose, onPlay }: PlayHistoryPanelProps) {
  const [history, setHistory] = useState<RecentlyPlayed[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    api.getPlayHistory()
      .then(items => { if (!disposed) setHistory(items); })
      .catch(e => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // 按本地日期分组，保持后端给的倒序
  const groups = useMemo(() => {
    if (!history) return [];
    const map = new Map<string, { day: string; items: RecentlyPlayed[] }>();
    for (const item of history) {
      const d = parseUtc(item.updated_at);
      if (Number.isNaN(d.getTime())) continue;
      const key = dayLabel(d);
      const bucket = map.get(key) ?? { day: key, items: [] };
      bucket.items.push(item);
      map.set(key, bucket);
    }
    return [...map.values()];
  }, [history]);

  return (
    <div className="image-viewer fixed inset-0 z-40 flex flex-col" role="dialog" aria-modal="true" aria-label="播放历史">
      <div className="flex items-center gap-3 px-4 py-3 shrink-0">
        <h2 className="text-sm text-gray-200 font-medium">播放历史</h2>
        <span className="text-xs text-gray-400 tabular-nums">{history ? `${history.length} 条` : "…"}</span>
        <span className="toolbar-spacer" />
        <button type="button" className="toolbar-chip" onClick={onClose} title="关闭（Esc）" aria-label="关闭播放历史">
          ✕
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-4 pb-6">
        {error && <p className="text-red-300 text-sm p-4">读取播放历史失败：{error}</p>}
        {!error && history === null && <p className="text-gray-400 text-sm p-4">加载中…</p>}
        {!error && history !== null && history.length === 0 && (
          <p className="text-gray-500 text-sm p-4">还没有播放记录。播放过的视频会出现在这里。</p>
        )}
        {groups.map(group => (
          <div key={group.day} className="mb-5">
            <p className="text-xs text-gray-400 font-medium mb-2 sticky top-0 py-1 bg-[#04070de6]">{group.day}</p>
            <ul className="flex flex-col gap-1">
              {group.items.map(item => {
                const finished = item.video.duration != null && item.video.duration > 0
                  && item.position / item.video.duration >= FINISHED_RATIO;
                const pct = item.video.duration && item.video.duration > 0
                  ? Math.min(100, Math.max(0, (item.position / item.video.duration) * 100)) : 0;
                return (
                  <li key={item.video.id}>
                    <button
                      type="button"
                      onClick={() => onPlay(item.video.id, item.position)}
                      className="w-full text-left px-3 py-2 rounded-lg hover:bg-white/5 transition flex items-center gap-3"
                      title={`${item.video.filename}\n播放到 ${formatTime(item.position)}`}
                    >
                      <span className="text-[10px] text-gray-500 shrink-0 tabular-nums w-10">
                        {parseUtc(item.updated_at).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block text-[13px] text-gray-200 truncate">{item.video.filename}</span>
                        <span className="block h-0.5 mt-1 rounded bg-white/10 overflow-hidden">
                          <span
                            className={finished ? "block h-full bg-emerald-500" : "block h-full bg-blue-500"}
                            style={{ width: `${pct}%` }}
                          />
                        </span>
                      </span>
                      <span className={`text-[11px] shrink-0 tabular-nums ${finished ? "text-emerald-400" : "text-gray-400"}`}>
                        {finished ? "已看完" : `看到 ${formatDuration(item.video.duration) === "--:--" ? "--:--" : formatTime(item.position)}`}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>
    </div>
  );
}
