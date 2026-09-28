import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import type { Video } from "../types";
import { hevcNativeSupported } from "../utils";

export const RATES = [0.75, 1, 1.25, 1.5, 2, 2.5, 3, 4, 5] as const;
export const RATE_KEY = "viewman.playbackRate";

export function loadRate(): number {
  const saved = Number(localStorage.getItem(RATE_KEY));
  return (RATES as readonly number[]).includes(saved) ? saved : 1;
}

interface UsePlayerPlaybackProps {
  video: Video;
  initialPosition: number;
  onProgress: (videoId: string, position: number) => Promise<void>;
  playlist?: Video[];
  onSelect?: (video: Video) => void;
}

// 播放器核心状态与控制：可播放路径解析、进度记忆与落盘、倍速、
// seek/快进快退、播放出错诊断、列表内上/下一个
export function usePlayerPlayback({ video, initialPosition, onProgress, playlist, onSelect }: UsePlayerPlaybackProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const currentTimeRef = useRef(0);
  const alive = useRef(true);
  const lastSaved = useRef(initialPosition);
  const saving = useRef(false);
  const seeking = useRef(false);
  const resumeAfterSeek = useRef(false);

  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [rate, setRate] = useState(loadRate);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const [progressError, setProgressError] = useState<string | null>(null);
  const [fileReadable, setFileReadable] = useState(false);
  const [showError, setShowError] = useState(false);
  // 自动下一集时页面已无用户手势，play() 可能被拦截：回退为静音自动播放
  const [autoMuted, setAutoMuted] = useState(false);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  // 换视频时清掉上一部的播放状态。组件不再整体重挂载（保住全屏），
  // 所以这些得自己归位；上一部的进度由保存 interval 的 cleanup 先落盘，
  // 这个 effect 在所有 cleanup 之后跑，时序上是安全的
  useEffect(() => {
    currentTimeRef.current = 0;
    setCurrentTime(0);
    setDuration(0);
    lastSaved.current = initialPosition;
    setPlaybackError(null);
    setProgressError(null);
    setFileReadable(false);
    setShowError(false);
    setAutoMuted(false);
    seeking.current = false;
    resumeAfterSeek.current = false;
  }, [video.id, initialPosition]);

  const dismissError = useCallback(() => setShowError(false), []);

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

  const tryPlay = useCallback((el: HTMLVideoElement) => {
    el.play()
      .catch(() => {
        el.muted = true;
        el.play()
          .then(() => setAutoMuted(true))
          .catch(() => undefined);
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
      // 优先用 timeupdate 维护的 ref：换视频时元素 src 已经换掉，直接读元素
      // 可能拿到重置后的 0，会把上一部刚存好的进度清掉。
      // position 为 0 不落盘：视频还没真正播过（转码等待中/解码报错/元数据未就绪）时
      // 元素位置恒为 0，存 0 会把打开时带入的进度清掉
      const position = currentTimeRef.current !== 0
        ? currentTimeRef.current
        : videoRef.current?.currentTime ?? 0;
      if (position > 0 && Number.isFinite(position) && position !== lastSaved.current) {
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

  // 手动切换上/下一个视频，切换前先落盘当前进度
  const stepVideo = useCallback((delta: number) => {
    if (!playlist || !onSelect || playlist.length < 2) return;
    const idx = playlist.findIndex(item => item.id === video.id);
    if (idx < 0) return;
    void handleSave();
    onSelect(playlist[(idx + delta + playlist.length) % playlist.length]);
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

  const clearAutoMuted = useCallback(() => setAutoMuted(false), []);

  const unmute = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    el.muted = false;
    clearAutoMuted();
    if (el.paused) tryPlay(el);
  }, [tryPlay, clearAutoMuted]);

  const changeRate = useCallback((e: React.ChangeEvent<HTMLSelectElement>) => {
    const value = Number(e.target.value);
    setRate(value);
    localStorage.setItem(RATE_KEY, String(value));
    const el = videoRef.current;
    if (el) el.playbackRate = value;
  }, []);

  return {
    videoRef,
    playSrc,
    preparing,
    currentTime,
    duration,
    rate,
    changeRate,
    autoMuted,
    unmute,
    clearAutoMuted,
    playbackError,
    progressError,
    fileReadable,
    showError,
    dismissError,
    handlePlaybackError,
    handleLoadedMetadata,
    handleTimeUpdate,
    handleEnded,
    stepVideo,
    togglePlay,
    saveNow: handleSave,
    handleSeekStart,
    handleSeek,
    handleSeekEnd,
    skip,
  };
}
