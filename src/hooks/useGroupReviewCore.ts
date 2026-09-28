import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { restoredResultStale } from "../detectionCache";
import type { Image } from "../types";
import { extrasOfGroups, liveGroups, selectGroupExtras } from "../similarGroups";
import type { SimilarGroup } from "../similarGroups";

const NO_INDEX = new Map<string, Image>();

export interface GroupReviewCoreOptions {
  /** 按 id 取图：库不再整表下发，面板里成组那几百张的元数据现查现用 */
  fetchImagesByIds: (ids: string[]) => Promise<Image[]>;
  /** 当前库内总数：恢复出来的旧结果拿它判断过没过期 */
  libraryTotal: number;
  /** 库内容代次：删除/移动/扫描之后 bump，面板据此把退库的组员裁出列表 */
  libraryVersion: number;
  setSelectMode: (on: boolean) => void;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  /** 「活组」口径两组不同：查重按组员存活裁剪，相似还叠加 keepRule 与远亲降级 */
  toAliveGroups: (groups: string[][], imageById: Map<string, Image>, chosenKeeps: Set<string>) => SimilarGroup[];
}

/**
 * 分组审阅类检测（重复/相似）的公共核心：分组与保留张状态、成组元数据现查、
 * 增量自动勾选、keep/autoSelect/整组勾选这些面板动作，以及缓存恢复的骨架。
 * 各自的检测请求、进度事件与特有字段（skipped/hashById/far/keepRule）留在各自的 hook 里。
 */
export function useGroupReviewCore({ fetchImagesByIds, libraryTotal, libraryVersion, setSelectMode, setSelectedIds, toAliveGroups }: GroupReviewCoreOptions) {
  const [groups, setGroups] = useState<string[][]>([]);
  const [chosenKeeps, setChosenKeeps] = useState<Set<string>>(new Set());
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [panelOpen, setPanelOpen] = useState(false);
  /** 从缓存恢复的那一轮别自动勾副本：那是上一趟的结果，凭什么一打开就把人待删清单填满 */
  const [autoTick, setAutoTick] = useState(true);
  /** 非 null = 现在看的是上一趟存的分组，值是它记下时的库内记录数；现跑一次就清成 null */
  const [restoredCount, setRestoredCount] = useState<number | null>(null);
  /** 已经自动勾过的副本：只补勾新冒出来的，免得把用户手动取消的又勾回来 */
  const autoSelected = useRef<Set<string>>(new Set());
  /** 这一轮是不是已经自己点过检测了：缓存回得慢的话别把刚跑出来的结果盖掉 */
  const ranLive = useRef(false);

  /** 只索引成组的图片：元数据按 id 现查，不为全库建 Map */
  const [imageById, setImageById] = useState<Map<string, Image>>(NO_INDEX);
  useEffect(() => {
    const wanted = [...new Set(groups.flat())];
    if (wanted.length === 0) {
      setImageById(NO_INDEX);
      return;
    }
    let disposed = false;
    fetchImagesByIds(wanted).then(rows => {
      if (disposed) return;
      setImageById(new Map(rows.map(image => [image.id, image])));
    }).catch(() => { /* 查不到就先空着，面板只是显示不出缩略图 */ });
    return () => { disposed = true; };
    // libraryVersion 变了要重查：删除/移动之后这份元数据还挂着旧账，列表就"不变"了
  }, [groups, fetchImagesByIds, libraryVersion]);

  const aliveGroups = useMemo(
    () => toAliveGroups(groups, imageById, chosenKeeps),
    [groups, imageById, chosenKeeps, toAliveGroups],
  );
  const extras = useMemo(() => extrasOfGroups(aliveGroups), [aliveGroups]);

  /**
   * 跟着分组结果增量同步勾选：新冒出来的副本补勾上，已经不算副本的撤勾——
   * 沿用上一轮的勾就等于按新结果删错图。用户手动取消过的不会被擅自勾回去。
   * 从缓存恢复的那一份只记账不勾选（autoTick 关掉），免得稍后把这几张当成"新冒出来的"补勾上去。
   */
  useEffect(() => {
    if (!autoTick) {
      for (const id of extras) autoSelected.current.add(id);
      return;
    }
    const want = new Set(extras);
    const fresh = extras.filter(id => !autoSelected.current.has(id));
    const stale = [...autoSelected.current].filter(id => !want.has(id));
    if (fresh.length === 0 && stale.length === 0) return;
    for (const id of fresh) autoSelected.current.add(id);
    for (const id of stale) autoSelected.current.delete(id);
    if (fresh.length > 0) setSelectMode(true);
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const id of fresh) next.add(id);
      for (const id of stale) next.delete(id);
      return next;
    });
  }, [extras, autoTick, setSelectedIds, setSelectMode]);

  /** 现跑新一轮前把上一轮的账清干净：组、保留张、勾选记录都归零 */
  const beginRun = useCallback(() => {
    setAutoTick(true);
    ranLive.current = true;
    setGroups([]);
    setChosenKeeps(new Set());
    autoSelected.current = new Set();
    setPanelOpen(true);
  }, []);

  /** 检测成功：这份结果是照当前库算出来的，摘掉"上一趟"的牌子 */
  const markLiveResult = useCallback(() => setRestoredCount(null), []);

  /** 接着上一趟的结果看：只展示不自动勾，并记下它存档时的库内记录数 */
  const restoreFromCache = useCallback((restoredGroups: string[][], libraryCount: number) => {
    setAutoTick(false);
    setRestoredCount(libraryCount);
    setGroups(restoredGroups);
    setDetected(true);
  }, []);

  /** 清空结果时的公共复位；连缓存一起作废由调用方顺手做（各自的缓存 key 不同） */
  const resetResults = useCallback(() => {
    setGroups([]);
    setChosenKeeps(new Set());
    autoSelected.current = new Set();
    setDetected(false);
    setPanelOpen(false);
    setRestoredCount(null);
    setAutoTick(true);
  }, []);

  /** 换某组的保留项：该组勾选跟着翻转，其他组的勾选不动 */
  const keep = useCallback((fromKeep: string, keepId: string) => {
    const group = aliveGroups.find(item => item.keep === fromKeep);
    if (!group) return;
    setChosenKeeps(prev => {
      const next = new Set(prev);
      next.delete(group.keep);
      next.add(keepId);
      return next;
    });
    setSelectedIds(prev => selectGroupExtras(prev, group, keepId));
  }, [aliveGroups, setSelectedIds]);

  /** 每组只留当前标着「保留」的那张，其余全部勾上 */
  const autoSelect = useCallback(() => {
    setAutoTick(true);
    setSelectMode(true);
    setSelectedIds(new Set(extras));
    for (const id of extras) autoSelected.current.add(id);
  }, [extras, setSelectMode, setSelectedIds]);

  /** 一键清空待删清单：面板里的「取消全选」按下去就该什么都不会删 */
  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
  }, [setSelectedIds]);

  /** 整组勾上/取消：勾上时保留张也在里面，全组一起进回收站是有意为之 */
  const setGroupSelection = useCallback((ids: string[], on: boolean) => {
    if (on) setSelectMode(true);
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const id of ids) {
        if (on) next.add(id); else next.delete(id);
      }
      return next;
    });
  }, [setSelectMode, setSelectedIds]);

  /** 看的是上一趟存的分组，且之后库里又添过图：这份分组未必含新添的那批（少掉的不算问题） */
  const stale = restoredResultStale(restoredCount, libraryTotal);

  return {
    groups,
    setGroups,
    chosenKeeps,
    setChosenKeeps,
    detected,
    setDetected,
    detecting,
    setDetecting,
    panelOpen,
    setPanelOpen,
    autoTick,
    setAutoTick,
    ranLive,
    autoSelected,
    imageById,
    aliveGroups,
    extras,
    stale,
    beginRun,
    markLiveResult,
    restoreFromCache,
    resetResults,
    keep,
    autoSelect,
    clearSelection,
    setGroupSelection,
  };
}

/** 查重的「活组」口径：只按组员是否还在库里裁剪 */
export function aliveGroupsForDuplicates(groups: string[][], imageById: Map<string, Image>, chosenKeeps: Set<string>): SimilarGroup[] {
  return liveGroups(groups, imageById, chosenKeeps);
}
