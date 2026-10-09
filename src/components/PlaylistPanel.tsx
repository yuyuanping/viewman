import { useEffect, useRef } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { logger } from "../logger";
import type { Video } from "../types";
import { copyToClipboard, formatTime } from "../utils";
import { ContextMenu, useContextMenu } from "./ContextMenu";

interface PlaylistPanelProps {
  playlist: Video[];
  currentId: string;
  progress?: Record<string, number | null>;
  onSelect: (video: Video) => void;
  onDelete?: (video: Video) => void;
  onMove?: (video: Video) => void;
  /** 非阻塞错误回报（父层转 toast）；不给则只记日志 */
  onError?: (message: string) => void;
}

export function PlaylistPanel({ playlist, currentId, progress, onSelect, onDelete, onMove, onError }: PlaylistPanelProps) {
  const currentItemRef = useRef<HTMLButtonElement>(null);
  const { menu, openMenu, closeMenu } = useContextMenu<Video>();

  const fail = (action: string, err: unknown) => {
    logger.warn("PlaylistPanel", `${action}失败:`, err);
    onError?.(`${action}失败：${String(err)}`);
  };

  // 打开/切换时把当前播放项滚动到可视区中部
  useEffect(() => {
    currentItemRef.current?.scrollIntoView({ block: "center" });
  }, [currentId]);

  if (playlist.length === 0) return null;

  return (
    <aside className="player-playlist shrink-0 overflow-y-auto border-l border-white/10 bg-black/45 py-14" aria-label="视频列表">
      {playlist.map(item => {
        const isCurrent = item.id === currentId;
        const pos = progress?.[item.id] ?? null;
        return (
          <div key={item.id} className="playlist-row group relative"
            onContextMenu={(e) => openMenu(e, item)}
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
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          label={`${menu.target.filename} 的操作菜单`}
          items={[
            {
              key: "reveal",
              label: "打开文件所在位置",
              onSelect: () => {
                const v = menu.target;
                void revealItemInDir(v.path).catch(err => fail("打开文件所在位置", err));
              },
            },
            {
              key: "copy",
              label: "复制文件名",
              onSelect: () => {
                const v = menu.target;
                void copyToClipboard(v.filename).catch(err => fail("复制文件名", err));
              },
            },
            ...(onMove
              ? [{
                  key: "move",
                  label: "移动到…",
                  onSelect: () => { const v = menu.target; onMove(v); },
                }]
              : []),
            ...(onDelete
              ? [{
                  key: "delete",
                  label: "删除到回收站",
                  onSelect: () => { const v = menu.target; onDelete(v); },
                }]
              : []),
          ]}
        />
      )}
    </aside>
  );
}
