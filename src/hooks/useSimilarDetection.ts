import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { SimilarHash, SimilarProgressPayload } from "../api";
import { restoredResultStale } from "../detectionCache";
import type { Image } from "../types";
import { extrasOfGroups, liveGroups, selectGroupExtras } from "../similarGroups";
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
 */
export function useSimilarDetection({ images, setSelectMode, setSelectedIds, notify }: SimilarOptions) {
  const [groups, setGroups] = useState<string[][]>([]);
  const [farIds, setFarIds] = useState<Set<string>>(NO_FAR);
  const [hashById, setHashById] = useState<Map<string, HashPair>>(NO_HASHES);
  const [chosenKeeps, setChosenKeeps] = useState<Set<string>>(new Set());
  const [threshold, setThreshold] = useState(SIMILAR_THRESHOLD_DEFAULT);
  const [keepRule, setKeepRule] = useState<KeepRule>("earliest");
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [recalculating, setRecalculating] = useState(false);
  const [progress, setProgress] = useState<{ processed: number; total: number } | null>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  /** 从缓存恢复的那一轮别自动勾副本：那是上一趟的结果，凭什么一打开就把人待删清单填满 */
  const [autoTick, setAutoTick] = useState(true);
  /** 非 null = 现在看的是上一趟存的分组，值是它记下时的库内记录数；现跑一次就清成 null */
  const [restoredCount, setRestoredCount] = useState<number | null>(null);
  /** 已经自动勾过的副本：只补勾新冒出来的，免得把用户手动取消的又勾回来 */
  const autoSelected = useRef<Set<string>>(new Set());
  const recalcTimer = useRef<number | null>(null);
  /** 这一轮是不是已经自己点过检测了：缓存回得慢的话别把刚跑出来的结果盖掉 */
  const ranLive = useRef(false);

  /** 只索引相似组里那几百张，避免为 19 万条清单建一张全库 Map */
  const imageById = useMemo(() => {
    const wanted = new Set(groups.flat());
    if (wanted.size === 0) return new Map<string, Image>();
    const byId = new Map<string, Image>();
    for (const image of images) if (wanted.has(image.id)) byId.set(image.id, image);
    return byId;
  }, [images, groups]);

  const aliveGroups = useMemo(
    () => liveGroups(groups, imageById, chosenKeeps, keepRule, farIds),
    [groups, imageById, chosenKeeps, keepRule, farIds],
  );
  const extras = useMemo(() => extrasOfGroups(aliveGroups), [aliveGroups]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<SimilarProgressPayload>("similar-progress", (event) => {
      const { processed, total, done, groups: fresh, far } = event.payload;
      setProgress(done || total === 0 ? null : { processed, total });
      if (fresh.length > 0) {
        // 一出组就算有结果：中途关掉面板还能从工具栏点回去，不必等整趟跑完
        setDetected(true);
        setGroups(fresh);
        setFarIds(new Set(far));
      }
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 事件收不到只是看不到中间进度，检测结束时仍会拿到完整结果 */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  /** 打开应用先接着上一趟的结果看：检测动辄几分钟，不该是"想看就得再跑一遍" */
  useEffect(() => {
    api.getSimilarCache().then(cached => {
      if (!cached || cached.result.groups.length === 0) return;
      // 缓存回得晚于用户自己点的检测，就别把刚跑出来的结果盖回去
      if (ranLive.current) return;
      setAutoTick(false);
      setRestoredCount(cached.libraryCount);
      setThreshold(cached.threshold);
      setHashById(toHashById(cached.result.hashes));
      setGroups(cached.result.groups);
      setFarIds(new Set(cached.result.far));
      setDetected(true);
    }).catch(() => { /* 读不到缓存就是还没跑过检测 */ });
  }, []);

  /**
   * 跟着分组结果重推一遍勾选：新冒出来的副本补勾上，已经不算副本的（换了组、
   * 成了保留张、被降成远亲）撤勾——换宽容度之后留着上一轮的勾就等于按新口径删错图。
   * 用户手动取消过的那张仍不会被我们擅自勾回去。
   * 从缓存恢复的那一份不勾（autoTick 关掉），只把这几张记成"已经处理过"，
   * 免得稍后清单算完又把它们当新冒出来的补勾上去。
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

  const detect = useCallback(async () => {
    setDetecting(true);
    setAutoTick(true);
    ranLive.current = true;
    setGroups([]);
    setFarIds(NO_FAR);
    setHashById(NO_HASHES);
    setChosenKeeps(new Set());
    autoSelected.current = new Set();
    setPanelOpen(true);
    try {
      const found = await api.findSimilarImages(threshold);
      // 这一份是照着当前库算出来的，不再挂"上次结果"的牌子
      setRestoredCount(null);
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
  }, [notify, threshold]);

  /** 换宽容度：指纹都在库里了，只是重新比对一遍，所以结果整份换掉。这是用户主动重算，勾选照旧跟上新口径 */
  const rethreshold = useCallback((next: number) => {
    setThreshold(next);
    setAutoTick(true);
    ranLive.current = true;
    autoSelected.current = new Set();
    if (recalcTimer.current !== null) window.clearTimeout(recalcTimer.current);
    recalcTimer.current = window.setTimeout(async () => {
      recalcTimer.current = null;
      setRecalculating(true);
      try {
        const again = await api.findSimilarImages(next);
        setRestoredCount(null);
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
    setFarIds(NO_FAR);
    setHashById(NO_HASHES);
    setChosenKeeps(new Set());
    autoSelected.current = new Set();
    setDetected(false);
    setPanelOpen(false);
    setRestoredCount(null);
    setAutoTick(true);
    // 面板上按"清空结果"就是连缓存一起作废，不然下次打开又原样复活
    api.clearDetectionCache("similar").catch(() => { /* 清不掉只是重启后还能看到旧结果 */ });
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

  /** 看的是上一趟存的分组，且之后库里又添过图：这份分组未必含新添的那批（少掉的不算问题） */
  const stale = restoredResultStale(restoredCount, images.length);

  /** 每组按当前规则的保留张之外的全部勾上 */
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
  }, [groups, imageById, chosenKeeps, farIds, setSelectMode, setSelectedIds]);

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
