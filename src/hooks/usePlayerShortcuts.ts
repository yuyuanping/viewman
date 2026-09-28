import { useEffect, type RefObject } from "react";

interface PlayerShortcutActions {
  skip: (delta: number) => void;
  togglePlay: () => void;
  toggleFullscreen: () => void;
  toggleMute: () => void;
}

// 通用播放器快捷键：空格 播放/暂停，←→ ±10 秒，F 全屏，M 静音，↑↓ 音量
export function usePlayerShortcuts(videoRef: RefObject<HTMLVideoElement | null>, actions: PlayerShortcutActions) {
  const { skip, togglePlay, toggleFullscreen, toggleMute } = actions;
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
      } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        skip(e.key === "ArrowLeft" ? -10 : 10);
      } else if (e.key.toLowerCase() === "f") {
        e.preventDefault();
        toggleFullscreen();
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        el.muted = !el.muted;
        toggleMute();
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
  }, [videoRef, skip, togglePlay, toggleFullscreen, toggleMute]);
}
