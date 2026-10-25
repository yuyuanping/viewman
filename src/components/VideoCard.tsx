import { memo, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { logger } from "../logger";
import type { Video } from "../types";
import { copyToClipboard, formatDuration, formatFileSize } from "../utils";
import { ContextMenu, useContextMenu } from "./ContextMenu";
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

export const VideoCard = memo(function VideoCard({ video, progress, missing = false, fake = false, short = false, duplicate = false, selectMode = false, selected = false, onToggleSelect, onPlay, onDeleted, onMoved }: VideoCardProps) {
  const [deleting, setDeleting] = useState(false);
  const [moving, setMoving] = useState(false);
  const { menu, openMenu, closeMenu } = useContextMenu<void>();
  const [movePromptOpen, setMovePromptOpen] = useState(false);
  // 卡片级操作失败不再 alert 阻塞：行内展示，可关闭
  const [actionError, setActionError] = useState<string | null>(null);

  const openContainingFolder = async () => {
    try {
      await revealItemInDir(video.path);
    } catch (err) {
      logger.warn("VideoCard", "打开所在位置失败:", err);
      setActionError(`打开文件所在位置失败：${String(err)}`);
    }
  };
  const copyFilename = async () => {
    try {
      await copyToClipboard(video.filename);
    } catch (err) {
      logger.warn("VideoCard", "复制文件名失败:", err);
      setActionError(`复制文件名失败：${String(err)}`);
    }
  };
  const progressPct = progress !== null && video.duration && video.duration > 0
    ? Math.max(0, Math.min(100, (progress / video.duration) * 100)) : 0;
  const extension = video.filename.split(".").pop()?.toUpperCase() || "VIDEO";
  // 封面缓存文件在运行中没了（手动清缓存、缓存盘拔掉）：加载失败就当没封面，退回
  // 扩展名占位，不剩一个 404 的破图。记路径而不是布尔量——重新生成出新的缓存路径就自动走回
  const [deadThumb, setDeadThumb] = useState<string | null>(null);
  const thumbnailPath = video.thumbnail_path && video.thumbnail_path !== deadThumb ? video.thumbnail_path : null;

  const handleDelete = async () => {
    setDeleting(true);
    setActionError(null);
    try {
      const report = await api.deleteVideos([video.id]);
      if (report.locked.length > 0) {
        setActionError("删除失败：文件被占用，关闭占用它的程序后重试。");
        setDeleting(false);
        return;
      }
      onDeleted(video.id);
    } catch (err) {
      logger.warn("VideoCard", "删除失败:", err);
      setActionError(`删除失败：${String(err)}`);
      setDeleting(false);
    }
  };

  const handleMove = () => {
    closeMenu();
    setMovePromptOpen(true);
  };

  /** 右键菜单：与图片卡片/播放列表同一套（打开所在位置 / 复制文件名 / 移动到… / 删除到回收站）。
   *  只在菜单打开时拼，网格里几十张卡每帧建一遍数组不值当 */
  const menuItems = useMemo(() => menu ? [
    { key: "reveal", label: "打开文件所在位置", onSelect: () => { void openContainingFolder(); } },
    { key: "copy", label: "复制文件名", onSelect: () => { void copyFilename(); } },
    { key: "move", label: moving ? "移动中…" : "移动到…", disabled: moving, onSelect: handleMove },
    { key: "delete", label: deleting ? "删除中…" : "删除到回收站", disabled: deleting, onSelect: () => { void handleDelete(); } },
  ] : [], [menu, video, moving, deleting]);

  return (
    <article
      className={`media-card${missing ? " media-missing" : ""}${fake ? " media-fake" : ""}${selected ? " media-selected" : ""}`}
      title={fake ? "文件内容实为图片，并非视频（可通过工具栏转换为图片文件）" : missing ? "文件当前不可读取（可能已被移动、删除或磁盘未连接）" : duplicate ? "与库内其他文件内容相同（多余副本，可通过工具栏删除）" : undefined}
      onContextMenu={(e) => openMenu(e, undefined)}
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
              onError={() => setDeadThumb(thumbnailPath)}
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
            <div className="absolute bottom-0 left-0 right-0 h-[3px] bg-black/45">
              <div className="h-full bg-gradient-to-r from-blue-500 to-indigo-300" style={{ width: `${progressPct}%` }} />
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
            setMoving(true);
            setActionError(null);
            try {
              const newPath = await api.moveVideo(video.id, dir);
              onMoved(video.id, newPath);
              return true;
            } catch (err) {
              logger.warn("VideoCard", "移动失败:", err);
              setActionError(`移动失败：${String(err)}`);
              return false;
            } finally {
              setMoving(false);
            }
          }}
          onClose={() => setMovePromptOpen(false)}
        />
      )}
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={closeMenu} label={`${video.filename} 的操作菜单`} />
      )}
      {actionError && (
        <div className="mx-3 mb-3 rounded-lg bg-red-950/90 px-2 py-1.5 text-[11px] leading-4 text-red-200 flex justify-between gap-2">
          <span className="break-all">{actionError}</span>
          <button onClick={() => setActionError(null)} aria-label="关闭错误" className="shrink-0 hover:text-white">✕</button>
        </div>
      )}
    </article>
  );
});

