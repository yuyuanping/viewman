import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { HevcDetectProgressPayload, HevcProgressPayload } from "../api";
import type { Notify } from "./useToasts";

/** HEVC 永久转码：检测库内 HEVC 视频，批量就地重编码为 H.264（原文件进回收站） */
export function useHevcConversion(loadVideos: () => Promise<void>, notify: Notify) {
  const [hevcIds, setHevcIds] = useState<string[]>([]);
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [detectProgress, setDetectProgress] = useState<{ processed: number; total: number } | null>(null);
  const [converting, setConverting] = useState(false);
  const [hevcProgress, setHevcProgress] = useState<{ processed: number; total: number } | null>(null);

  useEffect(() => {
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    listen<HevcProgressPayload>("hevc-progress", (event) => {
      const { processed, total, done } = event.payload;
      setHevcProgress(done ? null : { processed, total });
    }).then((fn) => {
      if (disposed) fn(); else unlisteners.push(fn);
    }).catch(() => { /* 事件监听失败不影响手动刷新 */ });
    listen<HevcDetectProgressPayload>("hevc-detect-progress", (event) => {
      const { processed, total, done } = event.payload;
      setDetectProgress(done ? null : { processed, total });
    }).then((fn) => {
      if (disposed) fn(); else unlisteners.push(fn);
    }).catch(() => { /* 事件监听失败时按钮只显示无计数的检测中 */ });
    return () => { disposed = true; unlisteners.forEach(fn => fn()); };
  }, []);

  const detect = useCallback(async () => {
    setDetecting(true);
    setDetectProgress(null);
    try {
      const ids = await api.findHevcVideos();
      setHevcIds(ids);
      setDetected(true);
      notify(ids.length > 0
        ? `检测到 ${ids.length} 个 HEVC 视频，可永久转码为 H.264。`
        : "没有检测到 HEVC 视频。");
    } catch (e) {
      notify(`检测 HEVC 失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
      setDetectProgress(null);
    }
  }, [notify]);

  const convert = useCallback(async () => {
    if (hevcIds.length === 0) return;
    if (!confirm(
      `将 ${hevcIds.length} 个 HEVC 视频就地重编码为 H.264（画质略降），原文件移入回收站（清空回收站后不可恢复）。转换耗时较长，确定继续？`,
    )) return;
    setConverting(true);
    setHevcProgress({ processed: 0, total: hevcIds.length });
    try {
      const result = await api.convertHevcVideos(hevcIds);
      await loadVideos();
      setHevcIds([]);
      setDetected(false);
      notify(result.converted > 0
        ? `已永久转码 ${result.converted} 个视频${result.errors.length > 0 ? `，${result.errors.length} 个失败` : ""}。`
        : `没有成功转码的视频${result.errors[0] ? `：${result.errors[0]}` : "：可能缺少 ffmpeg 或文件被占用。"}`);
    } catch (e) {
      notify(`永久转码失败：${String(e)}`, "error");
    } finally {
      setConverting(false);
      setHevcProgress(null);
    }
  }, [hevcIds, loadVideos, notify]);

  const clearDetected = useCallback(() => {
    setHevcIds([]);
    setDetected(false);
  }, []);

  return {
    hevcCount: hevcIds.length,
    hevcDetected: detected,
    detecting,
    detectProgress,
    detect,
    converting,
    convert,
    hevcProgress,
    clearDetected,
  };
}
