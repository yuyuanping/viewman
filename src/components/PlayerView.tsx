import { useRef, useEffect, useCallback, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Video, VideoFileStatus } from "../types";
import { formatTime } from "../utils";

interface PlayerViewProps {
  video: Video;
  initialPosition: number;
  onClose: () => void;
  onProgress: (videoId: string, position: number) => Promise<void>;
  onFallback?: (video: Video) => void;
}

export function PlayerView({ video, initialPosition, onClose, onProgress, onFallback }: PlayerViewProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const currentTimeRef = useRef(0);
  const [playing, setPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const [progressError, setProgressError] = useState<string | null>(null);
  const [fileReadable, setFileReadable] = useState(false);
  const [showError, setShowError] = useState(false);
  const [rateBadge, setRateBadge] = useState(false);
  const alive = useRef(true);
  const lastSaved = useRef(initialPosition);
  const saving = useRef(false);
  const seeking = useRef(false);
  const resumeAfterSeek = useRef(false);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  const handlePlaybackError = useCallback(async () => {
    setShowError(true);
    setPlaybackError("无法播放，正在检查文件是否可访问……");
    try {
      const result = await invoke<VideoFileStatus>("check_video_file", { videoId: video.id });
      if (!alive.current) return;
      setFileReadable(result.status === "readable");
      setPlaybackError(result.status !== "readable"
        ? result.message || "文件不可用，请检查磁盘连接或重新扫描目录。"
        : result.status === "readable"
          ? "文件可以读取，但内置播放器无法解码；可能是编码不支持或文件损坏。可尝试 PotPlayer。"
          : "读取视频数据失败，请检查磁盘连接后重试。");
    } catch (e) {
      if (alive.current) setPlaybackError(`无法确认文件状态：${String(e)}`);
    }
  }, [video.id]);

  const src = convertFileSrc(video.path);

  useEffect(() => {
    if (!showError) setPlaybackError(null);
  }, [video.id, src, showError]);

  // 元数据就绪后跳到上次进度，接着尝试自动播放
  const handleLoadedMetadata = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;

    if (video.duration && video.duration > 0) {
      setDuration(video.duration);
    } else {
      setDuration(el.duration || 0);
    }

    if (initialPosition > 0 && el.currentTime < initialPosition) {
      el.currentTime = initialPosition;
    }

    el.play().then(() => setPlaying(true)).catch(() => setPlaying(false));
  }, [initialPosition]);

  const handleTimeUpdate = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    currentTimeRef.current = el.currentTime;
    // 拖动过程中不让播放事件回写，避免滑块跳动
    if (seeking.current) return;
    setCurrentTime(el.currentTime);
    if (duration === 0 && el.duration) setDuration(el.duration);
  }, [duration]);

  const handleSave = useCallback(async () => {
    if (saving.current) return;
    saving.current = true;
    try {
      const position = videoRef.current?.currentTime ?? currentTimeRef.current;
      if (Number.isFinite(position) && position >= 0 && position !== lastSaved.current) {
        await onProgress(video.id, position);
        lastSaved.current = position;
      }
      if (alive.current) setProgressError(null);
    } catch (e) {
      if (alive.current) setProgressError(`进度保存失败，稍后重试：${String(e)}`);
    } finally {
      saving.current = false;
    }
  }, [video.id, onProgress]);

  useEffect(() => {
    const interval = setInterval(handleSave, 15000);
    return () => {
      clearInterval(interval);
      handleSave();
    };
  }, [handleSave]);

  const togglePlay = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    if (el.paused) {
      el.play().then(() => setPlaying(true)).catch(() => setPlaying(false));
    } else {
      el.pause();
      setPlaying(false);
    }
  }, []);

  const handleSeekStart = useCallback(() => {
    seeking.current = true;
    // 记住拖动前是否在播放，避免用户暂停后拖动进度条被强制续播
    resumeAfterSeek.current = !videoRef.current?.paused;
  }, []);
  const handleSeek = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const el = videoRef.current;
    if (!el) return;
    const value = parseFloat(e.target.value);
    if (Number.isFinite(value)) {
      el.currentTime = value;
      setCurrentTime(value);
    }
  }, []);
  const handleSeekEnd = useCallback(() => {
    seeking.current = false;
    const el = videoRef.current;
    if (el && resumeAfterSeek.current && el.paused) {
      el.play().then(() => setPlaying(true)).catch(() => setPlaying(false));
    }
    resumeAfterSeek.current = false;
  }, []);

  const RATES = [1, 1.5, 2] as const;
  const [rateIdx, setRateIdx] = useState(0);

  const cycleRate = useCallback(() => {
    const next = (rateIdx + 1) % RATES.length;
    setRateIdx(next);
    setRateBadge(true);
    const el = videoRef.current;
    if (el) {
      el.playbackRate = RATES[next];
    }
    setTimeout(() => setRateBadge(false), 900);
  }, [rateIdx]);

  return (
    <div className="player-overlay fixed inset-0 z-50 flex flex-col">
      <div className="player-stage flex-1 min-h-0 flex items-center justify-center px-4 pt-14 pb-2">
        <video
          ref={videoRef}
          src={src}
          onTimeUpdate={handleTimeUpdate}
          onLoadedMetadata={handleLoadedMetadata}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={handleSave}
          onError={() => { void handlePlaybackError(); }}
          className="player-video max-h-full max-w-full bg-black object-contain"
          controls={false}
          playsInline
        />
      </div>

      <div className="player-top absolute top-0 left-0 right-0 z-10 flex justify-between items-start px-4 py-3 gap-3">
        <div className="player-title min-w-0">
          <span className="truncate text-sm text-white/90 font-medium">{video.filename}</span>
        </div>
        <div className="player-actions flex gap-2 shrink-0">
          <button
            onClick={onClose}
            className="player-btn rounded-lg border border-white/10 bg-white/5 hover:bg-white/10 px-3 py-1.5 text-sm text-white/90 hover:text-white transition"
          >
            关闭
          </button>
        </div>
      </div>

      <div className="player-bar shrink-0 w-full flex flex-col items-center gap-2 px-4 pb-5">
        {showError && playbackError && (
          <div className="player-error w-full max-w-xl rounded-xl border border-red-500/30 bg-red-900/60 px-4 py-3 text-sm text-red-200 flex flex-col gap-3">
            <div className="flex items-start justify-between gap-3">
              <span className="flex-1">{playbackError}</span>
              <button
                onClick={() => setShowError(false)}
                className="shrink-0 text-red-300 hover:text-white transition"
                aria-label="关闭提示"
              >
                ✕
              </button>
            </div>
            {fileReadable && onFallback && (
              <button
                onClick={() => onFallback(video)}
                className="self-start rounded-lg border border-white/10 bg-white/5 hover:bg-white/10 px-3 py-1.5 text-sm text-white/90 hover:text-white transition"
              >
                用 PotPlayer 打开
              </button>
            )}
          </div>
        )}

        {progressError && (
          <p className="player-progress-notice text-amber-300 text-sm">{progressError}</p>
        )}

        <div className="player-controls w-full max-w-3xl flex flex-col gap-3.5">
          <button
            onClick={togglePlay}
            className="player-play inline-flex items-center justify-center w-14 h-14 rounded-full bg-white/10 hover:bg-white/15 text-white text-2xl transition"
            aria-label={playing ? "暂停" : "播放"}
          >
            {playing ? "❚❚" : "▶"}
          </button>

          <div className="player-progress relative w-full h-1.5 rounded-full bg-white/10 cursor-pointer group">
            <input
              type="range"
              min={0}
              max={Math.max(duration, 1)}
              step="0.1"
              value={currentTime}
              onChange={handleSeek}
              onPointerDown={handleSeekStart}
              onPointerUp={handleSeekEnd}
              className="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
              aria-label="拖动调整播放进度"
            />
            <div
              className="player-fill absolute inset-y-0 left-0 rounded-full bg-blue-500 pointer-events-none"
              style={{ width: `${(duration > 0 ? (currentTime / duration) * 100 : 0)}%` }}
            />
          </div>

          <div className="player-time flex items-center justify-between w-full text-sm tabular-nums text-white/80">
            <span>{formatTime(currentTime)}</span>
            <span className="text-white/60">/</span>
            <span>{formatTime(duration)}</span>
          </div>

          <div className="player-rate flex items-center gap-3">
            <button
              onClick={cycleRate}
              className={`player-rate-btn inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition ${
                rateBadge
                  ? "bg-blue-600 text-white"
                  : rateIdx === 0
                    ? "bg-white/10 text-white/90 hover:bg-white/15"
                    : "bg-blue-600 text-white hover:bg-blue-500"
              }`}
              aria-label="切换倍速"
            >
              <span>{RATES[rateIdx]}×</span>
              <span className="text-[10px] opacity-70">倍速</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
