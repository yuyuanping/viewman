import { useCallback, useEffect, useState } from "react";
import type { ScanProgressPayload } from "../api";
import { useTauriEvent } from "./useTauriEvent";
import type { RescanStatus } from "./useVideos";

/** 订阅后端扫描进度事件；自动重扫切换目录时清掉上一目录遗留的文件计数。
 * 事件名可换：图片库用 image-scan-progress，负载形状与视频一致。 */
export function useScanProgress(
  rescanStatus: RescanStatus | null,
  onNotice: (message: string) => void,
  event = "scan-progress",
) {
  const [scanProgress, setScanProgress] = useState<{ processed: number; total: number } | null>(null);

  useTauriEvent<ScanProgressPayload>(
    event,
    (payload) => {
      const { processed, total, done, warnings, summary } = payload;
      setScanProgress(done ? null : { processed, total });
      if (warnings?.length) onNotice(warnings.join("；"));
      // 扫描完成：有实际变更才提示，让"扫了一圈什么都没变"保持安静
      if (done && summary) {
        const parts: string[] = [];
        if (summary.added > 0) parts.push(`新增 ${summary.added}`);
        if (summary.removed > 0) parts.push(`移除 ${summary.removed}`);
        if (summary.refreshed > 0) parts.push(`刷新 ${summary.refreshed}`);
        if (parts.length > 0) onNotice(`扫描完成：${parts.join("，")}`);
      }
    },
    (e) => onNotice(`扫描进度监听失败：${String(e)}`),
  );

  useEffect(() => {
    if (rescanStatus) setScanProgress(null);
  }, [rescanStatus]);

  const resetScanProgress = useCallback(() => setScanProgress(null), []);

  return { scanProgress, resetScanProgress };
}
