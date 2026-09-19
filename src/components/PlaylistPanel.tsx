import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { Video } from "../types";
import { formatTime } from "../utils";

interface PlaylistPanelProps {
  playlist: Video[];
  currentId: string;
  progress?: Record<string, number | null>;
  onSelect: (video: Video) => void;
  onDelete?: (video: Video) => void;
  onMove?: (video: Video) => void;
}

export function PlaylistPanel({ playlist, currentId, progress, onSelect, onDelete, onMove }: PlaylistPanelProps) {
  const currentItemRef = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; video: Video } | null>(null);

  // 打开/切换时把当前播放项滚动到可视区中部
  useEffect(() => {
    currentItemRef.current?.scrollIntoView({ block: "center" });
  }, [currentId]);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  if (playlist.length === 0) return null;

  return (
    <aside className="player-playlist shrink-0 overflow-y-auto border-l border-white/10 bg-black/45 py-14" aria-label="视频列表">
      {playlist.map(item => {
        const isCurrent = item.id === currentId;
        const pos = progress?.[item.id] ?? null;
        return (
          <div key={item.id} className="playlist-row group relative"
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, video: item });
            }}
          >
            <button
              ref={isCurrent ? currentItemRef : undefined}
              onClick={() => !isCurrent && onSelect(item)}
              className={`playlist-item w-full flex items-center gap-2.5 px-3 py-2 text-left transition ${
                isCurrent ? "playlist-item-current bg-blue-600/25" : "hover:bg-white/10"
              }`}
              title={item.filename}
            >
              <span className="playlist-thumb shrink-0 w-16 h-10 rounded bg-gray-800 overflow-hidden grid place-items-center text-[9px] text-gray-400">
                {item.thumbnail_path ? (
                  <img src={convertFileSrc(item.thumbnail_path)} alt="" loading="lazy" className="w-full h-full object-cover" />
                ) : (
                  (item.filename.split(".").pop() ?? "").toUpperCase()
                )}
              </span>
              <span className="min-w-0 flex-1">
                <span className={`block truncate text-xs ${isCurrent ? "text-white" : "text-gray-300"}`}>{item.filename}</span>
                <span className="block text-[10px] text-gray-400 tabular-nums">
                  {formatTime(item.duration ?? 0)}{pos !== null && pos > 0 ? ` · 看到 ${formatTime(pos)}` : ""}
                </span>
              </span>
            </button>
            {onDelete && (
              <button
                onClick={() => onDelete(item)}
                className="playlist-remove absolute top-1/2 -translate-y-1/2 right-1.5 w-6 h-6 rounded-md bg-gray-950/80 hover:bg-red-700 text-gray-300 hover:text-white grid place-items-center text-xs opacity-0 focus:opacity-100 transition group-hover:opacity-100"
                title="删除到回收站"
                aria-label={`删除 ${item.filename}`}
              >
                ✕
              </button>
            )}
          </div>
        );
      })}
      {menu && (
        <div
          className="media-context-menu"
          style={{ left: menu.x, top: menu.y }}
          onClick={(e) => e.stopPropagation()}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button type="button" onClick={() => {
            setMenu(null);
            void revealItemInDir(menu.video.path).catch(err => alert("打开文件所在位置失败: " + err));
          }}>
            打开文件所在位置
          </button>
          {onMove && (
            <button type="button" onClick={() => { const v = menu.video; setMenu(null); onMove(v); }}>
              移动到…
            </button>
          )}
        </div>
      )}
    </aside>
  );
}
