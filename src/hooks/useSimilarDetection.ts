import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { SimilarHash, SimilarProgressPayload } from "../api";
import type { Image } from "../types";
import { extrasOfGroups, liveGroups, mergeSimilarGroups, selectGroupExtras } from "../similarGroups";
import type { HashPair, KeepRule } from "../similarGroups";
import type { Notify } from "./useToasts";

interface SimilarOptions {
  images: Image[];
  setSelectMode: (on: boolean) => void;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  notify: Notify;
}

/** 汉明距离阈值：≤10 / 64 位是业界常用中段，这里允许在 4–16 之间拖 */
export const SIMILAR_THRESHOLD_DEFAULT = 10;
export const SIMILAR_THRESHOLD_MIN = 4;
export const SIMILAR_THRESHOLD_MAX = 16;

const NO_HASHES = new Map<string, HashPair>();

function toHashById(hashes: SimilarHash[]): Map<string, HashPair> {
  const byId = new Map<string, HashPair>();
  for (const hit of hashes) byId.set(hit.id, [hit.lo, hit.hi]);
  return byId;
}

/**
 * 相似图检测（pHash）：后端每算完一批就推一次已成形的组，这里边收边并，
 * 面板开着就能一路审下去。指纹落在库里，所以换「宽容度」只是重新比对一遍：
 * 分组结果整体换掉（不能并，低阈值时留着高阈值的胖组就是错的）。
 */
export function useSimilarDetection({ images, setSelectMode, setSelectedIds, notify }: SimilarOptions) {
  const [groups, setGroups] = useState<string[][]>([]);
  const [hashById, setHashById] = useState<Map<string, HashPair>>(NO_HASHES);
  const [chosenKeeps, setChosenKeeps] = useState<Set<string>>(new Set());
  const [threshold, setThreshold] = useState(SIMILAR_THRESHOLD_DEFAULT);
  const [keepRule, setKeepRule] = useState<KeepRule>("earliest");
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [recalculating, setRecalculating] = useState(false);
  const [progress, setProgress] = useState<{ processed: number; total: number } | null>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  /** 已经自动勾过的副本：只补勾新冒出来的，免得把用户手动取消的又勾回来 */
  const autoSelected = useRef<Set<string>>(new Set());
  const recalcTimer = useRef<number | null>(null);

  /** 只索引相似组里那几百张，避免为 19 万条清单建一张全库 Map */
  const imageById = useMemo(() => {
    const wanted = new Set(groups.flat());
    if (wanted.size === 0) return new Map<string, Image>();
    const byId = new Map<string, Image>();
    for (const image of images) if (wanted.has(image.id)) byId.set(image.id, image);
    return byId;
  }, [images, groups]);

  const aliveGroups = useMemo(
    () => liveGroups(groups, imageById, chosenKeeps, keepRule),
    [groups, imageById, chosenKeeps, keepRule],
  );
  const extras = useMemo(() => extrasOfGroups(aliveGroups), [aliveGroups]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<SimilarProgressPayload>("similar-progress", (event) => {
      const { processed, total, done, groups: fresh } = event.payload;
      setProgress(done || total === 0 ? null : { processed, total });
      if (fresh.length > 0) setGroups(prev => mergeSimilarGroups(prev, fresh));
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 事件收不到只是看不到中间进度，检测结束时仍会拿到完整结果 */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    const fresh = extras.filter(id => !autoSelected.current.has(id));
    for (const id of extras) autoSelected.current.add(id);
    if (fresh.length === 0) return;
    setSelectMode(true);
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const id of fresh) next.add(id);
      return next;
    });
  }, [extras, setSelectedIds, setSelectMode]);

  const detect = useCallback(async () => {
    setDetecting(true);
    setGroups([]);
    setHashById(NO_HASHES);
    setChosenKeeps(new Set());
    autoSelected.current = new Set();
    setPanelOpen(true);
    try {
      const found = await api.findSimilarImages(threshold);
      if (found.groups.length === 0) {
        setDetected(false);
        setPanelOpen(false);
        notify("没有发现相似的图片系列。");
        return;
      }
      setHashById(toHashById(found.hashes));
      setGroups(prev => mergeSimilarGroups(prev, found.groups));
      setDetected(true);
      notify(`检测完成：${found.groups.length} 组相似图片，副本已勾上 ${found.groups.reduce((n, g) => n + g.length - 1, 0)} 张。`);
    } catch (e) {
      notify(`相似检测失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
      setProgress(null);
    }
  }, [notify, threshold]);

  /** 换宽容度：指纹都在库里了，只是重新比对一遍，所以结果整份换掉 */
  const rethreshold = useCallback((next: number) => {
    setThreshold(next);
    if (recalcTimer.current !== null) window.clearTimeout(recalcTimer.current);
    recalcTimer.current = window.setTimeout(async () => {
      recalcTimer.current = null;
      setRecalculating(true);
      try {
        const again = await api.findSimilarImages(next);
        setGroups(again.groups);
        setHashById(toHashById(again.hashes));
        setChosenKeeps(new Set());
        autoSelected.current = new Set();
      } catch (e) {
        notify(`重新比对失败：${String(e)}`, "error");
      } finally {
        setRecalculating(false);
      }
    }, 350);
  }, [notify]);

  useEffect(() => () => {
    if (recalcTimer.current !== null) window.clearTimeout(recalcTimer.current);
  }, []);

  const clear = useCallback(() => {
    if (recalcTimer.current !== null) {
      window.clearTimeout(recalcTimer.current);
      recalcTimer.current = null;
    }
    setGroups([]);
    setHashById(NO_HASHES);
    setChosenKeeps(new Set());
    autoSelected.current = new Set();
    setDetected(false);
    setPanelOpen(false);
  }, []);

  /** 换某组的保留项：该组勾选跟着翻转，其他组的勾选不动 */
  const keep = useCallback((at: number, keepId: string) => {
    const group = aliveGroups.find(item => item.at === at);
    if (!group) return;
    setChosenKeeps(prev => {
      const next = new Set(prev);
      next.delete(group.keep);
      next.add(keepId);
      return next;
    });
    setSelectedIds(prev => selectGroupExtras(prev, group, keepId));
  }, [aliveGroups, setSelectedIds]);

  /** 每组按当前规则的保留张之外的全部勾上 */
  const autoSelect = useCallback(() => {
    setSelectMode(true);
    setSelectedIds(new Set(extras));
    for (const id of extras) autoSelected.current.add(id);
  }, [extras, setSelectMode, setSelectedIds]);

  /**
   * 换选主规则：各组的保留张会整体搬家，勾选必须跟着整份换掉，
   * 沿用原来的增量勾选会让"已经变成主"的那张还留在待删清单里。
   */
  const applyKeepRule = useCallback((next: KeepRule) => {
    setKeepRule(next);
    const fresh = extrasOfGroups(liveGroups(groups, imageById, chosenKeeps, next));
    autoSelected.current = new Set(fresh);
    setSelectMode(true);
    setSelectedIds(new Set(fresh));
  }, [groups, imageById, chosenKeeps, setSelectMode, setSelectedIds]);

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
    detect,
    rethreshold,
    clear,
    keep,
    autoSelect,
    applyKeepRule,
  };
}
