import { useCallback, useEffect, useRef, useState } from "react";
import type { Video } from "../types";
import { formatTime } from "../utils";
import { PlaylistPanel } from "./PlaylistPanel";
import { usePlayerPlayback, RATES } from "../hooks/usePlayerPlayback";
import { usePlayerShortcuts } from "../hooks/usePlayerShortcuts";
import { useFullscreen } from "../hooks/useFullscreen";
import { useVideoRotation } from "../hooks/useVideoRotation";

interface PlayerViewProps {
  video: Video;
  initialPosition: number;
  onClose: () => void;
  onProgress: (videoId: string, position: number) => Promise<void>;
  onFallback?: (video: Video, position: number) => void;
  playlist?: Video[];
  playlistProgress?: Record<string, number | null>;
  onSelect?: (video: Video) => void;
  onDelete?: (video: Video) => void;
  onMove?: (video: Video) => void;
}

const PLAYLIST_VISIBLE_KEY = "viewman.playlistVisible";

export function PlayerView({ video, initialPosition, onClose, onProgress, onFallback, playlist, playlistProgress, onSelect, onDelete, onMove }: PlayerViewProps) {
  const playback = usePlayerPlayback({ video, initialPosition, onProgress, playlist, onSelect });
  const { targetRef: overlayRef, isFullscreen, toggleFullscreen } = useFullscreen<HTMLDivElement>();
  const { fitBoxRef, rot, rotateFrame, videoStyle } = useVideoRotation(video.id);

  // 全屏时隐藏标题栏和控制条，鼠标移动后短暂显示，2.5 秒无操作自动再隐藏
  const [controlsVisible, setControlsVisible] = useState(true);
  const controlsTimer = useRef<number | null>(null);
  useEffect(() => {
    if (!isFullscreen) {
      setControlsVisible(true);
      if (controlsTimer.current) window.clearTimeout(controlsTimer.current);
      return;
    }
    setControlsVisible(false);
  }, [isFullscreen]);
  useEffect(() => () => {
    if (controlsTimer.current) window.clearTimeout(controlsTimer.current);
  }, []);
  const handleOverlayMouseMove = useCallback(() => {
    if (!isFullscreen) return;
    setControlsVisible(true);
    if (controlsTimer.current) window.clearTimeout(controlsTimer.current);
    controlsTimer.current = window.setTimeout(() => setControlsVisible(false), 2500);
  }, [isFullscreen]);
  const chromeVisible = !isFullscreen || controlsVisible;

  // 播放列表显隐开关，记住用户上次的选择
  const [listVisible, setListVisible] = useState(() => localStorage.getItem(PLAYLIST_VISIBLE_KEY) !== "0");
  const toggleList = useCallback(() => {
    setListVisible(v => {
      localStorage.setItem(PLAYLIST_VISIBLE_KEY, v ? "0" : "1");
      return !v;
    });
  }, []);

  usePlayerShortcuts(playback.videoRef, {
    skip: playback.skip,
    togglePlay: playback.togglePlay,
    toggleFullscreen,
    toggleMute: playback.clearAutoMuted,
  });

  return (
    <div ref={overlayRef} onMouseMove={handleOverlayMouseMove} className="player-overlay fixed inset-0 z-50 flex flex-col">
      <div className="flex-1 min-h-0 flex">
        <div className={`player-stage flex-1 min-h-0 flex items-center justify-center ${isFullscreen ? "p-0" : "px-4 pt-14 pb-2"}`}>
          <div ref={fitBoxRef} className="w-full h-full min-w-0 min-h-0 flex items-center justify-center">
            {playback.playSrc ? (
              <video
                ref={playback.videoRef}
                src={playback.playSrc}
                onTimeUpdate={playback.handleTimeUpdate}
                onLoadedMetadata={playback.handleLoadedMetadata}
                onEnded={playback.handleEnded}
                onError={() => { void playback.handlePlaybackError(); }}
                onClick={playback.togglePlay}
                onDoubleClick={toggleFullscreen}
                style={videoStyle}
                className="player-video max-h-full max-w-full bg-black object-contain transition-transform duration-200"
                controls={false}
                playsInline
              />
            ) : (
              <p className="text-sm text-white/70">
                {playback.preparing ? "正在准备可播放版本（HEVC 转码可能需稍等）…" : "加载中…"}
              </p>
            )}
          </div>
        </div>

        {playlist && playlist.length > 0 && onSelect && !isFullscreen && listVisible && (
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

      <div className={`player-top absolute top-0 left-0 right-0 z-10 flex justify-between items-start px-4 py-3 gap-3 transition-opacity duration-300 ${chromeVisible ? "opacity-100" : "opacity-0 pointer-events-none"}`}>
        <div className="player-title min-w-0">
          <span className="truncate text-sm text-white/90 font-medium">{video.filename}</span>
        </div>
        <div className="player-actions flex gap-2 shrink-0">
          {playlist && playlist.length > 0 && (
            <button
              onClick={toggleList}
              className="player-btn rounded-lg border border-white/10 bg-white/5 hover:bg-white/10 px-3 py-1.5 text-sm text-white/90 hover:text-white transition"
              title={listVisible ? "隐藏播放列表" : "显示播放列表"}
            >
              {listVisible ? "隐藏列表" : "显示列表"}
            </button>
          )}
          <button
            onClick={onClose}
            className="player-btn rounded-lg border border-white/10 bg-white/5 hover:bg-white/10 px-3 py-1.5 text-sm text-white/90 hover:text-white transition"
          >
            关闭
          </button>
        </div>
      </div>

      <div className={`player-bar shrink-0 w-full flex flex-col items-center gap-2 px-4 pb-5 transition-opacity duration-300 ${isFullscreen ? "absolute bottom-0 left-0 right-0 z-10" : ""} ${chromeVisible ? "opacity-100" : "opacity-0 pointer-events-none"}`}>
        {playback.showError && playback.playbackError && (
          <div className="player-error w-full max-w-xl rounded-xl border border-red-500/30 bg-red-900/60 px-4 py-3 text-sm text-red-200 flex flex-col gap-3">
            <div className="flex items-start justify-between gap-3">
              <span className="flex-1">{playback.playbackError}</span>
              <button
                onClick={playback.dismissError}
                className="shrink-0 text-red-300 hover:text-white transition"
                aria-label="关闭提示"
              >
                ✕
              </button>
            </div>
            {playback.fileReadable && onFallback && (
              <button
                onClick={() => {
                  // 先把当前进度落盘，再带着播放器的实时位置交给外部播放器
                  void playback.saveNow().then(() => onFallback(video, playback.currentTime));
                }}
                className="self-start rounded-lg border border-white/10 bg-white/5 hover:bg-white/10 px-3 py-1.5 text-sm text-white/90 hover:text-white transition"
              >
                用 PotPlayer 打开
              </button>
            )}
          </div>
        )}

        {playback.progressError && (
          <p className="player-progress-notice text-amber-300 text-sm">{playback.progressError}</p>
        )}

        <div className="player-controls w-full max-w-3xl flex flex-col gap-3.5">
          <div className="player-progress relative w-full h-1.5 rounded-full bg-white/10 cursor-pointer group">
            <input
              type="range"
              min={0}
              max={Math.max(playback.duration, 1)}
              step="0.1"
              value={playback.currentTime}
              onChange={playback.handleSeek}
              onPointerDown={playback.handleSeekStart}
              onPointerUp={playback.handleSeekEnd}
              className="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
              aria-label="拖动调整播放进度"
            />
            <div
              className="player-fill absolute inset-y-0 left-0 rounded-full bg-blue-500 pointer-events-none"
              style={{ width: `${(playback.duration > 0 ? (playback.currentTime / playback.duration) * 100 : 0)}%` }}
            />
          </div>

          <div className="player-time flex items-center justify-between w-full text-sm tabular-nums text-white/80">
            <span>{formatTime(playback.currentTime)}</span>
            <span className="text-white/60">/</span>
            <span>{formatTime(playback.duration)}</span>
          </div>

          <div className="player-rate flex items-center gap-3">
            <button
              onClick={() => playback.stepVideo(-1)}
              disabled={!playlist || playlist.length < 2}
              className="inline-flex items-center gap-1.5 rounded-full bg-white/10 hover:bg-white/15 px-3 py-1.5 text-xs text-white/90 hover:text-white transition disabled:opacity-40 disabled:cursor-not-allowed"
              title="上一个视频"
            >
              ⏮ 上一个
            </button>
            <button
              onClick={() => playback.stepVideo(1)}
              disabled={!playlist || playlist.length < 2}
              className="inline-flex items-center gap-1.5 rounded-full bg-white/10 hover:bg-white/15 px-3 py-1.5 text-xs text-white/90 hover:text-white transition disabled:opacity-40 disabled:cursor-not-allowed"
              title="下一个视频"
            >
              下一个 ⏭
            </button>
            <select
              value={playback.rate}
              onChange={playback.changeRate}
              className={`player-rate-select rounded-full px-3 py-1.5 text-xs font-medium cursor-pointer outline-none transition bg-white/10 text-white/90 hover:bg-white/15 [&>option]:bg-gray-900 [&>option]:text-white ${playback.rate !== 1 ? "!bg-blue-600 text-white hover:!bg-blue-500" : ""}`}
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
            {playback.autoMuted && (
              <button
                onClick={playback.unmute}
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
