import { useCallback, useEffect, useMemo, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { api } from "../api";
import type { DuplicateProgressPayload } from "../api";
import { useTauriEvent } from "./useTauriEvent";
import { useGroupReviewCore, aliveGroupsForDuplicates } from "./useGroupReviewCore";
import type { Image } from "../types";
import type { Notify } from "./useToasts";

interface DuplicateGroupOptions {
  /** 按 id 取图：库不再整表下发，面板里成组那几百张的元数据现查现用 */
  fetchImagesByIds: (ids: string[]) => Promise<Image[]>;
  /** 当前库内总数：恢复出来的旧结果拿它判断过没过期 */
  libraryTotal: number;
  /** 库内容代次：删除/移动/扫描之后 bump，面板据此把退库的组员裁出列表 */
  libraryVersion: number;
  setSelectMode: (on: boolean) => void;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  notify: Notify;
}

/** 进度里带上的阶段与累计跳过的张数 */
export interface DuplicateRunProgress {
  processed: number;
  total: number;
  stage: string;
  skipped: number;
}

const NO_PROGRESS = null;

/**
 * 重复图片（内容相同）的分组审阅：后端边核对候选桶边推当前的完整分组，这里整份换掉。
 * 判定按"双指纹相同 + 画面核对"，保留张是组内最早入库那张，其余自动勾进多选。
 * 勾选走增量：只补新冒出来的副本，用户手动取消过的不会被擅自勾回去。
 * 公共的状态机（组/保留张/自动勾选/缓存恢复）在 useGroupReviewCore，这里只留查重自己的部分。
 */
export function useDuplicateGroups({ fetchImagesByIds, libraryTotal, libraryVersion, setSelectMode, setSelectedIds, notify }: DuplicateGroupOptions) {
  const [skipped, setSkipped] = useState(0);
  const [progress, setProgress] = useState<DuplicateRunProgress | null>(NO_PROGRESS);

  const {
    setGroups, detected, setDetected, detecting, setDetecting, panelOpen, setPanelOpen,
    ranLive, imageById, aliveGroups, extras, stale,
    beginRun, markLiveResult, restoreFromCache, resetResults,
    keep, autoSelect, clearSelection, setGroupSelection,
  } = useGroupReviewCore({ fetchImagesByIds, libraryTotal, libraryVersion, setSelectMode, setSelectedIds, toAliveGroups: aliveGroupsForDuplicates });

  // 事件收不到只是看不到中间进度，检测结束时仍会拿到完整结果
  useTauriEvent<DuplicateProgressPayload>("duplicate-progress", (payload) => {
    const { processed, total, stage, skipped: gone, groups: fresh } = payload;
    setProgress(total > 0 && processed < total ? { processed, total, stage, skipped: gone ?? 0 } : NO_PROGRESS);
    if (fresh && fresh.length > 0) {
      // 一出组就算有结果：中途关掉面板还能从工具栏点回去，不必等整趟跑完
      setDetected(true);
      setGroups(fresh);
      setSkipped(gone ?? 0);
    }
  });

  /** 打开应用先接着上一趟的结果看：重复检测热跑也要几分钟，不该是"想看就得再跑一遍" */
  useEffect(() => {
    api.getDuplicateCache().then(cached => {
      if (!cached || cached.report.groups.length === 0 || ranLive.current) return;
      restoreFromCache(cached.report.groups, cached.libraryCount);
      setSkipped(cached.report.skipped ?? 0);
    }).catch(() => { /* 读不到缓存就是还没跑过检测 */ });
  }, []);

  const detect = useCallback(async () => {
    setDetecting(true);
    beginRun();
    setSkipped(0);
    try {
      const found = await api.findDuplicateImages();
      markLiveResult();
      setSkipped(found.skipped ?? 0);
      if (found.groups.length === 0) {
        setDetected(false);
        setPanelOpen(false);
        // 解不出画面的条目根本没进比对，不报出来的话"没有重复"会被当成定论
        notify(found.skipped ? `没有发现内容相同的重复图片。另有 ${found.skipped} 张解不出画面，未参与比对。` : "没有发现内容相同的重复图片。");
        return;
      }
      setGroups(found.groups);
      setDetected(true);
      const copies = found.groups.reduce((n, g) => n + g.length - 1, 0);
      notify(`检测完成：${found.groups.length} 组重复图片，副本已勾上 ${copies} 张${found.skipped ? `，另有 ${found.skipped} 张解不出画面未参与比对` : ""}。`);
    } catch (e) {
      notify(`检测重复失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
      setProgress(NO_PROGRESS);
    }
  }, [notify, setDetecting, beginRun, markLiveResult, setDetected, setPanelOpen, setGroups]);

  const clear = useCallback(() => {
    resetResults();
    setSkipped(0);
    // 面板上按"清空结果"就是连缓存一起作废，不然下次打开又原样复活
    api.clearDetectionCache("duplicate").catch(() => { /* 清不掉只是重启后还能看到旧结果 */ });
  }, [resetResults]);

  /** 组内除保留张之外的副本：网格上标"重复副本"的就是这一批 */
  const duplicateIds = useMemo(() => new Set(extras), [extras]);

  return {
    groups: aliveGroups,
    imageById,
    duplicateIds,
    groupCount: aliveGroups.length,
    skipped,
    stale,
    detected,
    detecting,
    progress,
    panelOpen,
    setPanelOpen,
    detect,
    clear,
    keep,
    autoSelect,
    clearSelection,
    setGroupSelection,
  };
}
