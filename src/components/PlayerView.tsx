import { useRef, useEffect, useCallback, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Video } from "../types";

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
  const [playbackError, setPlaybackError] = useState(false);

  const src = convertFileSrc(video.path);

  useEffect(() => {
    setPlaybackError(false);
  }, [video.id, src]);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    if (initialPosition > 0) {
      el.currentTime = initialPosition;
    }
  }, [initialPosition]);

  const handleTimeUpdate = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    currentTimeRef.current = el.currentTime;
    setCurrentTime(el.currentTime);
    setDuration(el.duration || 0);
  }, []);

  const handleSave = useCallback(() => {
    const position = videoRef.current?.currentTime ?? currentTimeRef.current;
    if (position > 0) {
      onProgress(video.id, position);
    }
  }, [video.id, onProgress]);

  useEffect(() => {
    const interval = setInterval(handleSave, 15000);
    return () => {
      clearInterval(interval);
      handleSave();
    };
  }, [handleSave]);

  const togglePlay = () => {
    const el = videoRef.current;
    if (!el) return;
    if (el.paused) {
      el.play();
      setPlaying(true);
    } else {
      el.pause();
      setPlaying(false);
    }
  };

  const handleSeek = (e: React.ChangeEvent<HTMLInputElement>) => {
    const el = videoRef.current;
    if (!el) return;
    el.currentTime = parseFloat(e.target.value);
    setCurrentTime(el.currentTime);
  };

  const formatTime = (s: number) => {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = Math.floor(s % 60);
    if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
    return `${m}:${String(sec).padStart(2, "0")}`;
  };

  return (
    <div className="fixed inset-0 bg-black/90 z-50 flex flex-col items-center justify-center">
      <div className="absolute top-4 right-4 flex gap-2">
        <span className="text-gray-400 text-sm self-center">{video.filename}</span>
        <button onClick={onClose} className="bg-gray-700 hover:bg-gray-600 px-3 py-1 rounded text-white">
          关闭
        </button>
      </div>

      <div className="w-full max-w-5xl">
        <video
          ref={videoRef}
          src={src}
          onTimeUpdate={handleTimeUpdate}
          onLoadedMetadata={handleTimeUpdate}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={handleSave}
          onError={() => setPlaybackError(true)}
          className="w-full max-h-[80vh] bg-black"
          controls={false}
        />

        {playbackError && (
          <div className="bg-red-900/80 text-red-200 px-4 py-3 rounded mt-3 text-sm flex flex-col gap-2">
            <span>无法播放此文件，可能是不支持的格式（如 MKV / AVI / WMV / FLV）。</span>
            {onFallback && (
              <button
                onClick={() => onFallback(video)}
                className="self-start bg-gray-700 hover:bg-gray-600 px-3 py-1 rounded text-white"
              >
                用 PotPlayer 打开
              </button>
            )}
          </div>
        )}

        <div className="flex items-center gap-3 mt-3 px-2">
          <button onClick={togglePlay} className="text-white text-xl">
            {playing ? "⏸" : "▶"}
          </button>

          <input
            type="range"
            min={0}
            max={duration || 0}
            step={0.1}
            value={currentTime}
            onChange={handleSeek}
            className="flex-1 accent-blue-500"
          />

          <span className="text-white text-sm tabular-nums">
            {formatTime(currentTime)} / {formatTime(duration)}
          </span>
        </div>
      </div>
    </div>
  );
}
