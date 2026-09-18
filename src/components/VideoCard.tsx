import { useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import type { Video } from "../types";
import { formatDuration, formatFileSize } from "../utils";

interface VideoCardProps {
  video: Video;
  progress: number | null;
  missing?: boolean;
  onPlay: (video: Video) => void;
  onDeleted: () => void;
}

export function VideoCard({ video, progress, missing = false, onPlay, onDeleted }: VideoCardProps) {
  const [deleting, setDeleting] = useState(false);
  const progressPct = progress !== null && video.duration && video.duration > 0
    ? Math.max(0, Math.min(100, (progress / video.duration) * 100)) : 0;
  const extension = video.filename.split(".").pop()?.toUpperCase() || "VIDEO";
  const thumbnailPath = video.thumbnail_path ?? null;

  const handleDelete = async () => {
    if (!confirm(`确定要删除 "${video.filename}" 到回收站？`)) return;
    setDeleting(true);
    try {
      await invoke("delete_video", { videoId: video.id });
      onDeleted();
    } catch (err) {
      alert("删除失败: " + err);
      setDeleting(false);
    }
  };

  return (
    <article className={`media-card${missing ? " media-missing" : ""}`} title={missing ? "文件当前不可读取（可能已被移动、删除或磁盘未连接）" : undefined}>
      <button
        onClick={() => onPlay(video)}
        disabled={deleting}
        className="block w-full text-left"
        aria-label={`播放 ${video.filename}`}
      >
        <div className="media-preview">
          {missing && <span className="missing-flag">文件不可读</span>}
          {/* 缩略图 */}
          {thumbnailPath ? (
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
      <button
        onClick={handleDelete}
        disabled={deleting}
        className="media-remove absolute top-2 right-2 bg-gray-950/80 hover:bg-red-700 text-gray-300 rounded-lg w-7 h-7 grid place-items-center text-sm"
        title="删除到回收站"
        aria-label={`删除 ${video.filename} 到回收站`}
      >
        {deleting ? "…" : "×"}
      </button>
    </article>
  );
}

