import { useEffect, type RefObject } from "react";

interface PlayerShortcutActions {
  skip: (delta: number) => void;
  togglePlay: () => void;
  toggleFullscreen: () => void;
  toggleMute: () => void;
  /** ↑↓ 步进倍速：+1 加速 / -1 减速，挡位与控制条下拉一致 */
  stepRate: (delta: number) => void;
}

// 通用播放器快捷键：空格 播放/暂停，←→ ±10 秒，F 全屏，M 静音，↑↓ 步进倍速，Shift+↑↓ 音量
export function usePlayerShortcuts(videoRef: RefObject<HTMLVideoElement | null>, actions: PlayerShortcutActions) {
  const { skip, togglePlay, toggleFullscreen, toggleMute, stepRate } = actions;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      // 输入控件里有自己的键行为，不劫持（SELECT 是倍速下拉：聚焦时 ↑↓ 是它原生的换挡）
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
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
      } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        if (e.shiftKey) {
          // Shift 档是音量：加大音量时顺手解除静音，不然加了也听不见
          el.volume = e.key === "ArrowUp" ? Math.min(1, el.volume + 0.1) : Math.max(0, el.volume - 0.1);
          if (e.key === "ArrowUp") el.muted = false;
        } else {
          stepRate(e.key === "ArrowUp" ? 1 : -1);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [videoRef, skip, togglePlay, toggleFullscreen, toggleMute, stepRate]);
}
