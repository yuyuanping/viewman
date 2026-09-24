import { useRef, useEffect, useCallback, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import type { Video } from "../types";
import { formatTime, hevcNativeSupported } from "../utils";
import { PlaylistPanel } from "./PlaylistPanel";

interface PlayerViewProps {
  video: Video;
  initialPosition: number;
  onClose: () => void;
  onProgress: (videoId: string, position: number) => Promise<void>;
  onFallback?: (video: Video) => void;
  playlist?: Video[];
  playlistProgress?: Record<string, number | null>;
  onSelect?: (video: Video) => void;
  onDelete?: (video: Video) => void;
  onMove?: (video: Video) => void;
}

export const RATES = [0.75, 1, 1.25, 1.5, 2, 2.5, 3, 4, 5] as const;
export const RATE_KEY = "viewman.playbackRate";

export function loadRate(): number {
  const saved = Number(localStorage.getItem(RATE_KEY));
  return (RATES as readonly number[]).includes(saved) ? saved : 1;
}

export function PlayerView({ video, initialPosition, onClose, onProgress, onFallback, playlist, playlistProgress, onSelect, onDelete, onMove }: PlayerViewProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const currentTimeRef = useRef(0);
  const [playing, setPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [rate, setRate] = useState(loadRate);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const [progressError, setProgressError] = useState<string | null>(null);
  // 截图：成功/失败走 notice 条（播放器内没有 Toast 队列）
  const [captureNotice, setCaptureNotice] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [fileReadable, setFileReadable] = useState(false);
  const [showError, setShowError] = useState(false);
  const [autoMuted, setAutoMuted] = useState(false);
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
      const result = await api.checkVideoFile(video.id);
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

  // WebView2 解不了 HEVC：先询问后端可播放路径（必要时等待转码），再挂载 video
  const [playSrc, setPlaySrc] = useState<string | null>(null);
  const [preparing, setPreparing] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setPlaySrc(null);
    if (hevcNativeSupported) {
      setPlaySrc(convertFileSrc(video.path));
      return () => { cancelled = true; };
    }
    setPreparing(true);
    (async () => {
      let resolved: string;
      try {
        resolved = convertFileSrc(await api.getPlayablePath(video.id));
      } catch {
        resolved = convertFileSrc(video.path);
      }
      if (!cancelled) {
        setPlaySrc(resolved);
        setPreparing(false);
      }
    })();
    return () => { cancelled = true; };
  }, [video.id, video.path]);

  useEffect(() => {
    if (!showError) setPlaybackError(null);
  }, [video.id, playSrc, showError]);

  // 自动下一集时页面已无用户手势，play() 可能被拦截：回退为静音自动播放
  const tryPlay = useCallback((el: HTMLVideoElement) => {
    el.play()
      .then(() => setPlaying(true))
      .catch(() => {
        el.muted = true;
        el.play()
          .then(() => { setPlaying(true); setAutoMuted(true); })
          .catch(() => setPlaying(false));
      });
  }, []);

  // 元数据就绪后跳到上次进度，套用记住的倍速，接着尝试自动播放
  const handleLoadedMetadata = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;

    if (video.duration && video.duration > 0) {
      setDuration(video.duration);
    } else {
      setDuration(el.duration || 0);
    }

    // 上次已看到结尾的视频从头播，避免"秒结束"连锁跳集
    const dur = video.duration ?? el.duration ?? 0;
    if (initialPosition > 0 && initialPosition < dur - 1.5 && el.currentTime < initialPosition) {
      el.currentTime = initialPosition;
    }

    el.playbackRate = rate;
    tryPlay(el);
  }, [initialPosition, rate, video.duration, tryPlay]);

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

  // 播完先落盘进度，再自动切到列表中的下一个视频；到末尾后回到开头继续播
  const handleEnded = useCallback(async () => {
    await handleSave();
    if (!playlist || !onSelect || playlist.length < 2) return;
    const idx = playlist.findIndex(item => item.id === video.id);
    if (idx >= 0) onSelect(playlist[(idx + 1) % playlist.length]);
  }, [handleSave, playlist, video.id, onSelect]);

  const togglePlay = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    if (el.paused) {
      el.muted = false;
      setAutoMuted(false);
      tryPlay(el);
    } else {
      el.pause();
      setPlaying(false);
    }
  }, [tryPlay]);

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
      tryPlay(el);
    }
    resumeAfterSeek.current = false;
  }, [tryPlay]);

  // ±10 秒快进/后退，两端做夹取
  const skip = useCallback((delta: number) => {
    const el = videoRef.current;
    if (!el) return;
    const dur = Number.isFinite(el.duration) ? el.duration : 0;
    let target = el.currentTime + delta;
    if (target < 0) target = 0;
    if (dur > 0 && target > dur - 0.1) target = Math.max(dur - 0.1, 0);
    el.currentTime = target;
    setCurrentTime(target);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      const t = e.target as HTMLElement | null;
      // 焦点在进度条等表单控件上时交给控件自身的键盘行为
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      e.preventDefault();
      skip(e.key === "ArrowLeft" ? -10 : 10);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [skip]);

  const unmute = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    el.muted = false;
    setAutoMuted(false);
    if (el.paused) tryPlay(el);
  }, [tryPlay]);

  // 截图当前帧：后端 ffmpeg 抽帧存到视频同目录，几秒后自动收起成功提示
  const captureTimerRef = useRef<number | null>(null);
  const handleCapture = useCallback(async () => {
    const el = videoRef.current;
    if (!el || capturing) return;
    setCapturing(true);
    setCaptureNotice(null);
    try {
      const out = await api.captureFrame(video.id, el.currentTime);
      setCaptureNotice(`已保存：${out}`);
    } catch (e) {
      setCaptureNotice(`截图失败：${String(e)}`);
    } finally {
      setCapturing(false);
      if (captureTimerRef.current) window.clearTimeout(captureTimerRef.current);
      captureTimerRef.current = window.setTimeout(() => setCaptureNotice(null), 5000);
    }
  }, [video.id, capturing]);

  const changeRate = useCallback((e: React.ChangeEvent<HTMLSelectElement>) => {
    const value = Number(e.target.value);
    setRate(value);
    localStorage.setItem(RATE_KEY, String(value));
    const el = videoRef.current;
    if (el) el.playbackRate = value;
  }, []);

  // 全屏：对整个播放层请求全屏，自绘控制条在全屏内仍可用；Esc 可退出
  const overlayRef = useRef<HTMLDivElement>(null);
  const [isFullscreen, setIsFullscreen] = useState(false);
  useEffect(() => {
    const onChange = () => setIsFullscreen(document.fullscreenElement != null);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);
  const toggleFullscreen = useCallback(() => {
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => undefined);
    } else {
      void overlayRef.current?.requestFullscreen().catch(() => undefined);
    }
  }, []);

  // 通用播放器快捷键：空格 播放/暂停，F 全屏，M 静音，↑↓ 音量
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      // 输入控件里有自己的键行为，不劫持
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      const el = videoRef.current;
      if (!el) return;
      if (e.key === " ") {
        e.preventDefault();
        togglePlay();
      } else if (e.key.toLowerCase() === "f") {
        e.preventDefault();
        toggleFullscreen();
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        el.muted = !el.muted;
        setAutoMuted(false);
      } else if (e.key.toLowerCase() === "s") {
        e.preventDefault();
        void handleCapture();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        el.volume = Math.min(1, el.volume + 0.1);
        el.muted = false;
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        el.volume = Math.max(0, el.volume - 0.1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [togglePlay, toggleFullscreen, handleCapture]);

  // 画面旋转：每点一次顺时针 90°；90/270 时按旋转后的外接框重新约束视频
  const [rot, setRot] = useState(0);
  const rotateFrame = useCallback(() => setRot(r => (r + 90) % 360), []);
  const fitBoxRef = useRef<HTMLDivElement>(null);
  const [fitBox, setFitBox] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = fitBoxRef.current;
    if (!el) return;
    const measure = () => setFitBox({ w: el.clientWidth, h: el.clientHeight });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);
  const swapped = rot === 90 || rot === 270;
  const videoStyle = swapped && fitBox.w > 0
    ? { transform: `rotate(${rot}deg)`, maxWidth: `${fitBox.h}px`, maxHeight: `${fitBox.w}px` }
    : { transform: `rotate(${rot}deg)` };

  return (
    <div ref={overlayRef} className="player-overlay fixed inset-0 z-50 flex flex-col">
      <div className="flex-1 min-h-0 flex">
        <div className="player-stage flex-1 min-h-0 flex items-center justify-center px-4 pt-14 pb-2">
          <div ref={fitBoxRef} className="w-full h-full min-w-0 min-h-0 flex items-center justify-center">
            {playSrc ? (
              <video
                ref={videoRef}
                src={playSrc}
                onTimeUpdate={handleTimeUpdate}
                onLoadedMetadata={handleLoadedMetadata}
                onPlay={() => setPlaying(true)}
                onPause={() => setPlaying(false)}
                onEnded={handleEnded}
                onError={() => { void handlePlaybackError(); }}
                onDoubleClick={toggleFullscreen}
                style={videoStyle}
                className="player-video max-h-full max-w-full bg-black object-contain transition-transform duration-200"
                controls={false}
                playsInline
              />
            ) : (
              <p className="text-sm text-white/70">
                {preparing ? "正在准备可播放版本（HEVC 转码可能需稍等）…" : "加载中…"}
              </p>
            )}
          </div>
        </div>

        {playlist && playlist.length > 0 && onSelect && (
          <PlaylistPanel
            playlist={playlist}
            currentId={video.id}
            progress={playlistProgress}
            onSelect={onSelect}
            onDelete={onDelete}
            onMove={onMove}
          />
        )}
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
        {captureNotice && (
          <p className="player-progress-notice text-gray-300 text-sm truncate max-w-full px-6" title={captureNotice}>{captureNotice}</p>
        )}

        <div className="player-controls w-full max-w-3xl flex flex-col gap-3.5">
          <div className="player-transport flex items-center justify-center gap-4">
            <button
              onClick={() => skip(-10)}
              className="player-skip inline-flex items-center justify-center w-11 h-11 rounded-full bg-white/10 hover:bg-white/15 text-white text-sm font-medium transition"
              aria-label="后退10秒"
              title="后退 10 秒（←）"
            >
              ⏪10
            </button>
            <button
              onClick={togglePlay}
              className="player-play inline-flex items-center justify-center w-14 h-14 rounded-full bg-white/10 hover:bg-white/15 text-white text-2xl transition"
              aria-label={playing ? "暂停" : "播放"}
            >
              {playing ? "❚❚" : "▶"}
            </button>
            <button
              onClick={() => skip(10)}
              className="player-skip inline-flex items-center justify-center w-11 h-11 rounded-full bg-white/10 hover:bg-white/15 text-white text-sm font-medium transition"
              aria-label="快进10秒"
              title="快进 10 秒（→）"
            >
              10⏩
            </button>
          </div>

          <div className="flex items-center justify-center gap-3 text-xs text-gray-300">
            <button
              onClick={() => void handleCapture()}
              disabled={capturing}
              className="inline-flex items-center gap-1.5 rounded-full bg-white/10 hover:bg-white/15 px-3 py-1.5 transition disabled:opacity-50"
              title="截图当前帧到视频所在目录（S）"
            >
              {capturing ? "截图中…" : "📷 截图"}
            </button>
          </div>

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
            <select
              value={rate}
              onChange={changeRate}
              className={`player-rate-select rounded-full px-3 py-1.5 text-xs font-medium cursor-pointer outline-none transition bg-white/10 text-white/90 hover:bg-white/15 [&>option]:bg-gray-900 [&>option]:text-white ${rate !== 1 ? "!bg-blue-600 text-white hover:!bg-blue-500" : ""}`}
              aria-label="播放倍速"
              title="播放倍速"
            >
              {RATES.map(r => (
                <option key={r} value={r}>{r}× 倍速</option>
              ))}
            </select>
            <button
              onClick={rotateFrame}
              className={`player-rotate-btn inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition ${
                rot !== 0 ? "bg-blue-600 text-white hover:bg-blue-500" : "bg-white/10 text-white/90 hover:bg-white/15"
              }`}
              aria-label="画面顺时针旋转 90 度"
              title="画面旋转 90°"
            >
              <span>{rot === 0 ? "旋转" : `${rot}°`}</span>
              <span className={`text-[10px] ${rot === 0 ? "opacity-70" : "opacity-80"}`}>画面</span>
            </button>
            <button
              onClick={toggleFullscreen}
              className={`player-fullscreen-btn inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition ${
                isFullscreen ? "bg-blue-600 text-white hover:bg-blue-500" : "bg-white/10 text-white/90 hover:bg-white/15"
              }`}
              aria-label={isFullscreen ? "退出全屏" : "全屏"}
              title={isFullscreen ? "退出全屏（Esc）" : "全屏播放（也可双击画面）"}
            >
              <span>{isFullscreen ? "⛶ 退出" : "⛶ 全屏"}</span>
            </button>
            {autoMuted && (
              <button
                onClick={unmute}
                className="player-muted-chip inline-flex items-center gap-1.5 rounded-full bg-amber-600/80 hover:bg-amber-600 px-3 py-1 text-xs font-medium text-white transition"
              >
                自动播放已静音 · 点击恢复声音
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
