import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import type { Video } from "../types";
import { formatDuration, formatFileSize } from "../utils";
import { MoveTargetsDialog } from "./MoveTargetsDialog";

interface VideoCardProps {
  video: Video;
  progress: number | null;
  missing?: boolean;
  fake?: boolean;
  short?: boolean;
  duplicate?: boolean;
  selectMode?: boolean;
  selected?: boolean;
  onToggleSelect?: (shiftKey: boolean) => void;
  onPlay: (video: Video) => void;
  onDeleted: (videoId: string) => void;
  onMoved: (videoId: string, newPath: string) => void;
}

export function VideoCard({ video, progress, missing = false, fake = false, short = false, duplicate = false, selectMode = false, selected = false, onToggleSelect, onPlay, onDeleted, onMoved }: VideoCardProps) {
  const [deleting, setDeleting] = useState(false);
  const [moving, setMoving] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [movePromptOpen, setMovePromptOpen] = useState(false);

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

  const openContainingFolder = async () => {
    try {
      await revealItemInDir(video.path);
    } catch (err) {
      alert("打开文件所在位置失败: " + err);
    }
  };
  const progressPct = progress !== null && video.duration && video.duration > 0
    ? Math.max(0, Math.min(100, (progress / video.duration) * 100)) : 0;
  const extension = video.filename.split(".").pop()?.toUpperCase() || "VIDEO";
  const thumbnailPath = video.thumbnail_path ?? null;

  const handleDelete = async () => {
    if (!confirm(`确定要删除 "${video.filename}" 到回收站？`)) return;
    setDeleting(true);
    try {
      const deleted = await api.deleteVideos([video.id]);
      if (deleted.length > 0) onDeleted(video.id);
    } catch (err) {
      alert("删除失败: " + err);
      setDeleting(false);
    }
  };

  const handleMove = () => {
    setMenu(null);
    setMovePromptOpen(true);
  };

  return (
    <article
      className={`media-card${missing ? " media-missing" : ""}${fake ? " media-fake" : ""}${selected ? " media-selected" : ""}`}
      title={fake ? "文件内容实为图片，并非视频（可通过工具栏转换为图片文件）" : missing ? "文件当前不可读取（可能已被移动、删除或磁盘未连接）" : duplicate ? "与库内其他文件内容相同（多余副本，可通过工具栏删除）" : undefined}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
    >
      <button
        onClick={selectMode ? (e) => onToggleSelect?.(e.shiftKey) : (fake ? undefined : () => onPlay(video))}
        disabled={deleting || (!selectMode && fake)}
        className="block w-full text-left"
        aria-label={selectMode ? (selected ? `取消选择 ${video.filename}` : `选择 ${video.filename}`) : fake ? `${video.filename}（图片，无法播放）` : `播放 ${video.filename}`}
      >
        <div className="media-preview">
          {missing && <span className="missing-flag">文件不可读</span>}
          {fake && <span className="fake-flag">实为图片</span>}
          {duplicate && !fake && <span className="dup-flag">重复副本</span>}
          {selectMode && (
            <span className={`select-check${selected ? " select-checked" : ""}`} aria-hidden="true">
              {selected ? "✓" : ""}
            </span>
          )}
          {/* 封面：假视频直接显示图片本体，其余用抽帧缓存 */}
          {fake ? (
            <img
              src={convertFileSrc(video.path)}
              alt={video.filename}
              loading="lazy"
              className="media-thumbnail"
            />
          ) : thumbnailPath ? (
            <img
              src={convertFileSrc(thumbnailPath)}
              alt={video.filename}
              loading="lazy"
              className="media-thumbnail"
            />
          ) : (
            <>
              <span className="media-extension absolute top-3 left-3 text-[10px] font-semibold tracking-widest text-gray-400/90">
                {extension}
              </span>
              <span className="media-play">
                <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                  <path d="M9 5v14l11-7z" />
                </svg>
              </span>
            </>
          )}
          <span className="media-badge bottom-3 right-3">{formatDuration(video.duration)}</span>
          {short && !fake && <span className="media-badge bottom-3 left-3 short-badge">{video.duration !== null && video.duration < 1 ? "0 秒 · 可转图片" : "静图 · 可转图片"}</span>}
          {progressPct > 0 && (
            <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-white/10">
              <div className="h-full bg-blue-500" style={{ width: `${progressPct}%` }} />
            </div>
          )}
        </div>
        <div className="p-3.5">
          <p className="text-[13px] leading-5 line-clamp-2 min-h-10 text-gray-200 break-all" title={video.filename}>
            {video.filename}
          </p>
          <div className="flex justify-between text-[11px] text-gray-400 mt-3 gap-2 tabular-nums">
            <span>{video.height ? `${video.height}p` : "本地视频"} · {formatFileSize(video.file_size)}</span>
            {progress !== null && progress > 0 && <span className="text-blue-400 shrink-0">继续观看</span>}
          </div>
        </div>
      </button>
      {!selectMode && (
        <button
          onClick={handleDelete}
          disabled={deleting}
          className="media-remove absolute top-2 right-2 bg-gray-950/80 hover:bg-red-700 text-gray-300 rounded-lg w-7 h-7 grid place-items-center text-sm"
          title="删除到回收站"
          aria-label={`删除 ${video.filename} 到回收站`}
        >
          {deleting ? "…" : "×"}
        </button>
      )}
      {movePromptOpen && (
        <MoveTargetsDialog
          kind="video"
          noun="视频"
          count={1}
          onPick={async (dir) => {
            if (!confirm(`将 "${video.filename}" 移动到:\n${dir}`)) return false;
            setMoving(true);
            try {
              const newPath = await api.moveVideo(video.id, dir);
              onMoved(video.id, newPath);
              return true;
            } catch (err) {
              alert("移动失败: " + err);
              return false;
            } finally {
              setMoving(false);
            }
          }}
          onClose={() => setMovePromptOpen(false)}
        />
      )}
      {menu && (
        <div
          className="media-context-menu"
          style={{ left: menu.x, top: menu.y }}
          onClick={(e) => e.stopPropagation()}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button type="button" onClick={() => { setMenu(null); void openContainingFolder(); }}>
            打开文件所在位置
          </button>
          <button type="button" disabled={moving} onClick={() => { setMenu(null); void handleMove(); }}>
            {moving ? "移动中…" : "移动到…"}
          </button>
        </div>
      )}
    </article>
  );
}

