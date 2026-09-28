import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { api } from "../api";
import type { SimilarHash, SimilarProgressPayload } from "../api";
import { useTauriEvent } from "./useTauriEvent";
import { useGroupReviewCore } from "./useGroupReviewCore";
import type { Image } from "../types";
import { extrasOfGroups, liveGroups } from "../similarGroups";
import type { HashPair, KeepRule } from "../similarGroups";
import type { Notify } from "./useToasts";

interface SimilarOptions {
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

/** 汉明距离阈值：≤10 / 64 位是业界常用中段，这里允许在 4–16 之间拖 */
export const SIMILAR_THRESHOLD_DEFAULT = 10;
export const SIMILAR_THRESHOLD_MIN = 4;
export const SIMILAR_THRESHOLD_MAX = 16;

const NO_HASHES = new Map<string, HashPair>();
const NO_FAR: Set<string> = new Set();

function toHashById(hashes: SimilarHash[]): Map<string, HashPair> {
  const byId = new Map<string, HashPair>();
  for (const hit of hashes) byId.set(hit.id, [hit.lo, hit.hi]);
  return byId;
}

/**
 * 相似图检测（pHash + dHash）：后端每推一次都是当前的完整分组，这里整份换掉——
 * 口径换了组会缩小，把高阈值的胖组留着就是错的。换宽容度也只是重新比对（指纹已在库里）。
 * 组尾还挂着"远亲"：有邻居但没连上骨架，列出来给人看，但不自动勾。
 * 公共的状态机（组/保留张/自动勾选/缓存恢复）在 useGroupReviewCore，这里只留相似检测自己的部分。
 */
export function useSimilarDetection({ fetchImagesByIds, libraryTotal, libraryVersion, setSelectMode, setSelectedIds, notify }: SimilarOptions) {
  const [farIds, setFarIds] = useState<Set<string>>(NO_FAR);
  const [hashById, setHashById] = useState<Map<string, HashPair>>(NO_HASHES);
  const [threshold, setThreshold] = useState(SIMILAR_THRESHOLD_DEFAULT);
  const [keepRule, setKeepRule] = useState<KeepRule>("earliest");
  const [recalculating, setRecalculating] = useState(false);
  const [progress, setProgress] = useState<{ processed: number; total: number } | null>(null);
  const recalcTimer = useRef<number | null>(null);

  // 「活组」口径比查重多两层：keepRule 决定保留张，远亲降级只列出不勾
  const toAliveGroups = useCallback(
    (gs: string[][], byId: Map<string, Image>, keeps: Set<string>) => liveGroups(gs, byId, keeps, keepRule, farIds),
    [keepRule, farIds],
  );

  const {
    groups, setGroups, setChosenKeeps, detected, setDetected, detecting, setDetecting, panelOpen, setPanelOpen,
    ranLive, autoSelected, chosenKeeps, imageById, aliveGroups, stale,
    beginRun, markLiveResult, restoreFromCache, resetResults,
    keep, autoSelect, clearSelection, setGroupSelection,
  } = useGroupReviewCore({ fetchImagesByIds, libraryTotal, libraryVersion, setSelectMode, setSelectedIds, toAliveGroups });

  // 事件收不到只是看不到中间进度，检测结束时仍会拿到完整结果
  useTauriEvent<SimilarProgressPayload>("similar-progress", (payload) => {
    const { processed, total, done, groups: fresh, far } = payload;
    setProgress(done || total === 0 ? null : { processed, total });
    if (fresh.length > 0) {
      // 一出组就算有结果：中途关掉面板还能从工具栏点回去，不必等整趟跑完
      setDetected(true);
      setGroups(fresh);
      setFarIds(new Set(far));
    }
  });

  /** 打开应用先接着上一趟的结果看：检测动辄几分钟，不该是"想看就得再跑一遍" */
  useEffect(() => {
    api.getSimilarCache().then(cached => {
      if (!cached || cached.result.groups.length === 0 || ranLive.current) return;
      restoreFromCache(cached.result.groups, cached.libraryCount);
      setHashById(toHashById(cached.result.hashes));
      setFarIds(new Set(cached.result.far));
    }).catch(() => { /* 读不到缓存就是还没跑过检测 */ });
  }, []);

  const detect = useCallback(async () => {
    setDetecting(true);
    beginRun();
    setFarIds(NO_FAR);
    setHashById(NO_HASHES);
    try {
      const found = await api.findSimilarImages(threshold);
      markLiveResult();
      if (found.groups.length === 0) {
        setDetected(false);
        setPanelOpen(false);
        notify("没有发现相似的图片系列。");
        return;
      }
      setHashById(toHashById(found.hashes));
      setGroups(found.groups);
      setFarIds(new Set(found.far));
      setDetected(true);
      const copies = found.groups.reduce((n, g) => n + g.length, 0) - found.far.length - found.groups.length;
      notify(`检测完成：${found.groups.length} 组相似图片，副本已勾上 ${copies} 张${found.far.length > 0 ? `，另有 ${found.far.length} 张远亲只列出不勾` : ""}。`);
    } catch (e) {
      notify(`相似检测失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
      setProgress(null);
    }
  }, [notify, threshold, setDetecting, beginRun, setDetected, setPanelOpen, setGroups]);

  /** 换宽容度：指纹都在库里了，只是重新比对一遍，所以结果整份换掉。这是用户主动重算，勾选照旧跟上新口径 */
  const rethreshold = useCallback((next: number) => {
    setThreshold(next);
    beginRun();
    if (recalcTimer.current !== null) window.clearTimeout(recalcTimer.current);
    recalcTimer.current = window.setTimeout(async () => {
      recalcTimer.current = null;
      setRecalculating(true);
      try {
        const again = await api.findSimilarImages(next);
        markLiveResult();
        setGroups(again.groups);
        setFarIds(new Set(again.far));
        setHashById(toHashById(again.hashes));
        setChosenKeeps(new Set());
      } catch (e) {
        notify(`重新比对失败：${String(e)}`, "error");
      } finally {
        setRecalculating(false);
      }
    }, 350);
  }, [notify, beginRun, markLiveResult, setGroups, setChosenKeeps]);

  useEffect(() => () => {
    if (recalcTimer.current !== null) window.clearTimeout(recalcTimer.current);
  }, []);

  const clear = useCallback(() => {
    if (recalcTimer.current !== null) {
      window.clearTimeout(recalcTimer.current);
      recalcTimer.current = null;
    }
    setFarIds(NO_FAR);
    setHashById(NO_HASHES);
    resetResults();
    // 面板上按"清空结果"就是连缓存一起作废，不然下次打开又原样复活
    api.clearDetectionCache("similar").catch(() => { /* 清不掉只是重启后还能看到旧结果 */ });
  }, [resetResults]);

  /**
   * 换选主规则：各组的保留张会整体搬家，勾选必须跟着整份换掉，
   * 沿用原来的增量勾选会让"已经变成主"的那张还留在待删清单里。
   */
  const applyKeepRule = useCallback((next: KeepRule) => {
    setKeepRule(next);
    const fresh = extrasOfGroups(liveGroups(groups, imageById, chosenKeeps, next, farIds));
    autoSelected.current = new Set(fresh);
    setSelectMode(true);
    setSelectedIds(new Set(fresh));
  }, [groups, imageById, chosenKeeps, farIds, autoSelected, setSelectMode, setSelectedIds]);

  const similarIds = useMemo(
    () => new Set(aliveGroups.flatMap(group => group.ids)),
    [aliveGroups],
  );

  return {
    groups: aliveGroups,
    imageById,
    hashById,
    similarIds,
    groupCount: aliveGroups.length,
    detected,
    detecting,
    recalculating,
    progress,
    panelOpen,
    setPanelOpen,
    threshold,
    keepRule,
    stale,
    detect,
    rethreshold,
    clear,
    keep,
    autoSelect,
    clearSelection,
    setGroupSelection,
    applyKeepRule,
  };
}
