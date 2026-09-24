import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ScanProgressPayload } from "../api";
import type { RescanStatus } from "./useVideos";

/** 订阅后端扫描进度事件；自动重扫切换目录时清掉上一目录遗留的文件计数。
 * 事件名可换：图片库用 image-scan-progress，负载形状与视频一致。 */
export function useScanProgress(
  rescanStatus: RescanStatus | null,
  onNotice: (message: string) => void,
  event = "scan-progress",
) {
  const [scanProgress, setScanProgress] = useState<{ processed: number; total: number } | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<ScanProgressPayload>(event, (received) => {
      const { processed, total, done, warnings } = received.payload;
      setScanProgress(done ? null : { processed, total });
      if (warnings?.length) onNotice(warnings.join("；"));
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(e => onNotice(`扫描进度监听失败：${String(e)}`));
    return () => { disposed = true; unlisten?.(); };
  }, [onNotice, event]);

  useEffect(() => {
    if (rescanStatus) setScanProgress(null);
  }, [rescanStatus]);

  const resetScanProgress = useCallback(() => setScanProgress(null), []);

  return { scanProgress, resetScanProgress };
}
